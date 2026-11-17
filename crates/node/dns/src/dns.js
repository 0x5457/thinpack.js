(() => {
  const { codes, opErrorParts } = require("node:thinpack/internal/errors");

  const notImplemented = (api) => new codes.ERR_NOT_IMPLEMENTED(api);

  const dnsError = (error, hostname) => {
    const errno = Number(error.message);
    if (!Number.isInteger(errno)) return error;
    const failure = new Error(
      `getaddrinfo ENOTFOUND ${hostname}`,
    );
    failure.errno = errno;
    failure.code = "ENOTFOUND";
    failure.syscall = "getaddrinfo";
    failure.hostname = hostname;
    return failure;
  };

  const normalizeOptions = (options) => {
    if (typeof options === "number") return { family: options };
    if (options == null) return {};
    return {
      family: options.family ?? 0,
      all: Boolean(options.all),
      hints: options.hints,
    };
  };

  const lookup = (hostname, options, callback) => {
    if (typeof options === "function") {
      callback = options;
      options = {};
    }
    if (typeof callback !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("callback", "Function", callback);
    }
    const { family = 0, all = false } = normalizeOptions(options);
    Deno.core.ops
      .op_dns_lookup(String(hostname), Number(family))
      .then((result) => {
        const fields = result.split("\0");
        const addresses = [];
        for (let index = 0; index < fields.length; index += 2) {
          addresses.push({ address: fields[index + 1], family: Number(fields[index]) });
        }
        if (all) {
          callback(null, addresses);
        } else {
          callback(null, addresses[0].address, addresses[0].family);
        }
      })
      .catch((error) => callback(dnsError(error, hostname)));
  };

  const queryError = (error, hostname, rrtype) => {
    const [code, detail] = opErrorParts(error);
    if (detail === undefined) return error;
    const failure = new Error(`query${rrtype} ${code} ${hostname}`);
    failure.code = code;
    failure.syscall = `query${rrtype}`;
    failure.hostname = String(hostname);
    failure.detail = detail;
    return failure;
  };

  const query = (servers, hostname, rrtype) =>
    Deno.core.ops
      .op_dns_resolve(servers, String(hostname), rrtype)
      .then((json) => JSON.parse(json), (error) => Promise.reject(queryError(error, hostname, rrtype)));

  const reverse = (servers, address) =>
    Deno.core.ops
      .op_dns_reverse(servers, String(address))
      .then((json) => JSON.parse(json), (error) => {
        const [code, detail] = opErrorParts(error);
        if (detail === undefined) return Promise.reject(error);
        const failure = new Error(`getHostByAddr ${code} ${address}`);
        failure.code = code;
        failure.syscall = "getHostByAddr";
        return Promise.reject(failure);
      });

  const resolveWith = (servers, hostname, rrtype, callback) => {
    const promise = query(servers, hostname, rrtype);
    if (typeof callback === "function") {
      promise.then((value) => callback(null, value), callback);
      return;
    }
    return promise;
  };
  class Resolver {
    #servers = [];

    setServers(servers) {
      if (!Array.isArray(servers)) {
        throw new codes.ERR_INVALID_ARG_TYPE("servers", ["Array"], servers);
      }
      this.#servers = servers.map(String);
    }

    getServers() {
      return [...this.#servers];
    }

    resolve(hostname, rrtype, callback) {
      return resolveWith(this.#servers.join(","), hostname, rrtype, callback);
    }

    resolve4(hostname, callback) {
      return resolveWith(this.#servers.join(","), hostname, "A", callback);
    }

    resolve6(hostname, callback) {
      return resolveWith(this.#servers.join(","), hostname, "AAAA", callback);
    }

    resolveCname(hostname, callback) {
      return resolveWith(this.#servers.join(","), hostname, "CNAME", callback);
    }

    resolveMx(hostname, callback) {
      return resolveWith(this.#servers.join(","), hostname, "MX", callback);
    }

    resolveNs(hostname, callback) {
      return resolveWith(this.#servers.join(","), hostname, "NS", callback);
    }

    resolvePtr(hostname, callback) {
      return resolveWith(this.#servers.join(","), hostname, "PTR", callback);
    }

    resolveTxt(hostname, callback) {
      return resolveWith(this.#servers.join(","), hostname, "TXT", callback);
    }

    reverse(address, callback) {
      const promise = reverse(this.#servers.join(","), address);
      if (typeof callback === "function") {
        promise.then((value) => callback(null, value), callback);
        return;
      }
      return promise;
    }

    resolveAny() {
      throw notImplemented("dns.Resolver.resolveAny");
    }
  }

  const promises = {
    Resolver,
    resolve: (hostname, rrtype) => query("", hostname, rrtype),
    resolve4: (hostname) => query("", hostname, "A"),
    resolve6: (hostname) => query("", hostname, "AAAA"),
    resolveCname: (hostname) => query("", hostname, "CNAME"),
    resolveMx: (hostname) => query("", hostname, "MX"),
    resolveNs: (hostname) => query("", hostname, "NS"),
    resolvePtr: (hostname) => query("", hostname, "PTR"),
    resolveTxt: (hostname) => query("", hostname, "TXT"),
    reverse: (address) => reverse("", address),
    lookup: (hostname, options) => {
      const { family = 0, all = false } = normalizeOptions(options);
      return Deno.core.ops
        .op_dns_lookup(String(hostname), Number(family))
        .then((result) => {
          const fields = result.split("\0");
          const addresses = [];
          for (let index = 0; index < fields.length; index += 2) {
            addresses.push({ address: fields[index + 1], family: Number(fields[index]) });
          }
          return all ? addresses : addresses[0];
        }, (error) => Promise.reject(dnsError(error, hostname)));
    },
  };

  const unsupported = (name) => {
    const entry = () => {
      throw notImplemented(`dns.${name}`);
    };
    return entry;
  };

  module.exports = {
    lookup,
    promises,
    setDefaultResultOrder() {},
    getDefaultResultOrder: () => "verbatim",
    Resolver,
    resolve: (hostname, rrtype, callback) => resolveWith("", hostname, rrtype, callback),
    resolve4: (hostname, callback) => resolveWith("", hostname, "A", callback),
    resolve6: (hostname, callback) => resolveWith("", hostname, "AAAA", callback),
    resolveCname: (hostname, callback) => resolveWith("", hostname, "CNAME", callback),
    resolveMx: (hostname, callback) => resolveWith("", hostname, "MX", callback),
    resolveNs: (hostname, callback) => resolveWith("", hostname, "NS", callback),
    resolvePtr: (hostname, callback) => resolveWith("", hostname, "PTR", callback),
    resolveTxt: (hostname, callback) => resolveWith("", hostname, "TXT", callback),
    reverse: (address, callback) => {
      const promise = reverse("", address);
      if (typeof callback === "function") {
        promise.then((value) => callback(null, value), callback);
        return;
      }
      return promise;
    },
    resolveAny: unsupported("resolveAny"),
    resolveSrv: unsupported("resolveSrv"),
    lookupService: unsupported("lookupService"),
  };
})();
