(() => {
  const types = require("node:thinpack/internal/binding/types");
  const { os: { signals } } = require("node:thinpack/internal/constants");
  const { codes } = require("node:thinpack/internal/errors");
  const { validateFunction, validateString } = require("node:thinpack/internal/validators");

  const kIsEncodingSymbol = Symbol("kIsEncodingSymbol");
  const encodings = [
    "ascii",
    "utf8",
    "base64",
    "utf16le",
    "latin1",
    "hex",
    "buffer",
    "base64url",
  ];
  const encodingsMap = { __proto__: null };
  for (let index = 0; index < encodings.length; index += 1) {
    encodingsMap[encodings[index]] = index;
  }
  const normalizeEncoding = (encoding) => {
    if (encoding == null || encoding === "utf8" || encoding === "utf-8") {
      return "utf8";
    }
    switch (String(encoding).toLowerCase()) {
      case "":
      case "utf8":
      case "utf-8":
        return "utf8";
      case "ucs2":
      case "ucs-2":
      case "utf16le":
      case "utf-16le":
        return "utf16le";
      case "ascii":
        return "ascii";
      case "latin1":
      case "binary":
        return "latin1";
      case "base64":
        return "base64";
      case "base64url":
        return "base64url";
      case "hex":
        return "hex";
      default:
        return undefined;
    }
  };
  const lazyDOMException = (message, name) => new globalThis.DOMException(message, name);

  const kEmptyObject = Object.freeze(Object.create(null));
  const SymbolDispose = Symbol.dispose ?? Symbol("Symbol.dispose");
  const customInspectSymbol = Symbol.for("nodejs.util.inspect.custom");
  const kCustomPromisifiedSymbol = Symbol.for("nodejs.util.promisify.custom");
  const kCustomPromisifyArgsSymbol = Symbol("customPromisifyArgs");
  const connectionRefused = ["ECONNREFUSED", "connection refused"];
  const systemErrors = new Map([
    [-7, ["E2BIG", "argument list too long"]],
    [-13, ["EACCES", "permission denied"]],
    [-98, ["EADDRINUSE", "address already in use"]],
    [-99, ["EADDRNOTAVAIL", "address not available"]],
    [-97, ["EAFNOSUPPORT", "address family not supported"]],
    [-11, ["EAGAIN", "resource temporarily unavailable"]],
    [-3000, ["EAI_ADDRFAMILY", "address family not supported"]],
    [-3001, ["EAI_AGAIN", "temporary failure"]],
    [-3002, ["EAI_BADFLAGS", "bad ai_flags value"]],
    [-3013, ["EAI_BADHINTS", "invalid value for hints"]],
    [-3003, ["EAI_CANCELED", "request canceled"]],
    [-3004, ["EAI_FAIL", "permanent failure"]],
    [-3005, ["EAI_FAMILY", "ai_family not supported"]],
    [-3006, ["EAI_MEMORY", "out of memory"]],
    [-3007, ["EAI_NODATA", "no address"]],
    [-3008, ["EAI_NONAME", "unknown node or service"]],
    [-3009, ["EAI_OVERFLOW", "argument buffer overflow"]],
    [-3014, ["EAI_PROTOCOL", "resolved protocol is unknown"]],
    [-3010, ["EAI_SERVICE", "service not available for socket type"]],
    [-3011, ["EAI_SOCKTYPE", "socket type not supported"]],
    [-114, ["EALREADY", "connection already in progress"]],
    [-9, ["EBADF", "bad file descriptor"]],
    [-16, ["EBUSY", "resource busy or locked"]],
    [-125, ["ECANCELED", "operation canceled"]],
    [-4080, ["ECHARSET", "invalid Unicode character"]],
    [-103, ["ECONNABORTED", "software caused connection abort"]],
    [-111, connectionRefused],
    [-104, ["ECONNRESET", "connection reset by peer"]],
    [-89, ["EDESTADDRREQ", "destination address required"]],
    [-17, ["EEXIST", "file already exists"]],
    [-14, ["EFAULT", "bad address in system call argument"]],
    [-27, ["EFBIG", "file too large"]],
    [-113, ["EHOSTUNREACH", "host is unreachable"]],
    [-4, ["EINTR", "interrupted system call"]],
    [-22, ["EINVAL", "invalid argument"]],
    [-5, ["EIO", "i/o error"]],
    [-106, ["EISCONN", "socket is already connected"]],
    [-21, ["EISDIR", "illegal operation on a directory"]],
    [-40, ["ELOOP", "too many symbolic links encountered"]],
    [-24, ["EMFILE", "too many open files"]],
    [-90, ["EMSGSIZE", "message too long"]],
    [-36, ["ENAMETOOLONG", "name too long"]],
    [-100, ["ENETDOWN", "network is down"]],
    [-101, ["ENETUNREACH", "network is unreachable"]],
    [-23, ["ENFILE", "file table overflow"]],
    [-105, ["ENOBUFS", "no buffer space available"]],
    [-19, ["ENODEV", "no such device"]],
    [-2, ["ENOENT", "no such file or directory"]],
    [-12, ["ENOMEM", "not enough memory"]],
    [-64, ["ENONET", "machine is not on the network"]],
    [-92, ["ENOPROTOOPT", "protocol not available"]],
    [-28, ["ENOSPC", "no space left on device"]],
    [-38, ["ENOSYS", "function not implemented"]],
    [-107, ["ENOTCONN", "socket is not connected"]],
    [-20, ["ENOTDIR", "not a directory"]],
    [-39, ["ENOTEMPTY", "directory not empty"]],
    [-88, ["ENOTSOCK", "socket operation on non-socket"]],
    [-95, ["ENOTSUP", "operation not supported on socket"]],
    [-75, ["EOVERFLOW", "value too large for defined data type"]],
    [-1, ["EPERM", "operation not permitted"]],
    [-32, ["EPIPE", "broken pipe"]],
    [-71, ["EPROTO", "protocol error"]],
    [-93, ["EPROTONOSUPPORT", "protocol not supported"]],
    [-91, ["EPROTOTYPE", "protocol wrong type for socket"]],
    [-34, ["ERANGE", "result too large"]],
    [-30, ["EROFS", "read-only file system"]],
    [-108, ["ESHUTDOWN", "cannot send after transport endpoint shutdown"]],
    [-29, ["ESPIPE", "invalid seek"]],
    [-3, ["ESRCH", "no such process"]],
    [-110, ["ETIMEDOUT", "connection timed out"]],
    [-26, ["ETXTBSY", "text file is busy"]],
    [-18, ["EXDEV", "cross-device link not permitted"]],
    [-4094, ["UNKNOWN", "unknown error"]],
    [-4095, ["EOF", "end of file"]],
    [-6, ["ENXIO", "no such device or address"]],
    [-31, ["EMLINK", "too many links"]],
    [-112, ["EHOSTDOWN", "host is down"]],
    [-121, ["EREMOTEIO", "remote I/O error"]],
    [-25, ["ENOTTY", "inappropriate ioctl for device"]],
    [-4028, ["EFTYPE", "inappropriate file type or format"]],
    [-84, ["EILSEQ", "illegal byte sequence"]],
    [-94, ["ESOCKTNOSUPPORT", "socket type not supported"]],
    [-61, ["ENODATA", "no data available"]],
    [-49, ["EUNATCH", "protocol driver not attached"]],
    [-8, ["ENOEXEC", "exec format error"]],
  ]);
  // The table holds Linux errno values. macOS and Windows use libuv's platform
  // values for EMSGSIZE and ECONNREFUSED, so project those entries to the same
  // code and message instead of duplicating the complete platform tables.
  // Provenance: pinned Deno 894c3bb ext/node/polyfills/internal_binding/uv.ts.
  if (process.platform === "win32") {
    systemErrors.delete(-111);
    systemErrors.set(-4078, connectionRefused);
    systemErrors.set(-4065, ["EMSGSIZE", "message too long"]);
    // libuv's Windows UV_ENOENT, which uv_compat reports for a missing host path.
    systemErrors.set(-4058, ["ENOENT", "no such file or directory"]);
  } else if (process.platform === "darwin") {
    systemErrors.delete(-111);
    systemErrors.set(-61, connectionRefused);
    systemErrors.set(-40, ["EMSGSIZE", "message too long"]);
  }
  const once = (callback, { preserveReturnValue = false } = kEmptyObject) => {
    let called = false;
    let returnValue;
    return function (...args) {
      if (called) return returnValue;
      called = true;
      const result = Reflect.apply(callback, this, args);
      returnValue = preserveReturnValue ? result : undefined;
      return result;
    };
  };
  const setOwnProperty = (obj, key, value) => {
    Object.defineProperty(obj, key, {
      __proto__: null,
      configurable: true,
      enumerable: true,
      value,
      writable: true,
    });
    return value;
  };
  const RegExpPrototypeExec = Function.prototype.call.bind(RegExp.prototype.exec);
  const SideEffectFreeRegExpPrototypeExec = (regex, string) =>
    RegExpPrototypeExec(regex, string);

  const spliceOne = (list, index) => {
    for (; index + 1 < list.length; index += 1) list[index] = list[index + 1];
    list.pop();
  };

  const getLazy = (loader) => {
    let value;
    let loaded = false;
    return () => {
      if (!loaded) {
        value = loader();
        loaded = true;
      }
      return value;
    };
  };

  const isWindows = process.platform === "win32";
  const isMacOS = process.platform === "darwin";

  const removeColors = (string) => string.replace(/\u001b\[\d\d?m/g, "");

  const join = (output, separator) => {
    let string = "";
    if (output.length !== 0) {
      const lastIndex = output.length - 1;
      for (let index = 0; index < lastIndex; index += 1) {
        string += output[index];
        string += separator;
      }
      string += output[lastIndex];
    }
    return string;
  };

  const isError = (value) => types.isNativeError(value) || value instanceof Error;

  // Each call hands out fresh tuples so rewriting a returned entry cannot
  // change later results or the shared table.
  const getSystemErrorMap = () => {
    const map = new Map();
    for (const [code, [name, message]] of systemErrors) {
      map.set(code, [name, message]);
    }
    return map;
  };

  const getSystemErrorName = (error) => {
    const entry = systemErrors.get(error);
    return entry ? entry[0] : "Unknown system error " + error;
  };

  const getSystemErrorMessage = (error) => {
    const entry = systemErrors.get(error);
    return entry ? entry[1] : "Unknown system error " + error;
  };

  const convertProcessSignalToExitCode = (signalCode) => {
    if (!Object.hasOwn(signals, signalCode)) {
      throw new codes.ERR_UNKNOWN_SIGNAL(signalCode);
    }
    return 128 + signals[signalCode];
  };
  const codesWarned = new Set();

  const getDeprecationWarningEmitter = (code, message) => {
    let warned = false;
    return () => {
      if (warned) return;
      warned = true;
      if (!process.noDeprecation) {
        process.emitWarning(message, "DeprecationWarning", code);
      }
    };
  };
  const deprecate = (fn, message, code, useEmitSync, modifyPrototype = true) => {
    if (code !== undefined) validateString(code, "code");

    const emitDeprecationWarning = getDeprecationWarningEmitter(code, message);
    const deprecated =
      function (...args) {
        if (code === undefined) {
          emitDeprecationWarning();
        } else if (!codesWarned.has(code)) {
          emitDeprecationWarning();
          codesWarned.add(code);
        }
        if (new.target) return Reflect.construct(fn, args, new.target);
        return Reflect.apply(fn, this, args);
      };

    if (modifyPrototype) {
      Object.setPrototypeOf(deprecated, fn);
      if (fn.prototype) deprecated.prototype = fn.prototype;
      Object.defineProperty(deprecated, "length", {
        __proto__: null,
        ...Object.getOwnPropertyDescriptor(fn, "length"),
      });
    }
    return deprecated;
  };

  const promisify = (original) => {
    validateFunction(original, "original");

    if (original[kCustomPromisifiedSymbol]) {
      const fn = original[kCustomPromisifiedSymbol];
      validateFunction(fn, "util.promisify.custom");
      return Object.defineProperty(fn, kCustomPromisifiedSymbol, {
        __proto__: null,
        value: fn,
        enumerable: false,
        writable: false,
        configurable: true,
      });
    }
    const argumentNames = original[kCustomPromisifyArgsSymbol];

    function fn(...args) {
      return new Promise((resolve, reject) => {
        args.push((error, ...values) => {
          if (error) return reject(error);
          if (argumentNames !== undefined && values.length > 1) {
            const result = {};
            for (let index = 0; index < argumentNames.length; index += 1) {
              result[argumentNames[index]] = values[index];
            }
            resolve(result);
          } else {
            resolve(values[0]);
          }
        });
        if (types.isPromise(Reflect.apply(original, this, args))) {
          process.emitWarning(
            "Calling promisify on a function that returns a Promise is " +
              "likely a mistake.",
            "DeprecationWarning",
            "DEP0174",
          );
        }
      });
    }

    Object.setPrototypeOf(fn, Object.getPrototypeOf(original));
    const descriptors = Object.getOwnPropertyDescriptors(original);
    for (const descriptor of Object.values(descriptors)) {
      Object.setPrototypeOf(descriptor, null);
    }
    return Object.defineProperties(fn, descriptors);
  };

  promisify.custom = kCustomPromisifiedSymbol;
  const defineLazyProperties = (target, id, keys, writable = true) => {
    let loaded;
    for (const key of keys) {
      let value;
      let assigned = false;
      Object.defineProperty(target, key, {
        __proto__: null,
        enumerable: true,
        configurable: true,
        get() {
          if (assigned) return value;
          loaded ??= require(id);
          value ??= loaded[key];
          return value;
        },
        set: writable
          ? (newValue) => {
            assigned = true;
            value = newValue;
          }
          : undefined,
      });
    }
  };

  const assignFunctionName = (name, fn, descriptor = {}) => {
    if (typeof name !== "string") {
      name = "[" + name.description + "]";
    }
    return Object.defineProperty(fn, "name", {
      __proto__: null,
      writable: false,
      enumerable: false,
      configurable: true,
      ...Object.getOwnPropertyDescriptor(fn, "name"),
      ...descriptor,
      value: name,
    });
  };

  const kEnumerableProperty = { __proto__: null, enumerable: true };

  class WeakReference {
    #ref;
    constructor(value) {
      this.#ref = new WeakRef(value);
    }
    get() {
      return this.#ref.deref();
    }
    set(value) {
      this.#ref = new WeakRef(value);
    }
    incRef() {
      this.refs = (this.refs ?? 0) + 1;
    }
    decRef() {
      this.refs = (this.refs ?? 1) - 1;
    }
  }

  const SideEffectFreeRegExpPrototypeSymbolReplace = (regexp, string, replaceValue) =>
    String.prototype.replace.call(string, regexp, replaceValue);

  module.exports = {
    SymbolDispose,
    kEnumerableProperty,
    getDeprecationWarningEmitter,
    WeakReference,
    SideEffectFreeRegExpPrototypeSymbolReplace,
    assignFunctionName,
    encodingsMap,
    kIsEncodingSymbol,
    lazyDOMException,
    normalizeEncoding,
    convertProcessSignalToExitCode,
    customInspectSymbol,
    defineLazyProperties,
    deprecate,
    getLazy,
    once,
    SideEffectFreeRegExpPrototypeExec,
    setOwnProperty,
    getSystemErrorMap,
    getSystemErrorMessage,
    getSystemErrorName,
    isError,
    isMacOS,
    isWindows,
    join,
    kEmptyObject,
    promisify,
    removeColors,
    spliceOne,
  };
})();
