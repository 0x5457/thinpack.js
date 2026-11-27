(() => {
  const { Duplex } = require("node:stream");
  const { codes, opErrorParts } = require("node:thinpack/internal/errors");
  const { UVException } = require("node:thinpack/internal/errors");
  const { parseAddress } = require("node:thinpack/internal/net/address");
  // Set by the cluster child; absent in a primary or a process that does not use cluster.
  const kClusterListen = Symbol.for("thinpack.cluster.listen");

  // Node's active socket handles keep the event loop alive even when user code drops the
  // only JS reference. The runtime's lifetime follows pending ops, so retain the socket
  // object (and with it its read pump) until it closes.
  const activeSockets = new Set();
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
    #pending = null;

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
      activeSockets.add(this);
      this.once("close", () => activeSockets.delete(this));
      this._thinpackConnectionId = id;
      const [kind, localRaw, remoteRaw] = Deno.core.ops.op_net_info(id).split("\0");
      this.connecting = false;
      this.#connecting = false;
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
          this.#replayPending();
        })
        .catch((error) => {
          this.connecting = false;
          this.#connecting = false;
          this.destroy(socketError(error, "connect", path ?? `${host}:${port}`));
        });
      return this;
    }

    // `Writable` serializes `_write` and `_final`, so at most one of them waits
    // for the connection. Keep that one continuation instead of queueing data;
    // the stream's own buffering holds everything behind it.
    #defer(run, fail) {
      this.#pending = { run, fail };
    }

    #replayPending() {
      const pending = this.#pending;
      this.#pending = null;
      if (pending === null) return;
      if (this.destroyed || this.#id === 0) {
        // The socket was destroyed while 'connect' listeners ran. Node reports
        // the closed handle to the write; an end just settles the stream.
        if (pending.fail !== null) pending.fail(new codes.ERR_SOCKET_CLOSED());
        return;
      }
      pending.run();
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
              // `net.Socket` closes its writable half on EOF unless allowHalfOpen is set,
              // even while the readable side is paused. The stream's own end hook only runs
              // once the readable side is consumed, which is too late for a paused socket.
              if (this.allowHalfOpen === false && !this.writableEnded) this.end();
              if (this.allowHalfOpen === false && this.readableFlowing === null) {
                // A paused readable never emits 'end', so release the handle explicitly once
                // the writable half has finished.
                if (this.writableFinished) this.destroy();
                else this.once("finish", () => this.destroy());
              }
              return;
            }
            this.bytesRead += chunk.length;
            this.push(chunk);
            this.#refreshTimeout();
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
      if (this.#connecting) {
        this.#defer(() => this.#writeNow(chunk, callback), callback);
        return;
      }
      this.#writeNow(chunk, callback);
    }

    #writeNow(chunk, callback) {
      this.#refreshTimeout();
      Deno.core.ops
        .op_net_write(this.#id, chunk)
        .then((written) => {
          this.bytesWritten += written;
          callback();
        })
        .catch((error) => callback(socketError(error, "write", this.remoteAddress)));
    }

    _final(callback) {
      if (this.#connecting) {
        // An empty end still waits for the connection; shutting down (or
        // closing) the pending handle would fail, and the readable half has to
        // stay open for the peer's response after the FIN.
        this.#defer(() => this.#shutdownNow(callback), null);
        return;
      }
      this.#shutdownNow(callback);
    }

    #shutdownNow(callback) {
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
      const pending = this.#pending;
      const connected = this.#id !== 0;
      if (this.#timeout) {
        clearTimeout(this.#timeout);
        this.#timeout = null;
      }
      if (connected) {
        // Cancel the pending read first, mirroring `_thinpackReleaseSocket`:
        // `op_net_close` only shuts the fd down (a no-op on a pipe-backed fd),
        // so a blocked read would keep the transport alive and the event loop
        // could never drain.
        Deno.core.ops.op_net_cancel_read(this.#id);
        Deno.core.ops.op_net_close(this.#id);
        this.#id = 0;
      } else {
        this.#pending = null;
        if (this.#attempt !== 0) {
          Deno.core.ops.op_net_cancel_connect(this.#attempt);
        }
      }
      this.connecting = false;
      this.#connecting = false;
      callback(error);
      // `Writable` only runs buffered callbacks once the in-flight write
      // finishes. Report the closed connection to that write first, after the
      // destroy notifications above, so the stream can then settle the rest.
      if (pending !== null && !connected && pending.fail !== null) {
        process.nextTick(
          pending.fail,
          new codes.ERR_SOCKET_CLOSED_BEFORE_CONNECTION(),
        );
      }
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

    // Node's socket timeout is idle-based: each read or write re-arms it, and a
    // timeout is advisory unless someone destroys the socket.
    #refreshTimeout() {
      if (this.#timeout) clearTimeout(this.#timeout);
      this.#timeout = null;
      if (this.#timeoutDuration > 0) {
        this.#timeout = setTimeout(() => {
          this.#timeout = null;
          this.emit("timeout");
        }, this.#timeoutDuration);
        this.#timeout.unref?.();
      }
    }

    setTimeout(milliseconds, callback) {
      this.#timeoutDuration = milliseconds;
      if (callback) {
        if (milliseconds > 0) this.once("timeout", callback);
        else this.removeListener("timeout", callback);
      }
      this.#refreshTimeout();
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

  const isIPv4 = (value) =>
    /^(\d{1,3}\.){3}\d{1,3}$/.test(value) &&
    value.split(".").every((part) => Number(part) <= 255);

  const isIPv6 = (value) => {
    if (typeof value !== "string") return false;
    // Node accepts a zone identifier (`fe80::1%eth0`) whose characters may be
    // alphanumeric or `-`, `.`, `:`; it is not part of the address itself.
    const zone = value.indexOf("%");
    if (zone !== -1) {
      if (!/^[0-9A-Za-z\-.:]+$/.test(value.slice(zone + 1))) return false;
      value = value.slice(0, zone);
    }
    // A trailing dotted quad (`::ffff:127.0.0.1`) stands for the last two groups;
    // substitute plain groups so a single count rule covers both spellings.
    let text = value;
    const lastColon = value.lastIndexOf(":");
    if (lastColon !== -1 && value.indexOf(".", lastColon) !== -1) {
      const tail = value.slice(lastColon + 1);
      if (!isIPv4(tail)) return false;
      text = `${value.slice(0, lastColon + 1)}0:0`;
    }
    // `::` may compress at most one run of zero groups.
    const sections = text.split("::");
    if (sections.length > 2) return false;
    const head = sections[0] === "" ? [] : sections[0].split(":");
    const tail = sections.length === 2 && sections[1] !== "" ? sections[1].split(":") : [];
    const groups = [...head, ...tail];
    if (!groups.every((group) => /^[0-9a-fA-F]{1,4}$/.test(group))) return false;
    return sections.length === 2 ? groups.length <= 7 : groups.length === 8;
  };

  const isIP = (value) => {
    if (typeof value !== "string") return 0;
    if (isIPv4(value)) return 4;
    return isIPv6(value) ? 6 : 0;
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
        if (options.exclusive !== true && Server[kClusterListen] !== undefined) {
          throw new codes.ERR_NOT_IMPLEMENTED("cluster sharing a UNIX socket");
        }
        this.#listenUnix(path, callback);
        return this;
      }
      const port = Number(options.port ?? 0);
      const host = options.host ?? "0.0.0.0";
      // Node treats an explicit reusePort as an exclusive listener, which is exactly the
      // independent binding a cluster worker needs.
      if (options.reusePort === true) options.exclusive = true;
      if (options.exclusive !== true && Server[kClusterListen] !== undefined) {
        Server[kClusterListen](this, { address: host, port }, (bindPort) => {
          this.#bindTcp(host, bindPort, true, callback);
        });
        return this;
      }
      this.#bindTcp(host, port, options.reusePort === true, callback);
      return this;
    }

    #bindTcp(host, port, reusePort, callback) {
      Deno.core.ops
        .op_net_listen(String(host), port, reusePort === true)
        .then((id) => this.#listening(id, callback))
        .catch((error) => this.emit("error", socketError(error, "listen", `${host}:${port}`)));
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
      // Node invokes the listen callback with the server as `this`.
      if (callback) callback.call(this);
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

      const accepting = this.#accepting;
      this.#accepting = null;
      const acceptSettled = accepting
        ? accepting.then(() => {}, () => {})
        : Promise.resolve();
      const closed = () => {
        this.emit("close");
        if (callback) callback();
      };
      // Node stops accepting new connections but keeps the existing ones; 'close' waits
      // until they have all ended.
      const finish = () => {
        if (this.#connections.size === 0) acceptSettled.then(closed);
      };
      if (this.#connections.size === 0) {
        finish();
      } else {
        for (const socket of this.#connections) socket.once("close", finish);
      }
      return this;
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
  }

  // Node's net.Server is callable without `new`; keep that shape while retaining the class
  // internals that private fields need.
  const ServerExport = new Proxy(Server, {
    apply: (target, thisArg, args) => new target(...args),
  });

  module.exports = {
    Socket,
    Stream: Socket,
    Server: ServerExport,
    createServer: (options, connectionListener) => new Server(options, connectionListener),
    connect,
    createConnection: connect,
    isIP,
    isIPv4,
    isIPv6,
  };
})();
