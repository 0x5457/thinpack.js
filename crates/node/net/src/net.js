(() => {
  const { Duplex } = require("node:stream");
  const { codes, opErrorParts } = require("node:thinpack/internal/errors");
  const { UVException } = require("node:thinpack/internal/errors");
  const { parseAddress } = require("node:thinpack/internal/net/address");
  const invalidFdType = (type) => {
    const error = new TypeError(`Unsupported fd type: ${type}`);
    error.code = "ERR_INVALID_FD_TYPE";
    return error;
  };
  const socketError = (error, syscall, path) => {
    const [code, detail] = opErrorParts(error);
    if (detail === undefined) {
      const errno = Number(code);
      return Number.isInteger(errno) ? new UVException({ errno, syscall, path }) : error;
    }
    if (/^-?\d+$/.test(code)) return new UVException({ errno: Number(code), syscall, path });
    if (code === "ERR_INVALID_FD_TYPE") return invalidFdType(detail);
    const failure = new Error(detail);
    failure.code = code;
    return failure;
  };
  const socketOptions = (args) => {
    const first = args[0];
    if (typeof first === "number" ||
        (typeof first === "string" && !Number.isNaN(Number(first)))) {
      return { port: first, host: typeof args[1] === "string" ? args[1] : undefined };
    }
    if (typeof first === "string") return { path: first };
    return { ...first };
  };

  const lastFunction = (args) => {
    for (let index = args.length - 1; index >= 0; index -= 1) {
      if (typeof args[index] === "function") return args[index];
    }
    return undefined;
  };

  class Socket extends Duplex {
    #id = 0;
    #connecting = false;
    #isStream = false;
    #handle = null;
    #timeout = null;
    #timeoutDuration = 0;
    #reading = false;
    #released = false;
    #releaseWaiters = [];
    #attempt = 0;
    #localFamily = undefined;

    constructor(options = {}) {
      super({ ...options, allowHalfOpen: options.allowHalfOpen ?? false });
      this.connecting = false;
      this.encrypted = false;
      this.remoteAddress = undefined;
      this.remotePort = undefined;
      this.remoteFamily = undefined;
      this.localAddress = undefined;
      this.localPort = undefined;
      this.bytesRead = 0;
      this.bytesWritten = 0;
      if (options.handle) {
        this.#adopt(options.handle);
      } else if (options.fd !== undefined) {
        this.#adoptFd(options.fd);
      }
    }

    get pending() {
      return this.#connecting;
    }
    #adoptFd(fd) {
      if (!Number.isInteger(fd) || fd < 0) {
        throw new codes.ERR_INVALID_ARG_TYPE("options.fd", "integer", fd);
      }
      let id;
      try {
        id = Deno.core.ops.op_net_from_fd(fd);
      } catch (error) {
        throw socketError(error, "open");
      }
      this.#adopt(id);
    }

    #adopt(id) {
      this.#id = id;
      this._thinpackConnectionId = id;
      const [kind, localRaw, remoteRaw] = Deno.core.ops.op_net_info(id).split("\0");
      this.connecting = false;
      if (kind !== "tcp") {
        this.#isStream = true;
      } else {
        const local = parseAddress(localRaw);
        const remote = parseAddress(remoteRaw);
        this.localAddress = local.address;
        this.localPort = local.port;
        this.#localFamily = local.family;
        this.remoteAddress = remote.address;
        this.remotePort = remote.port;
        this.remoteFamily = remote.family;
      }
      this._read();
    }

    connect(...args) {
      const options = socketOptions(args);
      const callback = lastFunction(args);
      const path = typeof options.path === "string" ? options.path : undefined;
      const host = options.host ?? "localhost";
      const port = Number(options.port);
      if (path === undefined && (!Number.isInteger(port) || port <= 0 || port > 65535)) {
        throw new codes.ERR_OUT_OF_RANGE("options.port", `1 <= port <= 65535`, port);
      }

      this.connecting = true;
      this.#connecting = true;
      this.#attempt = (Socket.nextAttempt = (Socket.nextAttempt ?? 0) + 1);
      if (callback) this.once("connect", callback);
      if (options.timeout) {
        this.setTimeout(options.timeout, () => this.destroy(new Error("Connect timeout")));
      }

      const connecting = path === undefined
        ? Deno.core.ops.op_net_connect(String(host), port, this.#attempt)
        : Deno.core.ops.op_net_connect_unix(path, this.#attempt);
      connecting
        .then((id) => {
          if (this.destroyed) {
            Deno.core.ops.op_net_close(id);
            return;
          }
          this.#adopt(id);
          this.emit("connect");
          this.emit("ready");
        })
        .catch((error) => {
          this.connecting = false;
          this.#connecting = false;
          this.destroy(socketError(error, "connect", path ?? `${host}:${port}`));
        });
      return this;
    }

    _read() {
      if (!this.readable || this.#released || this.#reading || this.#id === 0) return;
      this.#reading = true;
      const pump = () => {
        Deno.core.ops
          .op_net_read(this.#id)
          .then((chunk) => {
            if (this.#released) {
              this.#settleRelease();
              return;
            }
            if (this.destroyed) return;
            if (chunk.length === 0) {
              this.#reading = false;
              this.push(null);
              return;
            }
            this.bytesRead += chunk.length;
            this.push(chunk);
            pump();
          })
          .catch((error) => {
            this.#reading = false;
            if (this.#released) {
              this.#settleRelease();
              return;
            }
            if (!this.destroyed) this.destroy(socketError(error, "read", this.remoteAddress));
          });
      };
      pump();
    }

    _write(chunk, encoding, callback) {
      Deno.core.ops
        .op_net_write(this.#id, chunk)
        .then((written) => {
          this.bytesWritten += written;
          callback();
        })
        .catch((error) => callback(socketError(error, "write", this.remoteAddress)));
    }

    _final(callback) {
      Deno.core.ops.op_net_shutdown(this.#id).then(
        () => callback(),
        (error) => callback(socketError(error, "shutdown", this.remoteAddress)),
      );
    }

    _destroy(error, callback) {
      if (this._thinpackUpgraded) {
        callback(error);
        return;
      }
      if (this.#timeout) {
        clearTimeout(this.#timeout);
        this.#timeout = null;
      }
      if (this.#id !== 0) {
        Deno.core.ops.op_net_close(this.#id);
        this.#id = 0;
      } else if (this.#attempt !== 0) {
        Deno.core.ops.op_net_cancel_connect(this.#attempt);
      }
      this.connecting = false;
      callback(error);
    }
    _thinpackReleaseSocket() {
      this.#released = true;
      Deno.core.ops.op_net_cancel_read(this.#id);
      if (!this.#reading) return Promise.resolve();
      return new Promise((resolve) => this.#releaseWaiters.push(resolve));
    }

    #settleRelease() {
      const waiters = this.#releaseWaiters;
      this.#releaseWaiters = [];
      for (const resolve of waiters) resolve();
    }

    setTimeout(milliseconds, callback) {
      this.#timeoutDuration = milliseconds;
      if (this.#timeout) clearTimeout(this.#timeout);
      if (milliseconds > 0) {
        this.#timeout = setTimeout(() => {
          this.#timeout = null;
          this.emit("timeout");
          if (callback) callback();
          else this.destroy(new Error("Timeout"));
        }, milliseconds);
        this.#timeout.unref?.();
      }
      return this;
    }
    setNoDelay(noDelay = true) {
      if (this.#isStream || this.#id === 0) return this;
      Deno.core.ops.op_net_set_no_delay(this.#id, Boolean(noDelay));
      return this;
    }

    setKeepAlive(enable = false, initialDelay = 0) {
      if (this.#isStream || this.#id === 0) return this;
      Deno.core.ops.op_net_set_keep_alive(this.#id, Boolean(enable), ~~(Number(initialDelay) / 1000));
      return this;
    }

    address() {
      if (this.#id === 0) return {};
      if (this.#isStream) return {};
      return { address: this.localAddress, family: this.#localFamily, port: this.localPort };
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
  }

  const connect = (...args) => {
    const socket = new Socket();
    return socket.connect(...args);
  };

  const isIP = (value) => {
    if (typeof value !== "string") return 0;
    if (/^(\d{1,3}\.){3}\d{1,3}$/.test(value) &&
        value.split(".").every((part) => Number(part) <= 255)) return 4;
    return value.includes(":") ? 6 : 0;
  };

  class Server extends require("node:events").EventEmitter {
    #listenerId = 0;
    #accepting = null;
    #connections = new Set();
    #local = "";
    #tcp = true;
    #path = null;
    #options;
    #abort;

    constructor(options, connectionListener) {
      super();
      if (typeof options === "function") {
        connectionListener = options;
        options = {};
      }
      this.#options = options === null || options === undefined ? {} : options;
      if (typeof this.#options !== "object") {
        throw new codes.ERR_INVALID_ARG_TYPE("options", "object", this.#options);
      }
      this.listening = false;
      this.maxConnections = this.#options.maxConnections;
      this.dropMaxConnection = this.#options.dropMaxConnection ?? false;
      if (connectionListener) this.on("connection", connectionListener);
      const signal = this.#options.signal;
      if (signal !== undefined) {
        if (signal === null || typeof signal.addEventListener !== "function") {
          throw new codes.ERR_INVALID_ARG_TYPE("options.signal", "AbortSignal", signal);
        }
        this.#abort = () => this.close();
        signal.addEventListener("abort", this.#abort, { once: true });
      }
    }

    listen(...args) {
      const options = socketOptions(args);
      const callback = lastFunction(args);
      if (options.fd !== undefined) {
        this.#listenFd(options.fd, callback);
        return this;
      }
      const path = typeof options.path === "string" ? options.path : undefined;
      if (path !== undefined) {
        this.#listenUnix(path, callback);
        return this;
      }
      const port = Number(options.port ?? 0);
      const host = options.host ?? "0.0.0.0";
      Deno.core.ops
        .op_net_listen(String(host), port)
        .then((id) => this.#listening(id, callback))
        .catch((error) => this.emit("error", socketError(error, "listen", `${host}:${port}`)));
      return this;
    }
    #listenFd(fd, callback) {
      if (!Number.isInteger(fd) || fd < 0) {
        throw new codes.ERR_INVALID_ARG_TYPE("options.fd", "integer", fd);
      }
      let id;
      try {
        id = Deno.core.ops.op_net_listen_fd(fd);
      } catch (error) {
        const failure = socketError(error, "listen");
        process.nextTick(() => this.emit("error", failure));
        return;
      }
      this.#listening(id, callback);
    }

    #listenUnix(path, callback) {
      Deno.core.ops
        .op_net_listen_unix(path)
        .then((id) => {
          this.#path = path;
          this.#listening(id, callback);
        })
        .catch((error) => this.emit("error", socketError(error, "listen", path)));
    }

    #listening(id, callback) {
      this.#listenerId = id;
      const [kind, local] = Deno.core.ops.op_net_listen_info(id).split("\0");
      this.#local = local;
      this.#tcp = kind === "tcp";
      this.listening = true;
      this.emit("listening");
      if (callback) callback();
      this.#accept();
    }

    #accept() {
      const listenerId = this.#listenerId;
      if (listenerId === 0 || this.#accepting !== null) return;
      const pending = Deno.core.ops.op_net_accept(listenerId);
      this.#accepting = pending;
      pending
        .then((id) => {
          this.#accepting = null;
          if (this.#listenerId === 0) {
            if (id) Deno.core.ops.op_net_close(id);
            return;
          }
          const max = this.maxConnections;
          if (max !== undefined && this.#connections.size >= max) {
            const dropped = new Socket({ handle: id });
            this.emit("drop", dropped);
            dropped.destroy();
            this.#accept();
            return;
          }
          const socket = new Socket({
            handle: id,
            allowHalfOpen: this.#options.allowHalfOpen ?? false,
            readable: this.#options.readable !== false,
            writable: this.#options.writable !== false,
            highWaterMark: this.#options.highWaterMark,
          });
          if (this.#options.noDelay !== false) socket.setNoDelay(true);
          if (this.#options.keepAlive) {
            socket.setKeepAlive(true, this.#options.keepAliveInitialDelay ?? 0);
          }
          this.#connections.add(socket);
          socket.on("close", () => {
            this.#connections.delete(socket);
            if (this.listening) this.#accept();
          });
          if (this.#options.pauseOnConnect) socket.pause();
          this.emit("connection", socket);
          this.#accept();
        })
        .catch((error) => {
          this.#accepting = null;
          if (this.listening) this.emit("error", socketError(error, "accept", this.#local));
        });
    }

    address() {
      if (!this.listening) return null;
      if (!this.#tcp) return this.#path;
      return parseAddress(this.#local);
    }

    getConnections(callback) {
      if (typeof callback !== "function") {
        throw new codes.ERR_INVALID_ARG_TYPE("callback", "function", callback);
      }
      process.nextTick(callback, null, this.#connections.size);
    }

    closeAllConnections() {
      for (const socket of this.#connections) socket.destroy();
    }

    closeIdleConnections() {
      for (const socket of this.#connections) {
        if (socket.readableLength === 0 && socket.writableLength === 0) socket.destroy();
      }
    }

    close(callback) {
      const id = this.#listenerId;
      this.#listenerId = 0;
      this.listening = false;
      if (id) Deno.core.ops.op_net_close_listener(id);
      if (this.#abort !== undefined) {
        this.#options.signal.removeEventListener("abort", this.#abort);
        this.#abort = undefined;
      }
      for (const socket of this.#connections) socket.destroy();
      this.#connections.clear();
      const accepting = this.#accepting;
      this.#accepting = null;
      const closed = () => {
        this.emit("close");
        if (callback) callback();
      };
      if (accepting) accepting.then(closed, closed);
      else closed();
      return this;
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
  }

  module.exports = {
    Socket,
    Stream: Socket,
    Server,
    createServer: (options, connectionListener) => new Server(options, connectionListener),
    connect,
    createConnection: connect,
    isIP,
    isIPv4: (value) => isIP(value) === 4,
    isIPv6: (value) => isIP(value) === 6,
  };
})();
