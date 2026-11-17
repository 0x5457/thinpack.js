(() => {
  const net = require("node:net");
  const { Readable, Writable } = require("node:stream");
  const { codes } = require("node:thinpack/internal/errors");

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
      let buffer = Buffer.alloc(0);
      let state = null;
      let upgraded = false;

      const parseHead = () => {
        const end = buffer.indexOf("\r\n\r\n");
        if (end === -1) return false;
        const head = buffer.subarray(0, end).toString("latin1");
        buffer = buffer.subarray(end + 4);
        const [requestLine, ...headerLines] = head.split("\r\n");
        const [method, url, protocol] = requestLine.split(" ");
        const httpVersion = (protocol ?? "").replace(/^HTTP\//, "");
        const headers = {};
        const raw = [];
        for (const line of headerLines) {
          const index = line.indexOf(":");
          if (index <= 0) continue;
          const name = line.slice(0, index).trim().toLowerCase();
          const value = line.slice(index + 1).trim();
          raw.push(name, value);
          headers[name] = headers[name] ? `${headers[name]}, ${value}` : value;
        }
        const req = new IncomingMessage(socket, method, url, httpVersion, headers);
        req.rawHeaders = raw;
        if (String(headers.upgrade ?? "").toLowerCase() === "websocket") {
          const head = buffer;
          buffer = Buffer.alloc(0);
          upgraded = true;
          state = null;
          if (this.listenerCount("upgrade") === 0) {
            socket.destroy();
          } else {
            this.emit("upgrade", req, socket, head);
          }
          return true;
        }

        const res = new ServerResponse(socket, req);
        const connection = (headers.connection ?? "").toLowerCase();
        const keepAlive = httpVersion === "1.1"
          ? connection !== "close"
          : connection === "keep-alive";
        res._keepAlive = keepAlive;
        req.shouldKeepAlive = keepAlive;
        req.res = res;
        const transferEncoding = (headers["transfer-encoding"] ?? "").toLowerCase();
        state = {
          req,
          res,
          remaining: transferEncoding.includes("chunked")
            ? -1
            : Number(headers["content-length"] ?? 0),
          chunked: transferEncoding.includes("chunked"),
          chunkSize: null,
        };
        if (headers.expect === "100-continue") {
          socket.write("HTTP/1.1 100 Continue\r\n\r\n");
        }
        this.emit("request", req, res);
        if (state && !state.chunked && state.remaining === 0) {
          state.req.complete = true;
          const request = state.req;
          queueMicrotask(() => request.push(null));
          state = null;
        }
        return true;
      };

      socket.on("data", (chunk) => {
        if (upgraded) return;
        buffer = Buffer.concat([buffer, chunk]);
        for (;;) {
          if (!state) {
            if (!parseHead()) return;
            if (upgraded) return;
            if (!state) continue;
          }
          if (state.chunked) {
            for (;;) {
              if (state.chunkSize === null) {
                const lineEnd = buffer.indexOf("\r\n");
                if (lineEnd === -1) return;
                state.chunkSize = Number.parseInt(
                  buffer.subarray(0, lineEnd).toString("latin1").split(";")[0],
                  16,
                );
                buffer = buffer.subarray(lineEnd + 2);
                if (state.chunkSize === 0) {
                  const trailerEnd = buffer.indexOf("\r\n\r\n");
                  const skip = trailerEnd === -1 ? buffer.length : trailerEnd + 4;
                  buffer = buffer.subarray(skip);
                  state.req.complete = true;
                  state.req.push(null);
                  state = null;
                  break;
                }
              }
              if (buffer.length < state.chunkSize + 2) return;
              state.req.push(buffer.subarray(0, state.chunkSize));
              buffer = buffer.subarray(state.chunkSize + 2);
              state.chunkSize = null;
            }
            continue;
          }
          if (state.remaining > 0) {
            const take = Math.min(state.remaining, buffer.length);
            if (take > 0) {
              state.req.push(buffer.subarray(0, take));
              buffer = buffer.subarray(take);
              state.remaining -= take;
            }
            if (state.remaining > 0) return;
          }
          state.req.complete = true;
          state.req.push(null);
          state = null;
        }
      });

      socket.on("error", () => socket.destroy());
    }
  }

  class Agent {
    #idle = new Map();

    constructor(options = {}) {
      this.options = options ?? {};
      this.maxSockets = this.options.maxSockets ?? Infinity;
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
    let buffer = Buffer.alloc(0);
    let response = null;
    let chunked = false;
    let remaining = 0;
    let chunkSize = null;
    let done = false;
    let keepAlive = Boolean(options.keepAlive);

    const cleanup = () => {
      socket.removeListener("data", onData);
      socket.removeListener("end", onEnd);
      socket.removeListener("error", onError);
    };

    const finish = () => {
      if (done) return;
      done = true;
      response.push(null);
      cleanup();
      if (keepAlive) {
        options.release?.(socket);
      }
    };

    const feed = (data) => {
      if (chunked) {
        buffer = Buffer.concat([buffer, data]);
        for (;;) {
          if (chunkSize === null) {
            const end = buffer.indexOf("\r\n");
            if (end < 0) return;
            chunkSize = Number.parseInt(buffer.subarray(0, end).toString("latin1"), 16);
            buffer = buffer.subarray(end + 2);
            if (chunkSize === 0) {
              finish();
              return;
            }
          }
          if (buffer.length < chunkSize + 2) return;
          response.push(buffer.subarray(0, chunkSize));
          buffer = buffer.subarray(chunkSize + 2);
          chunkSize = null;
        }
      }
      if (remaining === Infinity) {
        response.push(data);
        return;
      }
      const take = Math.min(remaining, data.length);
      if (take > 0) {
        response.push(data.subarray(0, take));
        remaining -= take;
      }
      if (remaining === 0) finish();
    };

    const onData = (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      if (response !== null) {
        const data = buffer;
        buffer = Buffer.alloc(0);
        feed(data);
        return;
      }
      const end = buffer.indexOf("\r\n\r\n");
      if (end < 0) return;
      const head = buffer.subarray(0, end).toString("latin1");
      const rest = buffer.subarray(end + 4);
      buffer = Buffer.alloc(0);
      const [statusLine, ...lines] = head.split("\r\n");
      const match = /^HTTP\/([\d.]+) (\d{3}) ?(.*)$/.exec(statusLine);
      if (match === null) {
        request.emit("error", new Error("Invalid HTTP response"));
        socket.destroy();
        return;
      }
      const headers = {};
      const raw = [];
      for (const line of lines) {
        const index = line.indexOf(":");
        if (index <= 0) continue;
        const name = line.slice(0, index).trim().toLowerCase();
        const value = line.slice(index + 1).trim();
        raw.push(name, value);
        headers[name] = headers[name] ? `${headers[name]}, ${value}` : value;
      }
      response = new IncomingMessage(socket, null, path, match[1], headers);
      response.statusCode = Number(match[2]);
      response.statusMessage = match[3];
      response.rawHeaders = raw;
      keepAlive = keepAlive &&
        String(headers.connection ?? "").toLowerCase() !== "close";

      if (response.statusCode === 101) {
        cleanup();
        request.emit("upgrade", response, socket, rest);
        return;
      }
      request.emit("response", response);
      if (rest.length > 0) feed(rest);
      chunked = /chunked/i.test(headers["transfer-encoding"] ?? "");
      remaining = headers["content-length"] !== undefined
        ? Number(headers["content-length"])
        : (chunked ? Infinity : Infinity);
    };

    function onEnd() {
      if (response !== null && !done) finish();
    }

    function onError(error) {
      cleanup();
      if (response !== null && !done) response.destroy(error);
    }

    socket.on("data", onData);
    socket.on("end", onEnd);
    socket.on("error", onError);
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
