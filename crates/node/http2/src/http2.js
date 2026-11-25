(() => {
  const { Buffer } = require("node:buffer");
  const { EventEmitter } = require("node:events");
  const { Duplex } = require("node:stream");
  const net = require("node:net");
  const { codes, opError, opErrorParts, UVException } = require("node:thinpack/internal/errors");
  const { parseAddress } = require("node:thinpack/internal/net/address");

  const notImplemented = (api) => new codes.ERR_NOT_IMPLEMENTED(api);

  const http2Error = (code, message) => {
    const error = new Error(message);
    error.code = code;
    return error;
  };
  const serverError = (error, syscall, path) => {
    const [code, detail] = opErrorParts(error);
    if (detail === undefined) return error;
    if (/^-?\d+$/.test(code)) return new UVException({ errno: Number(code), syscall, path });
    return http2Error(code, detail);
  };

  const pem = (value) => {
    if (value === undefined || value === null) return "";
    if (Array.isArray(value)) return value.map(pem).join("\n");
    return Buffer.isBuffer(value) ? value.toString("utf8") : String(value);
  };

  const DEFAULT_SETTINGS = {
    headerTableSize: 4096,
    enablePush: false,
    initialWindowSize: 65535,
    maxFrameSize: 16384,
    maxConcurrentStreams: 4294967295,
    maxHeaderListSize: 65535,
    maxHeaderSize: 65535,
    enableConnectProtocol: false,
  };

  const getDefaultSettings = () => ({ ...DEFAULT_SETTINGS });

  const constants = {
    NGHTTP2_SESSION_SERVER: 0,
    NGHTTP2_SESSION_CLIENT: 1,
    NGHTTP2_STREAM_STATE_IDLE: 1,
    NGHTTP2_STREAM_STATE_OPEN: 2,
    NGHTTP2_STREAM_STATE_RESERVED_LOCAL: 3,
    NGHTTP2_STREAM_STATE_RESERVED_REMOTE: 4,
    NGHTTP2_STREAM_STATE_HALF_CLOSED_LOCAL: 5,
    NGHTTP2_STREAM_STATE_HALF_CLOSED_REMOTE: 6,
    NGHTTP2_STREAM_STATE_CLOSED: 7,
    NGHTTP2_NO_ERROR: 0,
    NGHTTP2_PROTOCOL_ERROR: 1,
    NGHTTP2_INTERNAL_ERROR: 2,
    NGHTTP2_FLOW_CONTROL_ERROR: 3,
    NGHTTP2_SETTINGS_TIMEOUT: 4,
    NGHTTP2_STREAM_CLOSED: 5,
    NGHTTP2_FRAME_SIZE_ERROR: 6,
    NGHTTP2_REFUSED_STREAM: 7,
    NGHTTP2_CANCEL: 8,
    NGHTTP2_COMPRESSION_ERROR: 9,
    NGHTTP2_CONNECT_ERROR: 10,
    NGHTTP2_ENHANCE_YOUR_CALM: 11,
    NGHTTP2_INADEQUATE_SECURITY: 12,
    NGHTTP2_HTTP_1_1_REQUIRED: 13,
    NGHTTP2_DEFAULT_WEIGHT: 16,
    NGHTTP2_FLAG_NONE: 0,
    NGHTTP2_FLAG_END_STREAM: 1,
    NGHTTP2_FLAG_END_HEADERS: 4,
    NGHTTP2_FLAG_ACK: 1,
    NGHTTP2_FLAG_PADDED: 8,
    NGHTTP2_FLAG_PRIORITY: 32,
    DEFAULT_SETTINGS_HEADER_TABLE_SIZE: 4096,
    DEFAULT_SETTINGS_ENABLE_PUSH: 0,
    DEFAULT_SETTINGS_MAX_CONCURRENT_STREAMS: 4294967295,
    DEFAULT_SETTINGS_INITIAL_WINDOW_SIZE: 65535,
    DEFAULT_SETTINGS_MAX_FRAME_SIZE: 16384,
    DEFAULT_SETTINGS_MAX_HEADER_LIST_SIZE: 65535,
    MAX_MAX_FRAME_SIZE: 16777215,
    MIN_MAX_FRAME_SIZE: 16384,
    MAX_INITIAL_WINDOW_SIZE: 2147483647,
    NGHTTP2_SETTINGS_HEADER_TABLE_SIZE: 1,
    NGHTTP2_SETTINGS_ENABLE_PUSH: 2,
    NGHTTP2_SETTINGS_MAX_CONCURRENT_STREAMS: 3,
    NGHTTP2_SETTINGS_INITIAL_WINDOW_SIZE: 4,
    NGHTTP2_SETTINGS_MAX_FRAME_SIZE: 5,
    NGHTTP2_SETTINGS_MAX_HEADER_LIST_SIZE: 6,
    NGHTTP2_SETTINGS_ENABLE_CONNECT_PROTOCOL: 8,
    HTTP2_HEADER_ACCEPT: "accept",
    HTTP2_HEADER_ACCEPT_CHARSET: "accept-charset",
    HTTP2_HEADER_ACCEPT_ENCODING: "accept-encoding",
    HTTP2_HEADER_ACCEPT_LANGUAGE: "accept-language",
    HTTP2_HEADER_ACCEPT_RANGES: "accept-ranges",
    HTTP2_HEADER_ACCESS_CONTROL_ALLOW_CREDENTIALS: "access-control-allow-credentials",
    HTTP2_HEADER_ACCESS_CONTROL_ALLOW_HEADERS: "access-control-allow-headers",
    HTTP2_HEADER_ACCESS_CONTROL_ALLOW_METHODS: "access-control-allow-methods",
    HTTP2_HEADER_ACCESS_CONTROL_ALLOW_ORIGIN: "access-control-allow-origin",
    HTTP2_HEADER_ACCESS_CONTROL_EXPOSE_HEADERS: "access-control-expose-headers",
    HTTP2_HEADER_ACCESS_CONTROL_MAX_AGE: "access-control-max-age",
    HTTP2_HEADER_ACCESS_CONTROL_REQUEST_HEADERS: "access-control-request-headers",
    HTTP2_HEADER_ACCESS_CONTROL_REQUEST_METHOD: "access-control-request-method",
    HTTP2_HEADER_AGE: "age",
    HTTP2_HEADER_ALLOW: "allow",
    HTTP2_HEADER_AUTHORITY: ":authority",
    HTTP2_HEADER_AUTHORIZATION: "authorization",
    HTTP2_HEADER_CACHE_CONTROL: "cache-control",
    HTTP2_HEADER_CONNECTION: "connection",
    HTTP2_HEADER_CONTENT_DISPOSITION: "content-disposition",
    HTTP2_HEADER_CONTENT_ENCODING: "content-encoding",
    HTTP2_HEADER_CONTENT_LANGUAGE: "content-language",
    HTTP2_HEADER_CONTENT_LENGTH: "content-length",
    HTTP2_HEADER_CONTENT_LOCATION: "content-location",
    HTTP2_HEADER_CONTENT_RANGE: "content-range",
    HTTP2_HEADER_CONTENT_TYPE: "content-type",
    HTTP2_HEADER_COOKIE: "cookie",
    HTTP2_HEADER_DATE: "date",
    HTTP2_HEADER_DNT: "dnt",
    HTTP2_HEADER_ETAG: "etag",
    HTTP2_HEADER_EXPECT: "expect",
    HTTP2_HEADER_EXPIRES: "expires",
    HTTP2_HEADER_FROM: "from",
    HTTP2_HEADER_HOST: "host",
    HTTP2_HEADER_IF_MATCH: "if-match",
    HTTP2_HEADER_IF_MODIFIED_SINCE: "if-modified-since",
    HTTP2_HEADER_IF_NONE_MATCH: "if-none-match",
    HTTP2_HEADER_IF_RANGE: "if-range",
    HTTP2_HEADER_IF_UNMODIFIED_SINCE: "if-unmodified-since",
    HTTP2_HEADER_LAST_MODIFIED: "last-modified",
    HTTP2_HEADER_LINK: "link",
    HTTP2_HEADER_LOCATION: "location",
    HTTP2_HEADER_MAX_FORWARDS: "max-forwards",
    HTTP2_HEADER_METHOD: ":method",
    HTTP2_HEADER_ORIGIN: "origin",
    HTTP2_HEADER_PATH: ":path",
    HTTP2_HEADER_PRAGMA: "pragma",
    HTTP2_HEADER_PROTOCOL: ":protocol",
    HTTP2_HEADER_PROXY_AUTHENTICATE: "proxy-authenticate",
    HTTP2_HEADER_PROXY_AUTHORIZATION: "proxy-authorization",
    HTTP2_HEADER_RANGE: "range",
    HTTP2_HEADER_REFERER: "referer",
    HTTP2_HEADER_REFRESH: "refresh",
    HTTP2_HEADER_RETRY_AFTER: "retry-after",
    HTTP2_HEADER_SCHEME: ":scheme",
    HTTP2_HEADER_SERVER: "server",
    HTTP2_HEADER_SET_COOKIE: "set-cookie",
    HTTP2_HEADER_STATUS: ":status",
    HTTP2_HEADER_STRICT_TRANSPORT_SECURITY: "strict-transport-security",
    HTTP2_HEADER_TE: "te",
    HTTP2_HEADER_TRAILER: "trailer",
    HTTP2_HEADER_TRANSFER_ENCODING: "transfer-encoding",
    HTTP2_HEADER_UPGRADE: "upgrade",
    HTTP2_HEADER_UPGRADE_INSECURE_REQUESTS: "upgrade-insecure-requests",
    HTTP2_HEADER_USER_AGENT: "user-agent",
    HTTP2_HEADER_VARY: "vary",
    HTTP2_HEADER_VIA: "via",
    HTTP2_HEADER_WWW_AUTHENTICATE: "www-authenticate",
    HTTP2_HEADER_X_CONTENT_TYPE_OPTIONS: "x-content-type-options",
    HTTP2_HEADER_X_FRAME_OPTIONS: "x-frame-options",
    HTTP2_HEADER_X_XSS_PROTECTION: "x-xss-protection",
    HTTP2_METHOD_ACL: "ACL",
    HTTP2_METHOD_BIND: "BIND",
    HTTP2_METHOD_CHECKOUT: "CHECKOUT",
    HTTP2_METHOD_CONNECT: "CONNECT",
    HTTP2_METHOD_COPY: "COPY",
    HTTP2_METHOD_DELETE: "DELETE",
    HTTP2_METHOD_GET: "GET",
    HTTP2_METHOD_HEAD: "HEAD",
    HTTP2_METHOD_LINK: "LINK",
    HTTP2_METHOD_LOCK: "LOCK",
    HTTP2_METHOD_MERGE: "MERGE",
    HTTP2_METHOD_MKACTIVITY: "MKACTIVITY",
    HTTP2_METHOD_MKCALENDAR: "MKCALENDAR",
    HTTP2_METHOD_MKCOL: "MKCOL",
    HTTP2_METHOD_MOVE: "MOVE",
    HTTP2_METHOD_MSEARCH: "M-SEARCH",
    HTTP2_METHOD_NOTIFY: "NOTIFY",
    HTTP2_METHOD_OPTIONS: "OPTIONS",
    HTTP2_METHOD_PATCH: "PATCH",
    HTTP2_METHOD_POST: "POST",
    HTTP2_METHOD_PROPFIND: "PROPFIND",
    HTTP2_METHOD_PROPPATCH: "PROPPATCH",
    HTTP2_METHOD_PURGE: "PURGE",
    HTTP2_METHOD_PUT: "PUT",
    HTTP2_METHOD_REBIND: "REBIND",
    HTTP2_METHOD_REPORT: "REPORT",
    HTTP2_METHOD_SEARCH: "SEARCH",
    HTTP2_METHOD_SOURCE: "SOURCE",
    HTTP2_METHOD_SUBSCRIBE: "SUBSCRIBE",
    HTTP2_METHOD_TRACE: "TRACE",
    HTTP2_METHOD_UNBIND: "UNBIND",
    HTTP2_METHOD_UNLINK: "UNLINK",
    HTTP2_METHOD_UNLOCK: "UNLOCK",
    HTTP2_METHOD_UNSUBSCRIBE: "UNSUBSCRIBE",
    HTTP_STATUS_CONTINUE: 100,
    HTTP_STATUS_SWITCHING_PROTOCOLS: 101,
    HTTP_STATUS_PROCESSING: 102,
    HTTP_STATUS_EARLY_HINTS: 103,
    HTTP_STATUS_OK: 200,
    HTTP_STATUS_CREATED: 201,
    HTTP_STATUS_ACCEPTED: 202,
    HTTP_STATUS_NON_AUTHORITATIVE_INFORMATION: 203,
    HTTP_STATUS_NO_CONTENT: 204,
    HTTP_STATUS_RESET_CONTENT: 205,
    HTTP_STATUS_PARTIAL_CONTENT: 206,
    HTTP_STATUS_MULTI_STATUS: 207,
    HTTP_STATUS_ALREADY_REPORTED: 208,
    HTTP_STATUS_IM_USED: 226,
    HTTP_STATUS_MULTIPLE_CHOICES: 300,
    HTTP_STATUS_MOVED_PERMANENTLY: 301,
    HTTP_STATUS_FOUND: 302,
    HTTP_STATUS_SEE_OTHER: 303,
    HTTP_STATUS_NOT_MODIFIED: 304,
    HTTP_STATUS_USE_PROXY: 305,
    HTTP_STATUS_TEMPORARY_REDIRECT: 307,
    HTTP_STATUS_PERMANENT_REDIRECT: 308,
    HTTP_STATUS_BAD_REQUEST: 400,
    HTTP_STATUS_UNAUTHORIZED: 401,
    HTTP_STATUS_PAYMENT_REQUIRED: 402,
    HTTP_STATUS_FORBIDDEN: 403,
    HTTP_STATUS_NOT_FOUND: 404,
    HTTP_STATUS_METHOD_NOT_ALLOWED: 405,
    HTTP_STATUS_NOT_ACCEPTABLE: 406,
    HTTP_STATUS_PROXY_AUTHENTICATION_REQUIRED: 407,
    HTTP_STATUS_REQUEST_TIMEOUT: 408,
    HTTP_STATUS_CONFLICT: 409,
    HTTP_STATUS_GONE: 410,
    HTTP_STATUS_LENGTH_REQUIRED: 411,
    HTTP_STATUS_PRECONDITION_FAILED: 412,
    HTTP_STATUS_PAYLOAD_TOO_LARGE: 413,
    HTTP_STATUS_URI_TOO_LONG: 414,
    HTTP_STATUS_UNSUPPORTED_MEDIA_TYPE: 415,
    HTTP_STATUS_RANGE_NOT_SATISFIABLE: 416,
    HTTP_STATUS_EXPECTATION_FAILED: 417,
    HTTP_STATUS_MISDIRECTED_REQUEST: 421,
    HTTP_STATUS_UNPROCESSABLE_ENTITY: 422,
    HTTP_STATUS_LOCKED: 423,
    HTTP_STATUS_FAILED_DEPENDENCY: 424,
    HTTP_STATUS_TOO_EARLY: 425,
    HTTP_STATUS_UPGRADE_REQUIRED: 426,
    HTTP_STATUS_PRECONDITION_REQUIRED: 428,
    HTTP_STATUS_TOO_MANY_REQUESTS: 429,
    HTTP_STATUS_REQUEST_HEADER_FIELDS_TOO_LARGE: 431,
    HTTP_STATUS_UNAVAILABLE_FOR_LEGAL_REASONS: 451,
    HTTP_STATUS_INTERNAL_SERVER_ERROR: 500,
    HTTP_STATUS_NOT_IMPLEMENTED: 501,
    HTTP_STATUS_BAD_GATEWAY: 502,
    HTTP_STATUS_SERVICE_UNAVAILABLE: 503,
    HTTP_STATUS_GATEWAY_TIMEOUT: 504,
    HTTP_STATUS_HTTP_VERSION_NOT_SUPPORTED: 505,
    HTTP_STATUS_VARIANT_ALSO_NEGOTIATES: 506,
    HTTP_STATUS_INSUFFICIENT_STORAGE: 507,
    HTTP_STATUS_LOOP_DETECTED: 508,
    HTTP_STATUS_BANDWIDTH_LIMIT_EXCEEDED: 509,
    HTTP_STATUS_NOT_EXTENDED: 510,
    HTTP_STATUS_NETWORK_AUTHENTICATION_REQUIRED: 511,
  };
  const headerEntries = (headers) => {
    const entries = [];
    for (const [name, value] of Object.entries(headers)) {
      if (value === undefined || value === null) continue;
      for (const item of Array.isArray(value) ? value : [value]) {
        entries.push([name, String(item)]);
      }
    }
    return entries;
  };

  const decodeHeaders = (entries) => {
    const headers = {};
    for (const [name, value] of entries) {
      if (!(name in headers)) {
        headers[name] = value;
      } else if (name === "set-cookie") {
        headers[name] = Array.isArray(headers[name])
          ? [...headers[name], value]
          : [headers[name], value];
      } else {
        headers[name] = headers[name] + ", " + value;
      }
    }
    return headers;
  };

  class ClientHttp2Stream extends Duplex {
    #id = 0;
    #pumping = false;
    #localClosed = false;
    #endStreamInHeaders = false;
    #headers = null;
    #pendingWrites = [];
    #pendingFinal = null;

    constructor(session, headers, options = {}) {
      super({
        autoDestroy: true,
        allowHalfOpen: true,
        readableHighWaterMark: 65535,
        writableHighWaterMark: 65535,
      });
      this.session = session;
      this.rstCode = constants.NGHTTP2_NO_ERROR;
      this.aborted = false;
      this.pending = true;
      this.#headers = headers;
      this.#endStreamInHeaders = options.endStream === true;
    }
    thinpackOpen(sessionId) {
      if (this.destroyed || this.#id !== 0) return;
      try {
        this.#id = Deno.core.ops.op_http2_request(
          sessionId,
          headerEntries(this.#headers),
          this.#endStreamInHeaders,
        );
      } catch (error) {
        this.destroy(opError(error));
        return;
      }
      this.#headers = null;
      this.pending = false;
      process.nextTick(() => this.emit("ready"));
      for (const { chunk, callback } of this.#pendingWrites.splice(0)) {
        this.#writeNow(chunk, callback);
      }
      if (this.#pendingFinal) {
        const callback = this.#pendingFinal;
        this.#pendingFinal = null;
        this.#finalNow(callback);
      }
      this.#pump();
    }

    _read() {
      this.#pump();
    }

    _write(chunk, encoding, callback) {
      if (this.#id === 0) {
        this.#pendingWrites.push({ chunk, callback });
        return;
      }
      this.#writeNow(chunk, callback);
    }

    #writeNow(chunk, callback) {
      Deno.core.ops.op_http2_write(this.#id, chunk, false).then(
        () => callback(),
        (error) => callback(opError(error)),
      );
    }

    _final(callback) {
      if (this.#endStreamInHeaders) {
        callback();
        return;
      }
      if (this.#id === 0) {
        this.#pendingFinal = callback;
        return;
      }
      this.#finalNow(callback);
    }

    #finalNow(callback) {
      Deno.core.ops.op_http2_write(this.#id, new Uint8Array(0), true).then(
        () => callback(),
        (error) => callback(opError(error)),
      );
    }

    _destroy(error, callback) {
      const failure = error === undefined ? null : error;
      for (const pending of this.#pendingWrites.splice(0)) {
        pending.callback(failure);
      }
      if (this.#pendingFinal) {
        const final = this.#pendingFinal;
        this.#pendingFinal = null;
        final(failure);
      }
      this.#release(failure ? constants.NGHTTP2_CANCEL : 0);
      callback(failure);
    }

    #release(reason) {
      if (this.#id === 0) return;
      const id = this.#id;
      this.#id = 0;
      Deno.core.ops.op_http2_reset_stream(id, reason);
    }
    close(code, callback) {
      if (typeof code === "function") {
        callback = code;
        code = constants.NGHTTP2_NO_ERROR;
      }
      if (!this.#localClosed) {
        this.#localClosed = true;
        this.rstCode = code === undefined ? constants.NGHTTP2_CANCEL : code;
        this.#release(this.rstCode);
        this.destroy();
      }
      if (typeof callback === "function") process.nextTick(callback);
    }

    #pump() {
      if (this.#pumping || this.#id === 0) return;
      this.#pumping = true;
      this.#run();
    }

    async #run() {
      while (this.#id !== 0) {
        let event;
        try {
          event = await Deno.core.ops.op_http2_next_event(this.#id);
        } catch (error) {
          this.#pumping = false;
          this.destroy(opError(error));
          return;
        }
        if (event === null || !this.#handle(event)) {
          this.#pumping = false;
          return;
        }
      }
      this.#pumping = false;
    }

    #handle(event) {
      if (event.type === "response") {
        const { endStream, status, headers } = event.value;
        const flags = endStream ? 1 : 0;
        const decoded = decodeHeaders(headers);
        decoded[":status"] = String(status);
        this.emit("headers", decoded, flags);
        this.emit("response", decoded, flags);
      } else if (event.type === "data") {
        return this.push(event.value);
      } else if (event.type === "trailers") {
        this.emit("trailers", decodeHeaders(event.value), 0);
      } else if (event.type === "end") {
        this.push(null);
        this.read(0);
      } else if (event.type === "close") {
        const { reason } = event.value;
        if (!this.#localClosed) this.rstCode = reason;
        if (reason !== 0 && !this.#localClosed) {
          // Mirror the server twin: a peer-sent RST ends the readable side,
          // settles a pending end(), and destroys the stream.
          this.aborted = true;
          this.push(null);
          this.destroy();
        }
        return false;
      } else if (event.type === "error") {
        this.destroy(http2Error("ERR_HTTP2_STREAM_ERROR", event.value));
        return false;
      }
      return true;
    }
  }

  const goawayArguments = (code, lastStreamID, opaqueData) => {
    if (typeof code !== "number") {
      throw new codes.ERR_INVALID_ARG_TYPE("code", "number", code);
    }
    if (lastStreamID !== undefined && typeof lastStreamID !== "number") {
      throw new codes.ERR_INVALID_ARG_TYPE("lastStreamID", "number", lastStreamID);
    }
    let opaque = new Uint8Array(0);
    if (opaqueData !== undefined) {
      if (typeof opaqueData === "string" || ArrayBuffer.isView(opaqueData) ||
          opaqueData instanceof ArrayBuffer) {
        opaque = Buffer.from(opaqueData);
      } else {
        throw new codes.ERR_INVALID_ARG_TYPE(
          "opaqueData",
          "Buffer, TypedArray, DataView, or ArrayBuffer",
          opaqueData,
        );
      }
    }
    return { code, last: lastStreamID ?? 0, opaque };
  };

  const sendGoaway = (sessionId, closed, { code, last, opaque }) => {
    if (closed) {
      throw http2Error("ERR_HTTP2_INVALID_SESSION", "The session has been destroyed");
    }
    Deno.core.ops.op_http2_goaway(sessionId, code, last, opaque);
  };

  class ClientHttp2Session extends EventEmitter {
    #id = 0;
    #closed = false;
    #destroyed = false;
    #pumping = false;
    #streams = new Set();
    #queued = new Set();
    #pendingGoaway = [];

    constructor(authority, options) {
      super();
      this.authority = authority;
      this.options = options || {};
      this.socket = null;
      this.encrypted = false;
      this.type = constants.NGHTTP2_SESSION_CLIENT;
      this.alpnProtocol = false;
      this.connecting = true;
      this.closed = false;
      this.destroyed = false;
      this.pendingSettingsAck = false;
      this.state = {
        localWindowSize: DEFAULT_SETTINGS.initialWindowSize,
        remoteWindowSize: DEFAULT_SETTINGS.initialWindowSize,
        effectiveLocalWindowSize: DEFAULT_SETTINGS.initialWindowSize,
        effectiveRecvDataLength: 0,
        nextStreamID: 1,
        lastProcStreamID: 0,
        deflateDynamicTableSize: DEFAULT_SETTINGS.headerTableSize,
        inflateDynamicTableSize: DEFAULT_SETTINGS.headerTableSize,
        remoteSettings: { ...DEFAULT_SETTINGS },
      };
    }

    get thinpackSessionId() {
      return this.#id;
    }

    #socket() {
      const creator = this.options.createConnection;
      if (typeof creator === "function") {
        const socket = creator(this.authority, this.options);
        if (socket instanceof net.Socket) {
          return Promise.resolve({ socket, tls: false });
        }
        if (socket && socket.encrypted === true && typeof socket._thinpackReleaseTls === "function") {
          return Promise.resolve({ socket, tls: true });
        }
        return Promise.reject(notImplemented("http2.connect with a non-thinpack socket"));
      }
      if (this.secure) {
        return Promise.reject(notImplemented("http2.connect with https: without createConnection"));
      }
      return new Promise((resolve, reject) => {
        const socket = net.connect({ host: this.host, port: this.port });
        socket.once("connect", () => resolve({ socket, tls: false }));
        socket.once("error", reject);
      });
    }
    open() {
      this.#socket()
        .then(async ({ socket, tls }) => {
          this.socket = socket;
          this.encrypted = tls;
          this.alpnProtocol = tls ? "h2" : false;
          let id;
          if (tls) {
            await socket._thinpackReleaseTls();
            id = await Deno.core.ops.op_http2_connect_tls(socket._thinpackTlsId);
          } else {
            socket._thinpackUpgraded = true;
            await socket._thinpackReleaseSocket();
            id = await Deno.core.ops.op_http2_connect(socket._thinpackConnectionId);
          }
          if (this.#destroyed) {
            Deno.core.ops.op_http2_close_session(id);
            return;
          }
          this.#id = id;
          this.connecting = false;
          for (const args of this.#pendingGoaway) sendGoaway(id, false, args);
          this.#pendingGoaway.length = 0;
          for (const stream of this.#queued) stream.thinpackOpen(id);
          this.#queued.clear();
          this.#pump();
          this.emit("connect", this, socket);
          // The ops layer never surfaces the peer's SETTINGS frame; not emitting
          // "remoteSettings" is honest, broadcasting defaults would not be.
        })
        .catch((error) => {
          const failure = opError(error);
          for (const stream of this.#queued) stream.destroy(failure);
          this.#queued.clear();
          this.destroy(failure);
        });
      return this;
    }

    request(headers, options = {}) {
      if (this.#closed) {
        throw http2Error("ERR_HTTP2_INVALID_SESSION", "The session has been destroyed");
      }
      const filled = { ...headers };
      if (filled[":scheme"] === undefined) filled[":scheme"] = this.secure ? "https" : "http";
      if (filled[":authority"] === undefined) filled[":authority"] = this.authority;
      if (filled[":path"] === undefined) filled[":path"] = "/";
      const stream = new ClientHttp2Stream(this, filled, options);
      this.#streams.add(stream);
      stream.once("close", () => this.#streams.delete(stream));
      if (this.#id === 0) this.#queued.add(stream);
      else stream.thinpackOpen(this.#id);
      return stream;
    }

    close(callback) {
      this.#close();
      if (typeof callback === "function") process.nextTick(callback);
    }

    destroy(error, code) {
      if (error) process.nextTick(() => this.emit("error", error));
      this.#close();
      return this;
    }

    #close() {
      if (this.#closed) return;
      this.#closed = true;
      this.closed = true;
      this.destroyed = true;
      if (this.#id !== 0) {
        Deno.core.ops.op_http2_close_session(this.#id);
        this.#id = 0;
      }
      for (const stream of this.#queued) stream.destroy();
      this.#queued.clear();
      for (const stream of this.#streams) stream.destroy();
      this.#streams.clear();
      process.nextTick(() => this.emit("close"));
    }

    ping(callback, payload) {
      if (this.#id === 0) {
        if (this.#closed) {
          if (typeof callback === "function") process.nextTick(callback, http2Error("ERR_HTTP2_INVALID_SESSION", "session is closed"));
          return;
        }
        this.once("connect", () => this.ping(callback, payload));
        return;
      }
      const start = Date.now();
      Deno.core.ops
        .op_http2_ping(this.#id)
        .then(() => {
          if (typeof callback === "function") {
            process.nextTick(callback, null, Date.now() - start, payload ?? Buffer.alloc(8));
          }
        })
        .catch((error) => {
          if (typeof callback === "function") process.nextTick(callback, error);
          else this.destroy(error);
        });
    }
    settings(settings, callback) {
      if (typeof settings === "function") callback = settings;
      else if (settings !== undefined) {
        // The ops layer cannot send a SETTINGS frame; refuse to pretend one was applied.
        throw notImplemented("http2 session.settings with a settings object");
      }
      if (typeof callback === "function") process.nextTick(callback);
    }

    setLocalWindowSize() {
      return this;
    }

    incrementWindowSize() {
      return this;
    }

    setTimeout() {
      return this;
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
    goaway(code = constants.NGHTTP2_NO_ERROR, lastStreamID = 0, opaqueData) {
      const args = goawayArguments(code, lastStreamID, opaqueData);
      if (this.#closed) {
        throw http2Error("ERR_HTTP2_INVALID_SESSION", "The session has been destroyed");
      }
      if (this.#id === 0) {
        this.#pendingGoaway.push(args);
        return;
      }
      sendGoaway(this.#id, false, args);
    }
    #pump() {
      if (this.#pumping || this.#closed || this.#id === 0) return;
      this.#pumping = true;
      this.#run();
    }

    async #run() {
      while (!this.#closed) {
        let event;
        try {
          event = await Deno.core.ops.op_http2_session_event(this.#id);
        } catch (error) {
          this.destroy(opError(error));
          return;
        }
        if (event === null) break;
        if (!this.#handle(event)) break;
      }
      this.#pumping = false;
      this.#close();
    }

    #handle(event) {
      if (event.type === "goaway") {
        const { code, lastStreamId, opaque } = event.value;
        this.emit(
          "goaway",
          code,
          lastStreamId,
          opaque === undefined ? undefined : Buffer.from(opaque),
        );
        return true;
      }
      return false;
    }
  }

  const connect = (authority, options, listener) => {
    if (typeof options === "function") {
      listener = options;
      options = undefined;
    }
    if (typeof authority !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("authority", "string", authority);
    }
    const url = new URL(authority);
    const secure = url.protocol === "https:";
    if (!secure && url.protocol !== "http:") {
      throw notImplemented("http2.connect with " + url.protocol);
    }
    const session = new ClientHttp2Session(url.host, options);
    session.secure = secure;
    session.host = url.hostname;
    session.port = url.port === "" ? (secure ? 443 : 80) : Number(url.port);
    if (typeof listener === "function") session.once("connect", listener);
    return session.open();
  };
  class ServerHttp2Stream extends Duplex {
    #id = 0;
    #pumping = false;
    #localClosed = false;
    #sentHeaders = false;
    #endSent = false;
    #waitForTrailers = false;
    #pendingFinal = null;

    constructor(session, id) {
      super({ autoDestroy: true, allowHalfOpen: true });
      this.#id = id;
      this.session = session;
      this.rstCode = constants.NGHTTP2_NO_ERROR;
      this.aborted = false;
      this.pending = false;
    }

    get sentHeaders() {
      return this.#sentHeaders;
    }
    respond(headers, options = {}) {
      if (this.#sentHeaders) return false;
      this.#sentHeaders = true;
      this.#waitForTrailers = options.waitForTrailers === true;
      const endStream = options.endStream === true;
      if (endStream) this.#endSent = true;
      Deno.core.ops.op_http2_respond(this.#id, headerEntries(headers), endStream).then(
        () => {
          if (endStream) this.end();
        },
        (error) => this.destroy(opError(error)),
      );
      return true;
    }
    sendTrailers(trailers) {
      Deno.core.ops.op_http2_send_trailers(this.#id, headerEntries(trailers)).then(
        () => {
          this.#endSent = true;
          const final = this.#pendingFinal;
          this.#pendingFinal = null;
          if (final) final();
        },
        (error) => this.destroy(opError(error)),
      );
    }

    _read() {
      this.#pump();
    }

    _write(chunk, encoding, callback) {
      if (!this.#sentHeaders) {
        callback(http2Error("ERR_HTTP2_INVALID_STREAM", "respond() must be called before writing"));
        return;
      }
      Deno.core.ops.op_http2_write(this.#id, chunk, false).then(
        () => callback(),
        (error) => callback(opError(error)),
      );
    }

    _final(callback) {
      if (this.#endSent) {
        callback();
        return;
      }
      if (this.#waitForTrailers) {
        this.#pendingFinal = callback;
        process.nextTick(() => this.emit("wantTrailers"));
        return;
      }
      this.#endSent = true;
      Deno.core.ops.op_http2_write(this.#id, new Uint8Array(0), true).then(
        () => callback(),
        (error) => callback(opError(error)),
      );
    }

    _destroy(error, callback) {
      const failure = error === undefined ? null : error;
      const final = this.#pendingFinal;
      this.#pendingFinal = null;
      if (final) final(failure);
      this.#release(failure ? constants.NGHTTP2_CANCEL : 0);
      callback(failure);
    }

    #release(reason) {
      if (this.#localClosed) return;
      this.#localClosed = true;
      Deno.core.ops.op_http2_reset_stream(this.#id, reason);
    }

    close(code, callback) {
      if (typeof code === "function") {
        callback = code;
        code = undefined;
      }
      if (!this.#localClosed) {
        this.rstCode = code === undefined ? constants.NGHTTP2_NO_ERROR : code;
        this.#release(this.rstCode);
        this.destroy();
      }
      if (typeof callback === "function") process.nextTick(callback);
    }

    priority() {
      return undefined;
    }

    setTimeout() {
      return this;
    }

    #pump() {
      if (this.#pumping) return;
      this.#pumping = true;
      this.#run();
    }

    async #run() {
      while (!this.#localClosed) {
        let event;
        try {
          event = await Deno.core.ops.op_http2_next_event(this.#id);
        } catch (error) {
          this.#pumping = false;
          this.destroy(opError(error));
          return;
        }
        if (event === null || !this.#handle(event)) break;
      }
      this.#pumping = false;
    }

    #handle(event) {
      if (event.type === "data") {
        return this.push(event.value);
      }
      if (event.type === "trailers") {
        this.emit("trailers", decodeHeaders(event.value), 0);
      } else if (event.type === "end") {
        this.push(null);
        this.read(0);
      } else if (event.type === "close") {
        const { reason } = event.value;
        if (!this.#localClosed) this.rstCode = reason;
        if (reason !== 0 && !this.#localClosed) {
          this.destroy();
          return false;
        }
        return false;
      } else if (event.type === "error") {
        this.destroy(http2Error("ERR_HTTP2_STREAM_ERROR", event.value));
        return false;
      }
      return true;
    }
  }
  class ServerHttp2Session extends EventEmitter {
    #id = 0;
    #closed = false;
    #pumping = false;
    #server;

    constructor(server, id) {
      super();
      this.#id = id;
      this.#server = server;
      this.type = constants.NGHTTP2_SESSION_SERVER;
      this.encrypted = server.encrypted;
      this.alpnProtocol = server.encrypted ? "h2" : false;
      this.connecting = false;
      this.closed = false;
      this.destroyed = false;
      this.pendingSettingsAck = false;
      // Only server sessions record the accepted socket's addresses.
      const info = Deno.core.ops.op_http2_session_info(id);
      const local = parseAddress(info.local);
      const remote = parseAddress(info.remote);
      this.socket = {
        localAddress: local.address,
        localPort: local.port,
        remoteAddress: remote.address,
        remotePort: remote.port,
      };
    }
    _start() {
      if (this.#closed) return;
      this.#pump();
    }

    close(callback) {
      this.#close();
      if (typeof callback === "function") process.nextTick(callback);
    }

    destroy(error) {
      if (error) process.nextTick(() => this.emit("error", error));
      this.#close();
      return this;
    }

    ping(callback) {
      const start = Date.now();
      Deno.core.ops
        .op_http2_ping(this.#id)
        .then(() => {
          if (typeof callback === "function") {
            process.nextTick(callback, null, Date.now() - start, Buffer.alloc(8));
          }
        })
        .catch((error) => {
          if (typeof callback === "function") {
            process.nextTick(callback, error);
          } else {
            this.destroy(error);
          }
        });
    }

    settings(settings, callback) {
      if (typeof settings === "function") callback = settings;
      else if (settings !== undefined) {
        // The ops layer cannot send a SETTINGS frame; refuse to pretend one was applied.
        throw notImplemented("http2 session.settings with a settings object");
      }
      if (typeof callback === "function") process.nextTick(callback);
    }

    setTimeout() {
      return this;
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
    goaway(code = constants.NGHTTP2_NO_ERROR, lastStreamID = 0, opaqueData) {
      sendGoaway(
        this.#id,
        this.#closed,
        goawayArguments(code, lastStreamID, opaqueData),
      );
    }

    #close() {
      if (this.#closed) return;
      this.#closed = true;
      this.closed = true;
      this.destroyed = true;
      if (this.#id !== 0) {
        Deno.core.ops.op_http2_close_session(this.#id);
        this.#id = 0;
      }
      process.nextTick(() => this.emit("close"));
    }

    #pump() {
      if (this.#pumping || this.#closed) return;
      this.#pumping = true;
      this.#run();
    }

    async #run() {
      while (!this.#closed) {
        let event;
        try {
          event = await Deno.core.ops.op_http2_session_event(this.#id);
        } catch (error) {
          this.destroy(opError(error));
          return;
        }
        if (event === null) break;
        if (!this.#handle(event)) break;
      }
      this.#pumping = false;
      this.#close();
    }

    #handle(event) {
      if (event.type === "stream") {
        const { id, headers } = event.value;
        const stream = new ServerHttp2Stream(this, id);
        this.#server.emit("stream", stream, decodeHeaders(headers));
        return true;
      }
      if (event.type === "goaway") {
        const { code, lastStreamId, opaque } = event.value;
        this.emit(
          "goaway",
          code,
          lastStreamId,
          opaque === undefined ? undefined : Buffer.from(opaque),
        );
        return true;
      }
      if (event.type === "error") {
        this.emit("error", http2Error("ERR_HTTP2_SESSION_ERROR", event.value));
        return true;
      }
      return false;
    }
  }

  class Http2Server extends EventEmitter {
    #listenerId = 0;
    #closed = false;
    #local = "";
    #options;

    constructor(options, listener) {
      super();
      if (typeof options === "function") {
        listener = options;
        options = {};
      }
      this.#options = options ?? {};
      this.listening = false;
      this.encrypted = false;
      this.maxSessions = 0;
      if (listener) this.on("stream", listener);
    }

    _thinpackListen(host, port) {
      return Deno.core.ops.op_http2_listen(host, port);
    }
    _thinpackSetSecureContext(cert, key, ca, requestCert) {
      if (this.#listenerId === 0) return;
      Deno.core.ops.op_http2_set_secure_context(this.#listenerId, cert, key, ca, requestCert);
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
      if (this.#listenerId !== 0) throw http2Error("ERR_SERVER_ALREADY_LISTEN", "Listen method has been called more than once");

      const port = Number(options?.port ?? 0);
      const host = options?.host ?? "0.0.0.0";
      this._thinpackListen(String(host), port)
        .then((listener) => {
          this.#listenerId = listener.id;
          this.#local = listener.local;
          this.listening = true;
          this.emit("listening");
          if (callback) callback();
          this.#accept();
        })
        .catch((error) => this.emit("error", serverError(error, "listen", `${host}:${port}`)));
      return this;
    }

    async #accept() {
      while (!this.#closed && this.#listenerId !== 0) {
        let id;
        try {
          id = await Deno.core.ops.op_http2_accept_session(this.#listenerId);
        } catch (error) {
          const failure = opError(error);
          if (this.#closed || failure.code === "ECANCELED" || failure.code === "EBADF") break;
          if (this.encrypted) this.emit("tlsClientError", failure);
          else this.emit("sessionError", failure, null);
          continue;
        }
        if (this.#closed) {
          Deno.core.ops.op_http2_close_session(id);
          break;
        }
        const session = new ServerHttp2Session(this, id);
        this.emit("session", session);
        session._start();
      }
    }

    address() {
      if (this.#local === "") return null;
      return parseAddress(this.#local);
    }

    close(callback) {
      if (this.#closed) {
        if (callback) queueMicrotask(callback);
        return this;
      }
      this.#closed = true;
      this.listening = false;
      if (this.#listenerId !== 0) {
        Deno.core.ops.op_http2_listener_close(this.#listenerId);
        this.#listenerId = 0;
      }
      queueMicrotask(() => {
        this.emit("close");
        if (callback) callback();
      });
      return this;
    }

    setTimeout() {
      return this;
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
  }

  class Http2SecureServer extends Http2Server {
    #secureOptions;

    constructor(options, listener) {
      super(options, listener);
      this.#secureOptions = options ?? {};
      this.encrypted = true;
    }

    _thinpackListen(host, port) {
      const options = this.#secureOptions;
      return Deno.core.ops.op_http2_listen_secure(
        host,
        port,
        pem(options.cert),
        pem(options.key),
        pem(options.ca),
        Boolean(options.requestCert),
      );
    }
    setSecureContext(options = {}) {
      this.#secureOptions = {
        ...this.#secureOptions,
        ...options,
        cert: options.cert,
        key: options.key,
        ca: options.ca,
      };
      this._thinpackSetSecureContext(
        pem(options.cert),
        pem(options.key),
        pem(options.ca),
        Boolean(this.#secureOptions.requestCert),
      );
    }
  }

  const createServer = (options, onRequest) => {
    if (typeof options === "function") {
      onRequest = options;
      options = undefined;
    }
    return new Http2Server(options, onRequest);
  };

  const createSecureServer = (options, onRequest) => {
    if (typeof options === "function") {
      onRequest = options;
      options = undefined;
    }
    return new Http2SecureServer(options, onRequest);
  };

  module.exports = {
    constants,
    connect,
    createServer,
    createSecureServer,
    getDefaultSettings,
    getPackedSettings: () => {
      throw notImplemented("http2.getPackedSettings");
    },
    getUnpackedSettings: () => {
      throw notImplemented("http2.getUnpackedSettings");
    },
    sensitiveHeaders: Symbol("nodejs.http2.sensitiveHeaders"),
    Http2Session: ClientHttp2Session,
    ClientHttp2Session,
    ServerHttp2Session,
    Http2Server,
    Http2SecureServer,
    Http2Stream: ClientHttp2Stream,
    ClientHttp2Stream,
    ServerHttp2Stream,
  };
})();
