(() => {
  const { ops } = Deno.core;
  const HREF = 0;
  const PROTOCOL = 1;
  const USERNAME = 2;
  const PASSWORD = 3;
  const HOST = 4;
  const HOSTNAME = 5;
  const PORT = 6;
  const PATHNAME = 7;
  const SEARCH = 8;
  const HASH = 9;
  const ORIGIN = 10;

  const IteratorPrototype = Object.getPrototypeOf(
    Object.getPrototypeOf([][Symbol.iterator]()),
  );
  const notImplemented = (feature) => {
    const error = new Error(feature + " is not implemented yet");
    error.code = "ERR_NOT_IMPLEMENTED";
    return error;
  };

  const invalidArgumentType = (name, expected, actual) => {
    const type = actual === null ? "null" : typeof actual;
    const error = new TypeError(
      'The "' + name + '" argument must be of type ' + expected +
        ". Received type " + type,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    return error;
  };

  const missingArgs = (...names) => {
    const list = names.map((name) => '"' + name + '"').join(" and ");
    const error = new TypeError(
      "The " + list + (names.length === 1 ? " argument" : " arguments") +
        " must be specified",
    );
    error.code = "ERR_MISSING_ARGS";
    return error;
  };

  const invalidUrl = (input, base) => {
    const error = new TypeError("Invalid URL");
    error.code = "ERR_INVALID_URL";
    error.input = input;
    if (base !== undefined) error.base = base;
    return error;
  };

  const invalidTuple = () => {
    const error = new TypeError(
      "Each query pair must be an iterable [name, value] tuple",
    );
    error.code = "ERR_INVALID_TUPLE";
    return error;
  };

  const notIterable = (name) => {
    const error = new TypeError('The argument "' + name + '" is not iterable');
    error.code = "ERR_ARG_NOT_ITERABLE";
    return error;
  };
  const isHexDigit = (code) =>
    (code >= 0x30 && code <= 0x39) ||
    (code >= 0x41 && code <= 0x46) ||
    (code >= 0x61 && code <= 0x66);

  const REPLACEMENT = "\uFFFD";
  const utf8Decode = (bytes) => {
    let result = "";
    let needed = 0;
    let seen = 0;
    let codePoint = 0;
    let lower = 0x80;
    let upper = 0xbf;

    for (let index = 0; index < bytes.length; index += 1) {
      const byte = bytes[index];
      if (needed === 0) {
        if (byte <= 0x7f) {
          result += String.fromCharCode(byte);
        } else if (byte >= 0xc2 && byte <= 0xdf) {
          needed = 1;
          codePoint = byte & 0x1f;
        } else if (byte >= 0xe0 && byte <= 0xef) {
          if (byte === 0xe0) lower = 0xa0;
          else if (byte === 0xed) upper = 0x9f;
          needed = 2;
          codePoint = byte & 0x0f;
        } else if (byte >= 0xf0 && byte <= 0xf4) {
          if (byte === 0xf0) lower = 0x90;
          else if (byte === 0xf4) upper = 0x8f;
          needed = 3;
          codePoint = byte & 0x07;
        } else {
          result += REPLACEMENT;
        }
        continue;
      }
      if (byte < lower || byte > upper) {
        needed = 0;
        seen = 0;
        codePoint = 0;
        lower = 0x80;
        upper = 0xbf;
        result += REPLACEMENT;
        index -= 1;
        continue;
      }
      lower = 0x80;
      upper = 0xbf;
      codePoint = (codePoint << 6) | (byte & 0x3f);
      seen += 1;
      if (seen === needed) {
        result += String.fromCodePoint(codePoint);
        needed = 0;
        seen = 0;
        codePoint = 0;
      }
    }
    if (needed !== 0) result += REPLACEMENT;
    return result;
  };
  const percentDecode = (text) => {
    try {
      return decodeURIComponent(text);
    } catch {
      let result = "";
      let bytes = [];
      for (let index = 0; index < text.length; index += 1) {
        if (
          text.charCodeAt(index) === 0x25 &&
          isHexDigit(text.charCodeAt(index + 1)) &&
          isHexDigit(text.charCodeAt(index + 2))
        ) {
          bytes.push(parseInt(text.slice(index + 1, index + 3), 16));
          index += 2;
        } else {
          result += utf8Decode(bytes) + text[index];
          bytes = [];
        }
      }
      return result + utf8Decode(bytes);
    }
  };

  const hasPercentHex = (text) => {
    for (let index = 0; index + 2 < text.length; index += 1) {
      if (
        text.charCodeAt(index) === 0x25 &&
        isHexDigit(text.charCodeAt(index + 1)) &&
        isHexDigit(text.charCodeAt(index + 2))
      ) {
        return true;
      }
    }
    return false;
  };

  const decodeFormComponent = (text) => {
    if (text === "") return "";
    const spaced = text.indexOf("+") === -1 ? text : text.replace(/\+/g, " ");
    return hasPercentHex(spaced) ? percentDecode(spaced) : spaced;
  };
  const parseParams = (query) => {
    const list = [];
    let start = query.length > 0 && query[0] === "?" ? 1 : 0;
    while (start < query.length) {
      let end = query.indexOf("&", start);
      if (end === -1) end = query.length;
      if (end !== start) {
        const equals = query.indexOf("=", start);
        if (equals === -1 || equals > end) {
          list.push(decodeFormComponent(query.slice(start, end)), "");
        } else {
          list.push(
            decodeFormComponent(query.slice(start, equals)),
            decodeFormComponent(query.slice(equals + 1, end)),
          );
        }
      }
      start = end + 1;
    }
    return list;
  };

  const percentEscape = (character) =>
    "%" + character.charCodeAt(0).toString(16).toUpperCase();
  const encodeFormComponent = (value) =>
    encodeURIComponent(value).replace(/%20/g, "+").replace(/[!'()~]/g, percentEscape);

  const serializeParams = (list) => {
    const pairs = [];
    for (let index = 0; index < list.length; index += 2) {
      pairs.push(
        encodeFormComponent(list[index]) + "=" +
          encodeFormComponent(list[index + 1]),
      );
    }
    return pairs.join("&");
  };

  const toUSVString = (value) =>
    (typeof value === "string" ? value : `${value}`).toWellFormed();
  let setURLSearchParamsModified;
  let setURLSearchParamsContext;
  let getURLSearchParamsList;
  let setURLSearchParams;

  class URLSearchParamsIterator {
    #target;
    #kind;
    #index;

    constructor(target, kind) {
      this.#target = target;
      this.#kind = kind;
      this.#index = 0;
    }

    next() {
      const values = getURLSearchParamsList(this.#target);
      const index = this.#index;
      if (index >= values.length) {
        return { done: true, value: undefined };
      }
      this.#index = index + 2;
      if (this.#kind === "key") {
        return { done: false, value: values[index] };
      }
      if (this.#kind === "value") {
        return { done: false, value: values[index + 1] };
      }
      return { done: false, value: [values[index], values[index + 1]] };
    }
  }

  Object.setPrototypeOf(URLSearchParamsIterator.prototype, IteratorPrototype);

  class URLSearchParams {
    #searchParams = [];
    #context;
    #serialized;

    static {
      setURLSearchParamsContext = (params, context) => {
        params.#context = context;
      };
      getURLSearchParamsList = (params) => params.#searchParams;
      setURLSearchParams = (params, query) => {
        params.#serialized = undefined;
        params.#searchParams = query === undefined ? [] : parseParams(query);
      };
    }

    #markUpdated() {
      this.#serialized = undefined;
      if (this.#context !== undefined) setURLSearchParamsModified(this.#context);
    }
    constructor(init = undefined) {
      if (init === undefined) {
      } else if (
        init !== null &&
        (typeof init === "object" || typeof init === "function")
      ) {
        const method = init[Symbol.iterator];
        if (method === this[Symbol.iterator] && #searchParams in init) {
          this.#searchParams = init.#searchParams.slice();
          this.#serialized = init.#serialized;
        } else if (method != null) {
          if (typeof method !== "function") throw notIterable("Query pairs");
          for (const pair of init) {
            if (pair === null || pair === undefined) throw invalidTuple();
            if (Array.isArray(pair)) {
              if (pair.length !== 2) throw invalidTuple();
              this.#searchParams.push(
                toUSVString(pair[0]),
                toUSVString(pair[1]),
              );
            } else {
              if (
                (typeof pair !== "object" && typeof pair !== "function") ||
                typeof pair[Symbol.iterator] !== "function"
              ) {
                throw invalidTuple();
              }
              let length = 0;
              for (const element of pair) {
                length += 1;
                this.#searchParams.push(toUSVString(element));
              }
              if (length !== 2) throw invalidTuple();
            }
          }
        } else {
          const visited = new Map();
          for (const key of Reflect.ownKeys(init)) {
            const descriptor = Reflect.getOwnPropertyDescriptor(init, key);
            if (descriptor !== undefined && descriptor.enumerable) {
              const typedKey = toUSVString(key);
              const typedValue = toUSVString(init[key]);
              const existing = visited.get(typedKey);
              if (existing !== undefined) {
                this.#searchParams[existing] = typedValue;
              } else {
                visited.set(
                  typedKey,
                  this.#searchParams.push(typedKey, typedValue) - 1,
                );
              }
            }
          }
        }
      } else {
        init = toUSVString(init);
        this.#searchParams = init ? parseParams(init) : [];
      }
    }

    get size() {
      return this.#searchParams.length / 2;
    }

    append(name, value) {
      if (arguments.length < 2) throw missingArgs("name", "value");
      this.#searchParams.push(toUSVString(name), toUSVString(value));
      this.#markUpdated();
    }

    delete(name, value = undefined) {
      if (arguments.length < 1) throw missingArgs("name");
      const list = this.#searchParams;
      const typedName = toUSVString(name);
      const typedValue = value === undefined ? undefined : toUSVString(value);
      const kept = [];
      for (let index = 0; index < list.length; index += 2) {
        if (
          list[index] === typedName &&
          (typedValue === undefined || list[index + 1] === typedValue)
        ) {
          continue;
        }
        kept.push(list[index], list[index + 1]);
      }
      if (kept.length !== list.length) {
        this.#searchParams = kept;
        this.#serialized = undefined;
      }
      if (this.#context !== undefined) setURLSearchParamsModified(this.#context);
    }

    get(name) {
      if (arguments.length < 1) throw missingArgs("name");
      const list = this.#searchParams;
      const typedName = toUSVString(name);
      for (let index = 0; index < list.length; index += 2) {
        if (list[index] === typedName) return list[index + 1];
      }
      return null;
    }

    getAll(name) {
      if (arguments.length < 1) throw missingArgs("name");
      const list = this.#searchParams;
      const typedName = toUSVString(name);
      const values = [];
      for (let index = 0; index < list.length; index += 2) {
        if (list[index] === typedName) values.push(list[index + 1]);
      }
      return values;
    }

    has(name, value = undefined) {
      if (arguments.length < 1) throw missingArgs("name");
      const list = this.#searchParams;
      const typedName = toUSVString(name);
      const typedValue = value === undefined ? undefined : toUSVString(value);
      for (let index = 0; index < list.length; index += 2) {
        if (
          list[index] === typedName &&
          (typedValue === undefined || list[index + 1] === typedValue)
        ) {
          return true;
        }
      }
      return false;
    }

    set(name, value) {
      if (arguments.length < 2) throw missingArgs("name", "value");
      const list = this.#searchParams;
      const typedName = toUSVString(name);
      const typedValue = toUSVString(value);
      const kept = [];
      let found = false;
      for (let index = 0; index < list.length; index += 2) {
        if (list[index] !== typedName) {
          kept.push(list[index], list[index + 1]);
        } else if (!found) {
          kept.push(typedName, typedValue);
          found = true;
        }
      }
      if (!found) kept.push(typedName, typedValue);
      this.#searchParams = kept;
      this.#markUpdated();
    }

    sort() {
      const list = this.#searchParams;
      const pairs = [];
      for (let index = 0; index < list.length; index += 2) {
        pairs.push([list[index], list[index + 1]]);
      }
      pairs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      this.#searchParams = pairs.flat();
      this.#markUpdated();
    }

    entries() {
      return new URLSearchParamsIterator(this, "key+value");
    }

    forEach(callback, thisArg = undefined) {
      if (typeof callback !== "function") {
        throw invalidArgumentType("callback", "Function", callback);
      }
      let index = 0;
      while (index < this.#searchParams.length) {
        callback.call(
          thisArg,
          this.#searchParams[index + 1],
          this.#searchParams[index],
          this,
        );
        index += 2;
      }
    }

    keys() {
      return new URLSearchParamsIterator(this, "key");
    }

    values() {
      return new URLSearchParamsIterator(this, "value");
    }

    toString() {
      if (this.#serialized !== undefined) return this.#serialized;
      const serialized = serializeParams(this.#searchParams);
      this.#serialized = serialized;
      return serialized;
    }
  }

  Object.defineProperty(URLSearchParams.prototype, Symbol.toStringTag, {
    __proto__: null,
    configurable: true,
    value: "URLSearchParams",
  });
  Object.defineProperty(URLSearchParams.prototype, Symbol.iterator, {
    __proto__: null,
    configurable: true,
    writable: true,
    value: URLSearchParams.prototype.entries,
  });
  const components = (input, base) => {
    const source = typeof input === "string" ? input : `${input}`;
    const raw = ops.op_url_parse(
      source,
      base === undefined ? "" : `${base}`,
    );
    return raw === "" ? null : raw.split("\0");
  };

  class URL {
    #fields;
    #params;

    static {
      setURLSearchParamsModified = (url) => {
        const raw = ops.op_url_apply(
          url.#fields[HREF],
          "search",
          url.#params.toString(),
        );
        if (raw !== "") url.#fields = raw.split("\0");
      };
    }

    constructor(input, base = undefined) {
      if (arguments.length === 0) throw missingArgs("url");
      const fields = components(input, base);
      if (fields === null) throw invalidUrl(input, base);
      this.#fields = fields;
      this.#params = new URLSearchParams();
      setURLSearchParams(this.#params, fields[SEARCH].slice(1));
      setURLSearchParamsContext(this.#params, this);
    }
    #setPart(name, value) {
      const raw = ops.op_url_apply(this.#fields[HREF], name, `${value}`);
      if (raw === "") return false;
      this.#fields = raw.split("\0");
      return true;
    }
    #setQuery(name, value) {
      if (!this.#setPart(name, value)) return false;
      setURLSearchParams(this.#params, this.#fields[SEARCH].slice(1));
      return true;
    }

    get href() {
      return this.#fields[HREF];
    }

    set href(value) {
      if (!this.#setQuery("href", value)) throw invalidUrl(value);
    }

    get origin() {
      return this.#fields[ORIGIN];
    }

    get protocol() {
      return this.#fields[PROTOCOL];
    }

    set protocol(value) {
      this.#setPart("protocol", value);
    }

    get username() {
      return this.#fields[USERNAME];
    }

    set username(value) {
      this.#setPart("username", value);
    }

    get password() {
      return this.#fields[PASSWORD];
    }

    set password(value) {
      this.#setPart("password", value);
    }

    get host() {
      return this.#fields[HOST];
    }

    set host(value) {
      this.#setPart("host", value);
    }

    get hostname() {
      return this.#fields[HOSTNAME];
    }

    set hostname(value) {
      this.#setPart("hostname", value);
    }

    get port() {
      return this.#fields[PORT];
    }

    set port(value) {
      this.#setPart("port", value);
    }

    get pathname() {
      return this.#fields[PATHNAME];
    }

    set pathname(value) {
      this.#setPart("pathname", value);
    }

    get search() {
      return this.#fields[SEARCH];
    }

    set search(value) {
      this.#setQuery("search", value);
    }

    get searchParams() {
      return this.#params;
    }

    get hash() {
      return this.#fields[HASH];
    }

    set hash(value) {
      this.#setPart("hash", value);
    }

    toString() {
      return this.#fields[HREF];
    }

    toJSON() {
      return this.#fields[HREF];
    }

    static canParse(input, base = undefined) {
      if (arguments.length === 0) throw missingArgs("url");
      return components(input, base) !== null;
    }

    static parse(input, base = undefined) {
      if (arguments.length === 0) throw missingArgs("url");
      try {
        return new URL(input, base);
      } catch (error) {
        if (error.code === "ERR_INVALID_URL") return null;
        throw error;
      }
    }

    static createObjectURL() {
      throw notImplemented("URL.createObjectURL (Blob)");
    }

    static revokeObjectURL() {
      throw notImplemented("URL.revokeObjectURL (Blob)");
    }
  }

  Object.defineProperty(URL.prototype, Symbol.toStringTag, {
    __proto__: null,
    configurable: true,
    value: "URL",
  });
  const urlToHttpOptions = (url) => {
    if (url === null || typeof url !== "object") {
      throw invalidArgumentType("url", "Object", url);
    }
    const { hostname, pathname, port, username, password, search } = url;
    const options = {
      __proto__: null,
      ...url,
      protocol: url.protocol,
      hostname: hostname && hostname[0] === "[" ? hostname.slice(1, -1) : hostname,
      hash: url.hash,
      search,
      pathname,
      path: `${pathname || ""}${search || ""}`,
      href: url.href,
    };
    if (port !== "") options.port = Number(port);
    if (username || password) {
      options.auth = `${decodeURIComponent(username)}:${decodeURIComponent(password)}`;
    }
    return options;
  };

  const domainToASCII = function (domain) {
    if (arguments.length < 1) throw missingArgs("domain");
    return ops.op_url_domain_to_ascii(`${domain}`);
  };

  const domainToUnicode = function () {
    throw notImplemented("url.domainToUnicode (ICU)");
  };

  const pathToFileURL = function () {
    throw notImplemented("url.pathToFileURL");
  };

  const fileURLToPath = function () {
    throw notImplemented("url.fileURLToPath");
  };

  const fileURLToPathBuffer = function () {
    throw notImplemented("url.fileURLToPathBuffer");
  };
  class URLPattern {
    constructor() {
      throw notImplemented("URLPattern");
    }
  }
  const unsafeProtocol = new Set(["javascript", "javascript:"]);
  const hostlessProtocol = new Set(["javascript", "javascript:"]);
  const slashedProtocol = new Set([
    "http",
    "http:",
    "https",
    "https:",
    "ftp",
    "ftp:",
    "gopher",
    "gopher:",
    "file",
    "file:",
    "ws",
    "ws:",
    "wss",
    "wss:",
  ]);

  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;
  globalThis.__thinpack_url = {
    URL,
    URLPattern,
    URLSearchParams,
    domainToASCII,
    domainToUnicode,
    fileURLToPath,
    fileURLToPathBuffer,
    hostlessProtocol,
    pathToFileURL,
    slashedProtocol,
    unsafeProtocol,
    urlToHttpOptions,
  };
})();
