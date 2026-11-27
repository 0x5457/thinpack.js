(() => {
  const net = require("node:net");
  const { Readable, Writable } = require("node:stream");
  const { codes } = require("node:thinpack/internal/errors");
  const { Parser, ParseError, REQUEST, RESPONSE } = require("node:thinpack/internal/llhttp");

  const notImplemented = (api) => new codes.ERR_NOT_IMPLEMENTED(api);

  const STATUS_CODES = {
    100: "Continue", 101: "Switching Protocols", 102: "Processing",
    103: "Early Hints", 200: "OK", 201: "Created", 202: "Accepted",
    203: "Non-Authoritative Information", 204: "No Content",
    205: "Reset Content", 206: "Partial Content", 207: "Multi-Status",
    208: "Already Reported", 226: "IM Used", 300: "Multiple Choices",
    301: "Moved Permanently", 302: "Found", 303: "See Other",
    304: "Not Modified", 305: "Use Proxy", 307: "Temporary Redirect",
    308: "Permanent Redirect", 400: "Bad Request", 401: "Unauthorized",
    402: "Payment Required", 403: "Forbidden", 404: "Not Found",
    405: "Method Not Allowed", 406: "Not Acceptable",
    407: "Proxy Authentication Required", 408: "Request Timeout",
    409: "Conflict", 410: "Gone", 411: "Length Required",
    412: "Precondition Failed", 413: "Payload Too Large",
    414: "URI Too Long", 415: "Unsupported Media Type",
    416: "Range Not Satisfiable", 417: "Expectation Failed",
    418: "I'm a Teapot", 421: "Misdirected Request",
    422: "Unprocessable Entity", 423: "Locked", 424: "Failed Dependency",
    425: "Too Early", 426: "Upgrade Required", 428: "Precondition Required",
    429: "Too Many Requests", 431: "Request Header Fields Too Large",
    451: "Unavailable For Legal Reasons", 500: "Internal Server Error",
    501: "Not Implemented", 502: "Bad Gateway", 503: "Service Unavailable",
    504: "Gateway Timeout", 505: "HTTP Version Not Supported",
    506: "Variant Also Negotiates", 507: "Insufficient Storage",
    508: "Loop Detected", 509: "Bandwidth Limit Exceeded",
    510: "Not Extended", 511: "Network Authentication Required",
  };

  const METHODS = [
    "ACL", "BIND", "CHECKOUT", "CONNECT", "COPY", "DELETE", "GET", "HEAD",
    "LINK", "LOCK", "M-SEARCH", "MERGE", "MKACTIVITY", "MKCALENDAR", "MKCOL",
    "MOVE", "NOTIFY", "OPTIONS", "PATCH", "POST", "PROPFIND", "PROPPATCH",
    "PURGE", "PUT", "REBIND", "REPORT", "SEARCH", "SOURCE", "SUBSCRIBE",
    "TRACE", "UNBIND", "UNLINK", "UNLOCK", "UNSUBSCRIBE",
  ];

  // llhttp hands a head back as pieces: a header name or value may arrive in several
  // callbacks. Client and server read the head through this.
  class Head {
    url = "";
    status = "";
    headers = {};
    rawHeaders = [];
    #name = "";
    #value = null;

    onUrl(piece) {
      this.url += piece.toString("latin1");
    }

    onStatus(piece) {
      this.status += piece.toString("latin1");
    }

    onField(piece) {
      if (this.#value !== null) this.#commit();
      this.#name += piece.toString("latin1");
    }

    onValue(piece) {
      this.#value = (this.#value ?? "") + piece.toString("latin1");
    }

    // Flushes the pair that is still open when the head ends.
    end() {
      if (this.#name !== "") this.#commit();
    }

    #commit() {
      const name = this.#name.toLowerCase();
      const value = this.#value ?? "";
      this.rawHeaders.push(name, value);
      this.headers[name] =
        this.headers[name] === undefined ? value : `${this.headers[name]}, ${value}`;
      this.#name = "";
      this.#value = null;
    }
  }

  class IncomingMessage extends Readable {
    constructor(socket, method, url, httpVersion, headers) {
      super();
      this.socket = socket;
      this.connection = socket;
      this.method = method;
      this.url = url;
      this.httpVersion = httpVersion;
      this.headers = headers;
      this.rawHeaders = [];
      this.complete = false;
      this.aborted = false;
      this.res = null;
    }

    _read() {}

    setTimeout(milliseconds, callback) {
      this.socket.setTimeout?.(milliseconds, callback);
      return this;
    }
  }

  const kHeaders = Symbol("headers");
  const kChunked = Symbol("chunked");
  const kFinalChunk = Symbol("finalChunk");

  const headersSent = (action) => {
    const error = new Error(`Cannot ${action} headers after they are sent to the client`);
    error.code = "ERR_HTTP_HEADERS_SENT";
    return error;
  };

  // Node's ServerResponse is a function constructor: injectors inherit from it with
  // util.inherits() and call `http.ServerResponse.call(this, req)`.
  function ServerResponse(req) {
    Writable.call(this);
    this.socket = null;
    this.req = req;
    this.statusCode = 200;
    this.statusMessage = undefined;
    this.headersSent = false;
    this.sendDate = true;
    // A standalone response is keep-alive until the server applies the request's
    // connection policy.
    this._keepAlive = true;
    this._hasBody = req?.method !== "HEAD";
    this._header = null;
    this._headerSent = false;
    this._removedContLen = false;
    this[kHeaders] = new Map();
    this[kChunked] = false;
  }

  Object.setPrototypeOf(ServerResponse.prototype, Writable.prototype);
  Object.setPrototypeOf(ServerResponse, Writable);

  Object.defineProperty(ServerResponse.prototype, "connection", {
    get() {
      return this.socket;
    },
    set(socket) {
      this.socket = socket;
    },
  });

  ServerResponse.prototype.setHeader = function setHeader(name, value) {
    if (this._header !== null) throw headersSent("set");
    const key = String(name).toLowerCase();
    if (key === "content-length") this._removedContLen = false;
    this[kHeaders].set(key, value);
    return this;
  };

  ServerResponse.prototype.getHeader = function getHeader(name) {
    return this[kHeaders].get(String(name).toLowerCase());
  };

  ServerResponse.prototype.getHeaders = function getHeaders() {
    return Object.fromEntries(this[kHeaders]);
  };

  ServerResponse.prototype.hasHeader = function hasHeader(name) {
    return this[kHeaders].has(String(name).toLowerCase());
  };

  ServerResponse.prototype.removeHeader = function removeHeader(name) {
    if (this._header !== null) throw headersSent("remove");
    const key = String(name).toLowerCase();
    if (key === "content-length") this._removedContLen = true;
    this[kHeaders].delete(key);
  };

  ServerResponse.prototype.writeHead = function writeHead(statusCode, statusMessage, headers) {
    if (this._header !== null) throw headersSent("write");
    this.statusCode = statusCode;
    if (typeof statusMessage === "string") {
      this.statusMessage = statusMessage;
    } else if (statusMessage !== undefined) {
      headers = statusMessage;
    }
    if (headers) {
      const entries = Array.isArray(headers) || typeof headers[Symbol.iterator] === "function"
        ? headers
        : Object.entries(headers);
      for (const [name, value] of entries) {
        this.setHeader(name, value);
      }
    }
    this.statusMessage ??= STATUS_CODES[this.statusCode] ?? "Unknown";
    this._storeHeader();
    return this;
  };

  ServerResponse.prototype._storeHeader = function _storeHeader(finalLength) {
    if (String(this.getHeader("connection") ?? "").toLowerCase() === "close") {
      this._keepAlive = false;
    }
    const length = this.getHeader("content-length");
    // An implicit end() has a known body length. This framing is
    // serialization-only: getHeader never reports the inferred length.
    const statusCode = this.statusCode;
    const expectsBody = this._hasBody && statusCode !== 204 && statusCode !== 304 &&
      !(statusCode >= 100 && statusCode < 200);
    const inferredLength = typeof finalLength === "number" &&
      expectsBody &&
      this.req?.httpVersion === "1.1" &&
      length === undefined &&
      !this._removedContLen &&
      this.getHeader("transfer-encoding") === undefined &&
      this.getHeader("trailer") === undefined;
    this[kChunked] = length === undefined && this._hasBody &&
      this.req?.httpVersion === "1.1" && !inferredLength;

    const lines = [];
    for (const [name, value] of this[kHeaders]) {
      if (name === "content-length" || name === "transfer-encoding" || name === "connection") {
        continue;
      }
      if (Array.isArray(value)) {
        for (const entry of value) lines.push(`${name}: ${entry}`);
      } else {
        lines.push(`${name}: ${value}`);
      }
    }
    if (length !== undefined) {
      lines.push(`Content-Length: ${length}`);
    } else if (inferredLength) {
      lines.push(`Content-Length: ${finalLength}`);
    } else if (this[kChunked]) {
      lines.push("Transfer-Encoding: chunked");
    }
    if (this.sendDate && !this[kHeaders].has("date")) {
      lines.push(`Date: ${new Date().toUTCString()}`);
    }
    lines.push(`Connection: ${this._keepAlive ? "keep-alive" : "close"}`);

    const statusMessage = this.statusMessage ?? STATUS_CODES[this.statusCode] ?? "Unknown";
    this._header = `HTTP/1.1 ${this.statusCode} ${statusMessage}\r\n` +
      `${lines.join("\r\n")}\r\n\r\n`;
    this.headersSent = true;
  };

  ServerResponse.prototype._sendHead = function _sendHead(finalLength) {
    if (this._header === null) this._storeHeader(finalLength);
    if (this._headerSent) return;
    this._headerSent = true;
    this.socket.write(this._header);
  };

  ServerResponse.prototype._writeChunk = function _writeChunk(chunk) {
    if (!this._hasBody || chunk.length === 0) return;
    if (this[kChunked]) {
      this.socket.write(`${chunk.length.toString(16)}\r\n`);
      this.socket.write(chunk);
      this.socket.write("\r\n");
    } else {
      this.socket.write(chunk);
    }
  };

  ServerResponse.prototype._finish = function _finish() {
    // end() with no writes is the only path that reaches _finish before the head exists.
    this._sendHead(0);
    if (this[kChunked]) this.socket.write("0\r\n\r\n");
    if (!this._keepAlive) this.socket.end();
  };

  ServerResponse.prototype.assignSocket = function assignSocket(socket) {
    if (socket._httpMessage) {
      const error = new Error("ServerResponse has an already assigned socket");
      error.code = "ERR_HTTP_SOCKET_ASSIGNED";
      throw error;
    }
    socket._httpMessage = this;
    this.socket = socket;
    this.emit("socket", socket);
    if (this._header !== null) this._sendHead();
  };

  ServerResponse.prototype.detachSocket = function detachSocket(socket) {
    if (socket._httpMessage === this) socket._httpMessage = null;
    this.socket = null;
  };

  ServerResponse.prototype.flushHeaders = function flushHeaders() {
    if (this.socket === null) {
      if (this._header === null) this._storeHeader();
      return;
    }
    this._sendHead();
  };

  ServerResponse.prototype.end = function end(chunk, encoding, callback) {
    if (chunk == null || typeof chunk === "function" || this._header !== null ||
      this.writableLength !== 0 || this.writableBuffer.length !== 0) {
      return Writable.prototype.end.call(this, chunk, encoding, callback);
    }
    // Only the first write dispatched by end() is a complete body. Let Writable
    // normalize its arguments and encoding before measuring the actual bytes.
    this[kFinalChunk] = true;
    try {
      return Writable.prototype.end.call(this, chunk, encoding, callback);
    } finally {
      this[kFinalChunk] = false;
    }
  };

  ServerResponse.prototype._write = function _write(chunk, encoding, callback) {
    try {
      this._sendHead(this[kFinalChunk] ? chunk.byteLength : undefined);
      this._writeChunk(chunk);
      callback();
    } catch (error) {
      callback(error);
    }
  };

  ServerResponse.prototype._final = function _final(callback) {
    try {
      this._finish();
      callback();
    } catch (error) {
      callback(error);
    }
  };

  // A socket timeout is advisory until the request, response, and server all ignore
  // it; this mirrors Node's socketOnTimeout.
  const socketOnTimeout = (socket, request) => {
    const requestHandled =
      request !== null && !request.complete && request.emit("timeout", socket);
    const responseHandled = socket._httpMessage?.emit("timeout", socket);
    const serverHandled = socket.server.emit("timeout", socket);
    if (!requestHandled && !responseHandled && !serverHandled) socket.destroy();
  };

  class Server extends net.Server {
    constructor(options, requestListener) {
      if (typeof options === "function") {
        requestListener = options;
        options = {};
      }
      super();
      // Node's connection and message deadlines. The socket timeout is idle-based;
      // headers and request deadlines cover the request currently being parsed.
      this.timeout = 0;
      this.keepAliveTimeout = options?.keepAliveTimeout ?? 5000;
      this.keepAliveTimeoutBuffer = options?.keepAliveTimeoutBuffer ?? 1000;
      this.requestTimeout = options?.requestTimeout ?? 300000;
      this.headersTimeout = options?.headersTimeout ?? Math.min(60000, this.requestTimeout);
      this.on("connection", (socket) => this.#connection(socket));
      if (requestListener) this.on("request", requestListener);
    }

    setTimeout(msecs, callback) {
      this.timeout = msecs;
      if (callback) this.on("timeout", callback);
      return this;
    }

    #connection(socket) {
      const server = this;
      let head = new Head();
      let request = null;
      let upgrade = false;
      let upgraded = false;
      let activeRequests = 0;
      let idle = false;
      let headersTimer = null;
      let requestTimer = null;
      const pending = [];

      const clearRequestTimers = () => {
        if (headersTimer !== null) clearTimeout(headersTimer);
        if (requestTimer !== null) clearTimeout(requestTimer);
        headersTimer = null;
        requestTimer = null;
      };

      const requestTimedOut = () => {
        clearRequestTimers();
        if (socket.destroyed) return;
        // A response already on the wire cannot be replaced by an error page.
        if (socket._httpMessage?._headerSent) {
          socket.destroy();
          return;
        }
        socket.write(
          "HTTP/1.1 408 Request Timeout\r\nConnection: close\r\n\r\n",
          () => socket.destroy(),
        );
      };

      const startRequestTimers = () => {
        clearRequestTimers();
        if (server.headersTimeout > 0) {
          headersTimer = setTimeout(requestTimedOut, server.headersTimeout);
          headersTimer.unref?.();
        }
        if (server.requestTimeout > 0) {
          requestTimer = setTimeout(requestTimedOut, server.requestTimeout);
          requestTimer.unref?.();
        }
      };

      // Keep the idle deadline for a connection with no request in progress and no
      // response waiting to be written.
      const armIdleTimeout = () => {
        if (idle || socket.destroyed || activeRequests !== 0 || pending.length !== 0 ||
            socket._httpMessage !== null || !(server.keepAliveTimeout > 0)) {
          return;
        }
        socket.setTimeout(server.keepAliveTimeout + server.keepAliveTimeoutBuffer);
        idle = true;
      };

      // The first bytes of the next request end the idle phase. Do not leave the
      // keep-alive deadline armed while that request is being parsed.
      const startRequest = () => {
        activeRequests += 1;
        if (!idle) return;
        socket.setTimeout(server.timeout);
        idle = false;
      };

      socket.server = server;
      if (server.timeout > 0) socket.setTimeout(server.timeout);
      const onTimeout = () => socketOnTimeout(socket, request);
      socket.on("timeout", onTimeout);

      const startResponse = (request, response) => {
        response.assignSocket(socket);
        response.once("finish", () => {
          response.detachSocket(socket);
          if (!response._keepAlive) return;
          const next = pending.shift();
          if (next !== undefined) next();
          else armIdleTimeout();
        });
        server.emit("request", request, response);
      };

      const parser = new Parser(REQUEST, {
        onMessageBegin() {
          head = new Head();
          request = null;
          startRequest();
          startRequestTimers();
        },
        onUrl: (piece) => head.onUrl(piece),
        onHeaderField: (piece) => head.onField(piece),
        onHeaderValue: (piece) => head.onValue(piece),
        onHeadersComplete() {
          if (headersTimer !== null) {
            clearTimeout(headersTimer);
            headersTimer = null;
          }
          head.end();
          request = new IncomingMessage(
            socket,
            parser.method,
            head.url,
            parser.version,
            head.headers,
          );
          request.rawHeaders = head.rawHeaders;
          if (/^100-continue$/i.test(head.headers.expect ?? "")) {
            socket.write("HTTP/1.1 100 Continue\r\n\r\n");
          }
          if (/^websocket$/i.test(head.headers.upgrade ?? "")) {
            upgrade = true;
            clearRequestTimers();
            return 2;
          }

          const response = new ServerResponse(request);
          response._keepAlive = parser.keepAlive;
          request.shouldKeepAlive = parser.keepAlive;
          request.res = response;
          if (socket._httpMessage) {
            pending.push(() => startResponse(request, response));
          } else {
            startResponse(request, response);
          }
          return 0;
        },
        onBody: (piece) => request.push(piece),
        onMessageComplete() {
          clearRequestTimers();
          activeRequests -= 1;
          request.complete = true;
          if (!upgrade) request.push(null);
          // A response may finish before the request body; arm its idle deadline now.
          if (request.res?.writableFinished && request.res._keepAlive) armIdleTimeout();
        },
      }, http.maxHeaderSize);

      socket.on("data", (chunk) => {
        if (upgraded) return;
        let rest;
        try {
          rest = parser.execute(chunk);
        } catch (error) {
          if (!(error instanceof ParseError)) throw error;
          socket.destroy();
          return;
        }
        if (rest === null) return;
        upgraded = true;
        parser.destroy();
        // The socket belongs to the upgrade handler now; Node removes its HTTP timeout
        // listener at this handoff.
        socket.removeListener("timeout", onTimeout);
        if (server.listenerCount("upgrade") === 0) {
          socket.destroy();
        } else {
          server.emit("upgrade", request, socket, rest);
        }
      });

      socket.on("end", () => {
        try {
          parser.finish();
        } catch (error) {
          if (!(error instanceof ParseError)) throw error;
          socket.destroy();
        }
      });
      socket.on("close", () => {
        clearRequestTimers();
        parser.destroy();
      });
      socket.on("error", () => socket.destroy());
    }
  }

  class Agent {
    #idle = new Map();

    constructor(options = {}) {
      this.options = options ?? {};
      this.keepAlive = this.options.keepAlive ?? false;
    }
    take(origin) {
      const list = this.#idle.get(origin);
      const socket = list?.pop();
      return socket === undefined || socket.destroyed ? null : socket;
    }

    release(origin, socket) {
      if (!this.keepAlive || socket.destroyed || !socket.writable) {
        socket.destroy();
        return;
      }
      let list = this.#idle.get(origin);
      if (list === undefined) {
        list = [];
        this.#idle.set(origin, list);
      }
      list.push(socket);
    }

    destroy() {
      for (const list of this.#idle.values()) {
        for (const socket of list) socket.destroy();
      }
      this.#idle.clear();
    }
  }

  const normalizeRequestOptions = (options) => {
    if (typeof options === "string" || options instanceof URL) {
      const url = new URL(String(options));
      return {
        protocol: url.protocol,
        hostname: url.hostname,
        port: url.port || undefined,
        path: url.pathname + url.search,
        method: "GET",
      };
    }
    const copy = { ...options };
    copy.hostname ??= copy.host;
    copy.path ??= "/";
    return copy;
  };

  class ClientRequest extends Writable {
    #options;
    #chunks = [];
    #headers = new Map();
    #secure;
    #sent = false;
    #agent;
    #keepAlive = false;
    #origin;

    constructor(options, callback, secure = false) {
      super({ autoDestroy: false });
      this.#options = normalizeRequestOptions(options);
      this.#secure = secure || this.#options.protocol === "https:";
      this.#agent = options.agent === undefined || options.agent === false ? null : options.agent;
      this.#keepAlive = Boolean(this.#agent?.keepAlive);
      for (const [name, value] of Object.entries(this.#options.headers ?? {})) {
        this.#headers.set(String(name).toLowerCase(), value);
      }
      this.method = this.#options.method ?? "GET";
      this.path = this.#options.path;
      this.socket = null;
      this.aborted = false;
      if (callback) this.once("response", callback);
    }

    setHeader(name, value) {
      this.#headers.set(String(name).toLowerCase(), value);
      return this;
    }

    getHeader(name) {
      return this.#headers.get(String(name).toLowerCase());
    }

    removeHeader(name) {
      this.#headers.delete(String(name).toLowerCase());
    }

    hasHeader(name) {
      return this.#headers.has(String(name).toLowerCase());
    }

    _write(chunk, encoding, callback) {
      this.#chunks.push(Buffer.from(chunk));
      callback();
    }

    _final(callback) {
      try {
        this.#send();
        callback();
      } catch (error) {
        callback(error);
      }
    }

    abort() {
      this.destroy();
    }

    setTimeout(milliseconds, callback) {
      if (callback) this.once("timeout", callback);
      const arm = (socket) => {
        const apply = () => socket.setTimeout(milliseconds, () => this.emit("timeout"));
        if (socket.connecting) socket.once("connect", apply);
        else apply();
      };
      if (this.socket !== null) arm(this.socket);
      else this.once("socket", arm);
      return this;
    }

    destroy(error) {
      this.socket?.destroy();
      return super.destroy(error);
    }

    #send() {
      if (this.#sent) return;
      this.#sent = true;
      const options = this.#options;
      const host = options.hostname ?? options.host ?? "localhost";
      const port = Number(options.port ?? (this.#secure ? 443 : 80));
      this.#origin = `${this.#secure ? "https" : "http"}://${host}:${port}`;
      const reused = this.#agent?.take(this.#origin) ?? null;
      const socket = reused ?? (typeof options.createConnection === "function"
        ? options.createConnection({ ...options, port, host })
        : require("node:net").connect(port, host));
      this.socket = socket;
      this.emit("socket", socket);
      socket.on("error", (error) => this.emit("error", error));

      const onConnect = () => {
        const body = Buffer.concat(this.#chunks);
        const headers = new Map(this.#headers);
        if (!headers.has("host")) headers.set("host", `${host}:${port}`);
        if (!headers.has("connection")) {
          headers.set("connection", this.#keepAlive ? "keep-alive" : "close");
        }
        if (body.length > 0 && !headers.has("content-length") &&
            !headers.has("transfer-encoding")) {
          headers.set("content-length", String(body.length));
        }
        let head = `${this.method} ${this.path} HTTP/1.1\r\n`;
        for (const [name, value] of headers) {
          if (Array.isArray(value)) {
            for (const entry of value) head += `${name}: ${entry}\r\n`;
          } else {
            head += `${name}: ${value}\r\n`;
          }
        }
        socket.write(head + "\r\n");
        if (body.length > 0) socket.write(body);
        parseClientResponse(socket, this, this.path, {
          keepAlive: this.#keepAlive,
          release: (candidate) => this.#agent?.release(this.#origin, candidate),
        });
      };
      if (reused !== null) {
        onConnect();
      } else {
        socket.once(this.#secure ? "secureConnect" : "connect", onConnect);
      }
    }
  }

  const parseClientResponse = (socket, request, path, options = {}) => {
    let head = new Head();
    let response = null;
    let keepAlive = Boolean(options.keepAlive);
    let complete = false;
    let finished = false;
    let handedOver = false;

    const cleanup = () => {
      socket.removeListener("data", onData);
      socket.removeListener("end", onEnd);
      socket.removeListener("error", onError);
      socket.removeListener("close", cleanup);
      parser.destroy();
    };

    const finish = () => {
      if (finished) return;
      finished = true;
      response.complete = true;
      response.push(null);
      cleanup();
      if (keepAlive) options.release?.(socket);
    };

    const parser = new Parser(RESPONSE, {
      onMessageBegin() {
        head = new Head();
      },
      onStatus: (piece) => head.onStatus(piece),
      onHeaderField: (piece) => head.onField(piece),
      onHeaderValue: (piece) => head.onValue(piece),
      onBody: (piece) => response?.push(piece),
      onHeadersComplete(statusCode, _, shouldKeepAlive) {
        head.end();
        const message = new IncomingMessage(socket, null, path, parser.version, head.headers);
        message.statusCode = statusCode;
        message.statusMessage = head.status;
        message.rawHeaders = head.rawHeaders;
        if (statusCode === 101) {
          response = message;
          return 2;
        }
        // A 1xx answer is informational; the response being waited for follows it.
        if (statusCode < 200) return 0;
        keepAlive = keepAlive && shouldKeepAlive;
        response = message;
        request.emit("response", message);
        return request.method === "HEAD" ? 1 : 0;
      },
      onMessageComplete() {
        if (response !== null) complete = true;
      },
    }, http.maxHeaderSize);

    // The message is settled outside the callbacks: the parser still owns the chunk it is
    // executing, and an upgrade hands the socket to the caller at that boundary.
    const settle = () => {
      if (!complete || handedOver) return;
      handedOver = true;
      finish();
    };

    const onData = (chunk) => {
      let rest;
      try {
        rest = parser.execute(chunk);
      } catch (error) {
        if (!(error instanceof ParseError)) throw error;
        cleanup();
        if (response === null) request.emit("error", error);
        else response.destroy(error);
        socket.destroy();
        return;
      }
      settle();
      if (rest === null) return;
      handedOver = true;
      cleanup();
      request.emit("upgrade", response, socket, rest);
    };

    const onEnd = () => {
      if (response === null) return;
      try {
        parser.finish();
      } catch (error) {
        if (!(error instanceof ParseError)) throw error;
        cleanup();
        response.destroy(error);
        return;
      }
      settle();
    };

    const onError = (error) => {
      cleanup();
      if (response !== null && !finished) response.destroy(error);
    };

    socket.on("data", onData);
    socket.on("end", onEnd);
    socket.on("error", onError);
    socket.on("close", cleanup);
  };

  const request = (input, options, callback, secure = false) => {
    if (typeof options === "function") {
      callback = options;
      options = undefined;
    }
    if (typeof input !== "string" && !(input instanceof URL)) {
      return new ClientRequest(input, callback, secure);
    }
    const url = new URL(String(input));
    return new ClientRequest({
      protocol: url.protocol,
      hostname: url.hostname,
      port: url.port || undefined,
      path: url.pathname + url.search,
      ...options,
    }, callback, secure);
  };

  const get = (input, options, callback, secure = false) => {
    const req = request(input, options, callback, secure);
    req.end();
    return req;
  };

  const createServer = (options, requestListener) => {
    if (typeof options === "function") {
      requestListener = options;
      options = {};
    }
    return new Server(options, requestListener);
  };

  const unsupported = (name) => () => {
    throw notImplemented(`http.${name}`);
  };

  const http = {
    STATUS_CODES,
    METHODS,
    maxHeaderSize: 16 * 1024,
    IncomingMessage,
    ServerResponse,
    Server,
    Agent,
    createServer,
    request,
    get,
  };

  module.exports = http;
})();
