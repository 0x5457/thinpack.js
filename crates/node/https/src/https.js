(() => {
  const http = require("node:http");
  const tls = require("node:tls");

  const tlsConnect = (options) => tls.connect({
    port: options.port,
    host: options.host,
    ca: options.ca,
    cert: options.cert,
    key: options.key,
    servername: options.servername,
    rejectUnauthorized: options.rejectUnauthorized,
    ALPNProtocols: options.ALPNProtocols,
  });
  const withTls = (options) => {
    if (typeof options === "string" || options instanceof URL) {
      const url = new URL(String(options));
      return {
        protocol: url.protocol,
        hostname: url.hostname,
        port: url.port || undefined,
        path: url.pathname + url.search,
        method: "GET",
        secure: true,
        createConnection: tlsConnect,
      };
    }
    return { ...options, secure: true, createConnection: tlsConnect };
  };

  class Server extends http.Server {
    #secure;

    constructor(options, listener) {
      if (typeof options === "function") {
        listener = options;
        options = {};
      }
      super(options, listener);
      this.#secure = tls.createServer(options);
      this.#secure.on("secureConnection", (socket) => this.emit("connection", socket));
      this.#secure.on("tlsClientError", (error) => this.emit("tlsClientError", error));
      this.#secure.on("listening", () => this.emit("listening"));
      this.#secure.on("close", () => this.emit("close"));
    }

    listen(...args) {
      this.#secure.listen(...args);
      return this;
    }

    close(callback) {
      this.#secure.close(callback);
      return this;
    }

    address() {
      return this.#secure.address();
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
  }

  module.exports = {
    ...http,
    Agent: http.Agent,
    Server,
    globalAgent: new http.Agent({ keepAlive: true }),
    request: (options, callback) => http.request(withTls(options), callback),
    get: (options, callback) => http.get(withTls(options), callback),
    createServer: (options, listener) => new Server(options, listener),
  };
})();
