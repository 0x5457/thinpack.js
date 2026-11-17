(() => {
  const { Duplex } = require("node:stream");
  const { EventEmitter } = require("node:events");
  const { codes, opErrorParts, UVException } = require("node:thinpack/internal/errors");
  const { parseAddress } = require("node:thinpack/internal/net/address");

  const notImplemented = (api) => new codes.ERR_NOT_IMPLEMENTED(api);

  const pem = (value) => {
    if (value === undefined || value === null) return "";
    if (Array.isArray(value)) return value.map(pem).join("\n");
    return Buffer.isBuffer(value) ? value.toString("utf8") : String(value);
  };
  const alpnOf = (options) =>
    Array.isArray(options.ALPNProtocols) ? options.ALPNProtocols.join(",") : "";
  class SecureContext {
    constructor(options = {}) {
      this.ca = options.ca;
      this.cert = options.cert;
      this.key = options.key;
    }
  }
  const tlsError = (error, syscall, path) => {
    const [code, detail] = opErrorParts(error);
    if (detail === undefined) return error;
    if (/^-?\d+$/.test(code)) {
      return new UVException({ errno: Number(code), syscall, path });
    }
    const failure = new Error(detail);
    failure.code = code;
    return failure;
  };

  class TLSSocket extends Duplex {
    #id = 0;
    #attempt = 0;
    #reading = false;
    #released = false;
    #releaseWaiters = [];
    #timeout = null;
    #pendingWrites = [];
    #pendingFinal = null;

    constructor(options = {}) {
      super({ ...options, allowHalfOpen: options.allowHalfOpen ?? false });
      this.encrypted = true;
      this.authorized = false;
      this.alpnProtocol = undefined;
      this.servername = undefined;
      this.connecting = false;
    }

    #adopt(id) {
      this.#id = id;
      const [alpn, authorized] = Deno.core.ops.op_tls_info(id).split("\0");
      this.alpnProtocol = alpn || false;
      this.authorized = authorized === "1";
      this.connecting = false;
      this.#flushWrites();
      this.emit("secureConnect");
      this.emit("connect");
      this._read();
    }

    #flushWrites() {
      for (const { chunk, callback } of this.#pendingWrites) {
        this.#writeNow(chunk, callback);
      }
      this.#pendingWrites = [];
      if (this.#pendingFinal) {
        const callback = this.#pendingFinal;
        this.#pendingFinal = null;
        this.#shutdownNow(callback);
      }
    }

    #writeNow(chunk, callback) {
      Deno.core.ops
        .op_tls_write(this.#id, chunk)
        .then(() => callback())
        .catch((error) => callback(tlsError(error, "write", this.servername)));
    }

    #shutdownNow(callback) {
      Deno.core.ops.op_tls_shutdown(this.#id).then(
        () => callback(),
        (error) => callback(tlsError(error, "shutdown", this.servername)),
      );
    }

    connect(options = {}) {
      const port = Number(options.port ?? 443);
      const host = options.host ?? options.hostname ?? "localhost";
      const servername = options.servername ?? host;
      this.servername = servername;
      const upgrade = options.socket;
      if (upgrade !== undefined && upgrade._thinpackConnectionId === undefined) {
        this.destroy(notImplemented("tls.connect with a non-thinpack socket"));
        return this;
      }
      this.connecting = true;
      this.#attempt = (TLSSocket.nextAttempt = (TLSSocket.nextAttempt ?? 0) + 1);
      const alpn = Array.isArray(options.ALPNProtocols)
        ? options.ALPNProtocols.join(",")
        : options.ALPNProtocols ?? "http/1.1";
      const context = options.secureContext ?? {};
      const ca = pem(options.ca ?? context.ca);
      const cert = pem(options.cert ?? context.cert);
      const key = pem(options.key ?? context.key);
      const operation = upgrade === undefined
        ? Deno.core.ops.op_tls_connect(
            String(host),
            port,
            String(servername),
            ca,
            cert,
            key,
            String(alpn),
            options.rejectUnauthorized !== false,
            this.#attempt,
          )
        : (async () => {
            const tcpId = upgrade._thinpackConnectionId;
            upgrade._thinpackUpgraded = true;
            await upgrade._thinpackReleaseSocket?.();
            return Deno.core.ops.op_tls_connect_over(
              tcpId,
              String(servername),
              ca,
              cert,
              key,
              String(alpn),
              options.rejectUnauthorized !== false,
              this.#attempt,
            );
          })();
      operation
        .then((id) => {
          if (this.destroyed) {
            Deno.core.ops.op_tls_close(id);
            return;
          }
          this.#adopt(id);
        })
        .catch((error) => {
          this.connecting = false;
          this.destroy(tlsError(error, "connect", `${host}:${port}`));
        });
      return this;
    }
    _acceptServer(id) {
      this.#id = id;
      const [alpn, authorized] = Deno.core.ops.op_tls_info(id).split("\0");
      this.alpnProtocol = alpn || false;
      this.authorized = authorized === "1";
      this.encrypted = true;
      this.connecting = false;
      this._read();
    }

    _read() {
      if (this.#released || this.#reading || this.#id === 0) return;
      this.#reading = true;
      const pump = () => {
        Deno.core.ops
          .op_tls_read(this.#id)
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
            this.push(chunk);
            pump();
          })
          .catch((error) => {
            this.#reading = false;
            if (this.#released) {
              this.#settleRelease();
              return;
            }
            if (!this.destroyed) {
              this.destroy(tlsError(error, "read", this.servername));
            }
          });
      };
      pump();
    }
    _thinpackReleaseTls() {
      this.#released = true;
      Deno.core.ops.op_tls_cancel_read(this.#id);
      if (!this.#reading) return Promise.resolve();
      return new Promise((resolve) => this.#releaseWaiters.push(resolve));
    }

    get _thinpackTlsId() {
      return this.#id;
    }

    #settleRelease() {
      const waiters = this.#releaseWaiters;
      this.#releaseWaiters = [];
      for (const resolve of waiters) resolve();
    }

    _write(chunk, encoding, callback) {
      if (this.#id === 0) {
        this.#pendingWrites.push({ chunk, callback });
        return;
      }
      this.#writeNow(chunk, callback);
    }

    _final(callback) {
      if (this.#id === 0) {
        this.#pendingFinal = callback;
        return;
      }
      this.#shutdownNow(callback);
    }

    _destroy(error, callback) {
      const pending = this.#pendingWrites;
      this.#pendingWrites = [];
      for (const { callback: writeCallback } of pending) {
        writeCallback(error ?? new Error("Connection closed"));
      }
      if (this.#pendingFinal) {
        const finalCallback = this.#pendingFinal;
        this.#pendingFinal = null;
        finalCallback(error ?? new Error("Connection closed"));
      }
      if (this.#timeout) {
        clearTimeout(this.#timeout);
        this.#timeout = null;
      }
      if (this.#released) {
        this.#id = 0;
        callback(error);
        return;
      }
      if (this.#id !== 0) {
        Deno.core.ops.op_tls_close(this.#id);
        this.#id = 0;
      } else if (this.#attempt !== 0) {
        Deno.core.ops.op_tls_cancel_connect(this.#attempt);
      }
      this.connecting = false;
      callback(error);
    }

    setTimeout(milliseconds, callback) {
      if (this.#timeout) clearTimeout(this.#timeout);
      this.#timeout = setTimeout(() => {
        this.#timeout = null;
        this.emit("timeout");
        if (callback) callback();
        else this.destroy(new Error("Timeout"));
      }, milliseconds);
      this.#timeout.unref?.();
      return this;
    }

    setNoDelay() {
      return this;
    }

    setKeepAlive() {
      return this;
    }

    getPeerCertificate() {
      return {};
    }

    getSession() {
      return undefined;
    }

    isSessionReused() {
      return false;
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
  }

  const connect = (...args) => {
    let options = args[0];
    let callback = args[args.length - 1];
    if (typeof callback !== "function") callback = undefined;
    if (typeof options === "number") {
      options = { port: options, host: typeof args[1] === "string" ? args[1] : undefined };
    } else {
      options = { ...options };
    }
    const socket = new TLSSocket(options);
    if (callback) socket.once("secureConnect", callback);
    return socket.connect(options);
  };

  class Server extends EventEmitter {
    #listenerId = 0;
    #closed = false;
    #options;
    #local = "";

    constructor(options, listener) {
      super();
      if (typeof options === "function") {
        listener = options;
        options = {};
      }
      this.#options = options ?? {};
      this.listening = false;
      if (listener) this.on("secureConnection", listener);
    }

    listen(...args) {
      let options = args[0];
      let callback = args[args.length - 1];
      if (typeof callback !== "function") callback = undefined;
      if (typeof options === "number" || typeof options === "string") {
        options = {
          port: Number(options),
          host: typeof args[1] === "string" ? args[1] : undefined,
        };
      } else if (typeof args[1] === "string") {
        options = { ...options, host: args[1] };
      }
      const port = Number(options?.port ?? 0);
      const host = options?.host ?? "0.0.0.0";
      const source = this.#options;
      const alpn = alpnOf(source);

      Deno.core.ops
        .op_tls_listen(
          String(host),
          port,
          pem(source.cert),
          pem(source.key),
          pem(source.ca),
          Boolean(source.requestCert),
          alpn,
        )
        .then((payload) => {
          const [id, local] = payload.split("\0");
          this.#listenerId = Number(id);
          this.#local = local;
          this.listening = true;
          this.emit("listening");
          if (callback) callback();
          this.#accept();
        })
        .catch((error) => {
          this.emit("error", tlsError(error, "listen", `${host}:${port}`));
        });
      return this;
    }

    async #accept() {
      while (!this.#closed && this.#listenerId !== 0) {
        try {
          const id = await Deno.core.ops.op_tls_accept(this.#listenerId);
          if (this.#closed) {
            Deno.core.ops.op_tls_close(id);
            break;
          }
          const socket = new TLSSocket({ isServer: true });
          socket.server = this;
          socket._acceptServer(id);
          this.emit("secureConnection", socket);
        } catch (error) {
          if (this.#closed) break;
          this.emit("tlsClientError", tlsError(error, "handshake"));
        }
      }
    }

    address() {
      if (this.#local === "") return null;
      return parseAddress(this.#local);
    }
    setSecureContext(options = {}) {
      const next = { ...this.#options, ...options };
      next.cert = options.cert;
      next.key = options.key;
      next.ca = options.ca;
      this.#options = next;
      if (this.#listenerId !== 0) {
        Deno.core.ops.op_tls_set_secure_context(
          this.#listenerId,
          pem(next.cert),
          pem(next.key),
          pem(next.ca),
          Boolean(next.requestCert),
          alpnOf(next),
        );
      }
    }

    close(callback) {
      if (this.#closed) {
        if (callback) queueMicrotask(callback);
        return this;
      }
      this.#closed = true;
      if (this.#listenerId !== 0) {
        Deno.core.ops.op_tls_listener_close(this.#listenerId);
        this.#listenerId = 0;
      }
      queueMicrotask(() => {
        this.emit("close");
        if (callback) callback();
      });
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
    TLSSocket,
    Server,
    SecureContext,
    createServer: (options, listener) => new Server(options, listener),
    connect,
    createSecureContext: (options) => new SecureContext(options),
    checkServerIdentity: () => undefined,
    getCACertificates: () => [],
    DEFAULT_MIN_VERSION: "TLSv1.2",
    DEFAULT_MAX_VERSION: "TLSv1.3",
  };
})();
