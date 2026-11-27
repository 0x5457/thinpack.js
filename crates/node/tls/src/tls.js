(() => {
  const { Duplex } = require("node:stream");
  const { Socket } = require("node:net");
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
  const clientAlpnOf = (options) =>
    Array.isArray(options.ALPNProtocols)
      ? options.ALPNProtocols.join(",")
      : options.ALPNProtocols ?? "http/1.1";
  const isDuplex = (value) =>
    value !== null &&
    typeof value === "object" &&
    typeof value.on === "function" &&
    typeof value.read === "function" &&
    typeof value.write === "function" &&
    typeof value.end === "function";
  const streamWrapError = () => {
    const error = new Error("JSStreamSocket requires a byte-mode Duplex");
    error.code = "ERR_STREAM_WRAP";
    return error;
  };

  // Adapted from Node's lib/internal/js_stream_socket.js (Copyright Joyent,
  // Inc. and other Node contributors, MIT) and Deno's
  // ext/node/polyfills/internal/js_stream_socket.js (MIT). It gives TLS a
  // genuine stream handle and is also the class http2-wrapper recovers from
  // `_handle._parentWrap.constructor`. I/O stays in JavaScript; no native
  // handle is invented.
  class JSStreamSocket extends Socket {
    #stream;
    #ended = false;
    #released = false;

    constructor(stream) {
      super({ allowHalfOpen: stream.allowHalfOpen === true });
      this.#stream = stream;
      this.stream = stream;
      this.encrypted = false;
      // Node's handle is an extensible JSStream object; h1-over-h2 patches
      // `getpeername` on it because a generic Duplex has no peer address.
      this._handle = {};
      stream.pause();
      stream.on("data", (chunk) => {
        if (typeof chunk === "string" || stream.readableObjectMode === true) {
          stream.pause();
          this.destroy(streamWrapError());
          return;
        }
        if (!this.push(chunk)) stream.pause();
      });
      stream.once("end", () => {
        this.#ended = true;
        this.push(null);
      });
      stream.once("close", () => {
        if (!this.destroyed) this.destroy();
      });
      stream.on("error", (error) => {
        if (!this.destroyed) this.destroy(error);
      });
    }

    _read() {
      if (!this.#ended) this.#stream.resume();
    }

    _write(chunk, encoding, callback) {
      try {
        this.#stream.write(chunk, callback);
      } catch (error) {
        callback(error);
      }
    }

    _final(callback) {
      this.#stream.end(callback);
    }

    _destroy(error, callback) {
      if (!this.#released) {
        this.#released = true;
        this.#stream.destroy(error);
      }
      callback(error);
    }
  }

  // Shape of Node's TLSWrap for the JavaScript transport path: `_parentWrap` is
  // the real adapter, and the rustls session lives in the ops table behind id.
  class TlsStreamHandle {
    constructor(adapter) {
      this._parentWrap = adapter;
      this.id = 0;
    }
  }

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

  const STREAM_WRITE = 16 * 1024;

  class TLSSocket extends Duplex {
    #id = 0;
    #attempt = 0;
    #reading = false;
    #released = false;
    #releaseWaiters = [];
    #timeout = null;
    #pendingWrite = null;
    #pendingFinal = null;
    #tlsOptions = {};
    #transport = null;
    #streamId = 0;
    #started = false;
    #earlyError = null;
    #secureEstablished = false;
    #peerClosed = false;
    #transportEnded = false;
    #readEnded = false;
    #draining = false;
    #activeDone = null;
    #pumping = false;
    #transportReleased = false;

    constructor(socketOrOptions = {}, tlsOptions) {
      const socket = isDuplex(socketOrOptions) ? socketOrOptions : undefined;
      const options = socket === undefined
        ? socketOrOptions ?? tlsOptions ?? {}
        : tlsOptions ?? {};
      super({
        ...options,
        allowHalfOpen: socket?.allowHalfOpen ?? options.allowHalfOpen ?? false,
      });
      this.encrypted = true;
      this.authorized = false;
      this.alpnProtocol = undefined;
      this.servername = undefined;
      this.connecting = false;
      this.secureConnecting = false;
      this.server = options.server;
      this.#tlsOptions = options;
      if (socket !== undefined) this.#attach(socket);
    }

    #attach(stream) {
      this.#transport = new JSStreamSocket(stream);
      this.#transport.on("error", (error) => {
        if (!this.#started) {
          this.#earlyError ??= error;
          return;
        }
        if (!this.destroyed) this.destroy(error);
      });
      this._handle = new TlsStreamHandle(this.#transport);
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
      if (this.#pendingWrite !== null) {
        const { chunk, callback } = this.#pendingWrite;
        this.#pendingWrite = null;
        this.#writeNow(chunk, callback);
        return;
      }
      if (this.#pendingFinal !== null) {
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

    #startStream() {
      if (this.#started || this.#transport === null || this.destroyed) return;
      this.#started = true;
      const options = this.#tlsOptions;
      if (options.isServer === true) {
        this.destroy(notImplemented("TLS server over a Duplex"));
        return;
      }
      if (this.#earlyError !== null) {
        this.destroy(this.#earlyError);
        return;
      }
      if (this.#transport.destroyed) {
        this.destroy();
        return;
      }
      // rustls verifies the server name during the handshake; a custom identity
      // hook cannot run on top because the peer certificate never reaches JS.
      if ((options.checkServerIdentity ?? module.exports.checkServerIdentity) !==
          checkServerIdentity) {
        this.destroy(notImplemented(
          "tls.connect with a custom checkServerIdentity (the peer certificate is not exposed by this TLS implementation)",
        ));
        return;
      }
      const context = options.secureContext ?? {};
      const servername = options.servername ?? options.host ?? options.hostname ?? "localhost";
      this.servername = servername;
      this.connecting = true;
      this.secureConnecting = true;
      try {
        this.#streamId = Deno.core.ops.op_tls_stream_client(
          String(servername),
          pem(options.ca ?? context.ca),
          pem(options.cert ?? context.cert),
          pem(options.key ?? context.key),
          String(clientAlpnOf(options)),
          options.rejectUnauthorized !== false,
        );
        this._handle.id = this.#streamId;
      } catch (error) {
        this.destroy(tlsError(error, "connect", this.servername));
        return;
      }
      const transport = this.#transport;
      transport.on("data", (chunk) => this.#onStreamData(chunk));
      transport.once("end", () => this.#onStreamEnd());
      transport.once("close", () => this.#onStreamClose());
      transport.resume();
      this.#pump();
      this.#updateStreamState();
      this.#flushPending();
    }

    #onStreamData(chunk) {
      if (this.destroyed || this.#streamId === 0) return;
      try {
        Deno.core.ops.op_tls_stream_feed(this.#streamId, chunk);
      } catch (error) {
        this.destroy(tlsError(error, "read", this.servername));
        return;
      }
      this.#drainStream();
      if (this.destroyed) return;
      this.#pump();
      this.#flushPending();
    }

    #onStreamEnd() {
      if (this.#transportEnded || this.destroyed) return;
      this.#transportEnded = true;
      if (!this.#started || this.#streamId === 0) {
        if (!this.#readEnded) {
          this.#readEnded = true;
          this.push(null);
        }
        return;
      }
      try {
        Deno.core.ops.op_tls_stream_eof(this.#streamId);
      } catch (error) {
        this.destroy(tlsError(error, "read", this.servername));
        return;
      }
      this.#drainStream();
      if (this.destroyed) return;
      this.#pump();
      this.#flushPending();
    }

    #onStreamClose() {
      if (this.destroyed) return;
      this.#onStreamEnd();
      if (!this.destroyed && !this.#peerClosed) {
        this.destroy(new Error("TLS transport closed"));
      }
    }

    // Pull plaintext into the readable side. A full readable pauses the
    // transport, but buffered rustls plaintext is drained first on every
    // resume, and EOF is queued only after that buffer is empty.
    #drainStream() {
      if (this.#draining || this.#readEnded || this.destroyed || this.#streamId === 0) {
        return false;
      }
      this.#draining = true;
      try {
        let stateUpdated = false;
        for (;;) {
          let chunk;
          try {
            chunk = Deno.core.ops.op_tls_stream_read(this.#streamId);
          } catch (error) {
            this.destroy(tlsError(error, "read", this.servername));
            return false;
          }
          if (chunk.length === 0) break;
          // The handshake can complete in the same feed as the peer's first
          // application data. Publish authorized/secureConnect before that
          // plaintext reaches user data handlers, and stop if one destroys us.
          if (!stateUpdated) {
            stateUpdated = true;
            this.#updateStreamState();
            if (this.destroyed) return false;
          }
          if (!this.push(chunk)) {
            this.#transport.pause();
            this.#updateStreamState();
            return false;
          }
        }
        this.#updateStreamState();
        if (this.destroyed) return false;
        if (this.#peerClosed && !this.#readEnded) {
          this.#readEnded = true;
          this.push(null);
          return false;
        }
        return true;
      } finally {
        this.#draining = false;
      }
    }

    #updateStreamState() {
      if (this.#streamId === 0 || this.destroyed) return;
      const [alpn, authorized, handshaking, peerClosed] = Deno.core.ops
        .op_tls_stream_info(this.#streamId)
        .split("\0");
      this.alpnProtocol = alpn || false;
      this.authorized = authorized === "1";
      this.#peerClosed = peerClosed === "1";
      if (handshaking === "1" || this.#secureEstablished) return;
      this.#secureEstablished = true;
      this.connecting = false;
      this.secureConnecting = false;
      this.emit("secureConnect");
      this.emit("secure");
    }

    // One ciphertext write at a time. The next chunk is taken directly from
    // rustls only after the transport reports the previous write complete, and
    // completion arrives on a fresh microtask so a synchronous Duplex cannot
    // re-enter the TLS engine.
    #pump(done = null) {
      if (done !== null) {
        const previous = this.#activeDone;
        this.#activeDone = previous === null
          ? done
          : (error) => {
              previous(error);
              done(error);
            };
      }
      if (this.#transport === null || this.destroyed || this.#pumping) return;
      if (this.#streamId === 0) {
        if (this.#activeDone !== null) {
          this.#finishPump(new Error("Connection closed"));
        }
        return;
      }
      this.#pumping = true;
      this.#pumpOnce();
    }

    #pumpOnce() {
      let chunk;
      try {
        chunk = Deno.core.ops.op_tls_stream_output(this.#streamId);
      } catch (error) {
        const failure = tlsError(error, "write", this.servername);
        this.#finishPump(failure);
        this.destroy(failure);
        return;
      }
      if (chunk.length === 0) {
        this.#finishPump();
        return;
      }
      try {
        this.#transport.write(chunk, (error) => {
          queueMicrotask(() => {
            if (this.destroyed) {
              this.#pumping = false;
              return;
            }
            if (error) {
              this.#finishPump(error);
              this.destroy(error);
              return;
            }
            this.#pumpOnce();
          });
        });
      } catch (error) {
        this.#finishPump(error);
        this.destroy(error);
      }
    }

    #finishPump(error = null) {
      this.#pumping = false;
      const done = this.#activeDone;
      this.#activeDone = null;
      if (done !== null) done(error ?? undefined);
    }

    #writeStream(chunk, callback) {
      const step = (offset) => {
        if (this.destroyed || this.#streamId === 0) {
          callback(new Error("Connection closed"));
          return;
        }
        const end = Math.min(offset + STREAM_WRITE, chunk.length);
        try {
          Deno.core.ops.op_tls_stream_write(
            this.#streamId,
            chunk.subarray(offset, end),
          );
        } catch (error) {
          callback(tlsError(error, "write", this.servername));
          return;
        }
        if (end === chunk.length) {
          this.#pump(callback);
        } else {
          // A transport error belongs to the user's write. Do not queue the
          // remaining plaintext slices as though the previous one succeeded.
          this.#pump((error) => {
            if (error) callback(error);
            else step(end);
          });
        }
      };
      step(0);
    }

    #shutdownStream(callback) {
      if (this.#streamId === 0) {
        callback();
        return;
      }
      try {
        Deno.core.ops.op_tls_stream_shutdown(this.#streamId);
      } catch (error) {
        callback(tlsError(error, "shutdown", this.servername));
        return;
      }
      this.#pump((error) => {
        if (error) {
          callback(error);
          return;
        }
        this.#endTransport(callback);
      });
    }

    // close_notify and every prior write have reached the transport before its
    // write half is closed; the read half stays open for the peer's response.
    #endTransport(callback) {
      if (this.#transport === null || this.#transportReleased) {
        callback();
        return;
      }
      let called = false;
      const done = (error) => {
        if (called) return;
        called = true;
        callback(error);
      };
      try {
        this.#transport.end(done);
      } catch (error) {
        done(error);
      }
    }

    #flushPending() {
      if (!this.#secureEstablished || this.#streamId === 0 || this.destroyed) return;
      if (this.#pendingWrite !== null) {
        const { chunk, callback } = this.#pendingWrite;
        this.#pendingWrite = null;
        this.#writeStream(chunk, callback);
        return;
      }
      if (this.#pendingFinal !== null) {
        const callback = this.#pendingFinal;
        this.#pendingFinal = null;
        this.#shutdownStream(callback);
      }
    }

    #releaseStream(error) {
      const failure = error ?? new Error("Connection closed");
      this.#pumping = false;
      if (this.#streamId !== 0) {
        Deno.core.ops.op_tls_stream_close(this.#streamId);
        this.#streamId = 0;
      }
      if (this.#activeDone !== null) {
        const done = this.#activeDone;
        this.#activeDone = null;
        done(failure);
      }
      if (this.#pendingWrite !== null) {
        const { callback } = this.#pendingWrite;
        this.#pendingWrite = null;
        callback(failure);
      }
      if (this.#pendingFinal !== null) {
        const callback = this.#pendingFinal;
        this.#pendingFinal = null;
        callback(failure);
      }
      if (!this.#transportReleased) {
        this.#transportReleased = true;
        this.#transport.destroy(error);
      }
      this._handle = null;
      this.connecting = false;
      this.secureConnecting = false;
    }

    connect(options = {}) {
      if (this.#transport !== null) {
        if (this.#streamId === 0) {
          this.#tlsOptions = { ...this.#tlsOptions, ...options };
          this.#startStream();
        }
        return this;
      }
      const port = Number(options.port ?? 443);
      const host = options.host ?? options.hostname ?? "localhost";
      const servername = options.servername ?? host;
      this.servername = servername;
      const upgrade = options.socket;
      if (upgrade !== undefined && upgrade._thinpackConnectionId === undefined) {
        if (!isDuplex(upgrade)) {
          this.destroy(notImplemented("tls.connect with a non-thinpack socket"));
          return this;
        }
        this.#tlsOptions = { ...this.#tlsOptions, ...options };
        this.#attach(upgrade);
        this.#startStream();
        return this;
      }
      // rustls verifies the server name during the handshake; a custom identity
      // hook cannot run on top because the peer certificate never reaches JS.
      // Refuse the connection loudly rather than silently skipping the check.
      if ((options.checkServerIdentity ?? module.exports.checkServerIdentity) !==
          checkServerIdentity) {
        this.destroy(notImplemented(
          "tls.connect with a custom checkServerIdentity (the peer certificate is not exposed by this TLS implementation)",
        ));
        return this;
      }
      this.connecting = true;
      this.#attempt = (TLSSocket.nextAttempt = (TLSSocket.nextAttempt ?? 0) + 1);
      const alpn = clientAlpnOf(options);
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

    _start() {
      if (this.#transport !== null) this.#startStream();
    }

    _read() {
      if (this.#transport !== null) {
        if (!this.#started || this.destroyed) return;
        if (!this.#drainStream()) return;
        if (!this.#transportEnded) this.#transport.resume();
        return;
      }
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
      if (this.#transport !== null) {
        if (!this.#secureEstablished || this.#streamId === 0) {
          this.#pendingWrite = { chunk, callback };
          return;
        }
        this.#writeStream(chunk, callback);
        return;
      }
      if (this.#id === 0) {
        this.#pendingWrite = { chunk, callback };
        return;
      }
      this.#writeNow(chunk, callback);
    }

    _final(callback) {
      if (this.#transport !== null) {
        if (!this.#secureEstablished || this.#streamId === 0) {
          if (this.#pendingFinal !== null) {
            this.#pendingFinal(new Error("Connection closed"));
          }
          this.#pendingFinal = callback;
          return;
        }
        this.#shutdownStream(callback);
        return;
      }
      if (this.#id === 0) {
        this.#pendingFinal = callback;
        return;
      }
      this.#shutdownNow(callback);
    }

    _destroy(error, callback) {
      if (this.#timeout) {
        clearTimeout(this.#timeout);
        this.#timeout = null;
      }
      if (this.#transport !== null) {
        this.#releaseStream(error);
        callback(error);
        return;
      }
      const pending = this.#pendingWrite;
      this.#pendingWrite = null;
      if (pending !== null) {
        pending.callback(error ?? new Error("Connection closed"));
      }
      if (this.#pendingFinal) {
        const finalCallback = this.#pendingFinal;
        this.#pendingFinal = null;
        finalCallback(error ?? new Error("Connection closed"));
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
      // The ops layer does not surface the negotiated peer certificate; report
      // that honestly instead of handing back an empty object.
      throw notImplemented("tls.TLSSocket.getPeerCertificate");
    }

    getSession() {
      throw notImplemented("tls.TLSSocket.getSession");
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
    if (options.socket !== undefined &&
        options.socket._thinpackConnectionId === undefined &&
        isDuplex(options.socket)) {
      const socket = new TLSSocket(options.socket, options);
      if (callback) socket.once("secureConnect", callback);
      socket._start();
      return socket;
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

  // rustls performs the real hostname verification during the handshake, so the
  // default hook has nothing left to check. `connect` rejects any *replacement*
  // hook loudly (see TLSSocket.connect).
  const checkServerIdentity = () => undefined;

  module.exports = {
    TLSSocket,
    Server,
    SecureContext,
    createServer: (options, listener) => new Server(options, listener),
    connect,
    createSecureContext: (options) => new SecureContext(options),
    checkServerIdentity,
    getCACertificates: () => {
      throw notImplemented("tls.getCACertificates");
    },
    DEFAULT_MIN_VERSION: "TLSv1.2",
    DEFAULT_MAX_VERSION: "TLSv1.3",
  };
})();
