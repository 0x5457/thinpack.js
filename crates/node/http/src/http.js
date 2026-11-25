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

  class ServerResponse extends Writable {
    #headers = new Map();
    #sent = false;
    #chunked = false;

    constructor(socket, req) {
      super();
      this.socket = socket;
      this.req = req;
      this.statusCode = 200;
      this.statusMessage = undefined;
      this.headersSent = false;
      this.sendDate = true;
      this._keepAlive = false;
    }

    setHeader(name, value) {
      this.#headers.set(String(name).toLowerCase(), value);
      return this;
    }

    getHeader(name) {
      return this.#headers.get(String(name).toLowerCase());
    }

    getHeaders() {
      return Object.fromEntries(this.#headers);
    }

    hasHeader(name) {
      return this.#headers.has(String(name).toLowerCase());
    }

    removeHeader(name) {
      this.#headers.delete(String(name).toLowerCase());
    }

    writeHead(statusCode, statusMessage, headers) {
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
      return this;
    }

    #head() {
      if (this.#sent) return;
      if (String(this.getHeader("connection") ?? "").toLowerCase() === "close") {
        this._keepAlive = false;
      }
      const length = this.getHeader("content-length");
      this.#chunked =
        length === undefined && this.req.method !== "HEAD" && this.req.httpVersion === "1.1";

      const lines = [];
      for (const [name, value] of this.#headers) {
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
      } else if (this.#chunked) {
        lines.push("Transfer-Encoding: chunked");
      } else if (this.req.method === "HEAD") {
        lines.push("Content-Length: 0");
      }
      if (this.sendDate && !this.#headers.has("date")) {
        lines.push(`Date: ${new Date().toUTCString()}`);
      }
      lines.push(`Connection: ${this._keepAlive ? "keep-alive" : "close"}`);

      this.#sent = true;
      this.headersSent = true;
      const statusMessage =
        this.statusMessage ?? STATUS_CODES[this.statusCode] ?? "Unknown";
      this.socket.write(
        `HTTP/1.1 ${this.statusCode} ${statusMessage}\r\n${lines.join("\r\n")}\r\n\r\n`,
      );
    }

    flushHeaders() {
      this.#head();
    }

    _write(chunk, encoding, callback) {
      try {
        this.#head();
        if (this.req.method !== "HEAD" && chunk.length > 0) {
          if (this.#chunked) {
            this.socket.write(`${chunk.length.toString(16)}\r\n`);
            this.socket.write(chunk);
            this.socket.write("\r\n");
          } else {
            this.socket.write(chunk);
          }
        }
        callback();
      } catch (error) {
        callback(error);
      }
    }

    _final(callback) {
      try {
        this.#head();
        if (this.#chunked) this.socket.write("0\r\n\r\n");
        this.headersSent = true;
        if (!this._keepAlive) this.socket.end();
        callback();
      } catch (error) {
        callback(error);
      }
    }
  }

  class Server extends net.Server {
    constructor(options, requestListener) {
      super();
      this.timeout = 0;
      this.keepAliveTimeout = 5000;
      this.on("connection", (socket) => this.#connection(socket));
      if (requestListener) this.on("request", requestListener);
    }

    #connection(socket) {
      const server = this;
      let head = new Head();
      let request = null;
      let upgrade = false;
      let upgraded = false;

      const parser = new Parser(REQUEST, {
        onMessageBegin() {
          head = new Head();
          request = null;
        },
        onUrl: (piece) => head.onUrl(piece),
        onHeaderField: (piece) => head.onField(piece),
        onHeaderValue: (piece) => head.onValue(piece),
        onHeadersComplete() {
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
            return 2;
          }

          const response = new ServerResponse(socket, request);
          response._keepAlive = parser.keepAlive;
          request.shouldKeepAlive = parser.keepAlive;
          request.res = response;
          server.emit("request", request, response);
          return 0;
        },
        onBody: (piece) => request.push(piece),
        onMessageComplete() {
          request.complete = true;
          if (!upgrade) request.push(null);
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
      socket.on("close", () => parser.destroy());
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

  const request = (options, callback, secure = false) =>
    new ClientRequest(options, callback, secure);

  const get = (options, callback, secure = false) => {
    const req = new ClientRequest(options, callback, secure);
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
