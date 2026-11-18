(() => {
  // Mirrors Node's `determineSpecificType`; error messages are part of the
  // observable API, and the upstream tests assert them verbatim.
  const describe = (value) => {
    if (value === null) return "null";
    if (value === undefined) return "undefined";

    const type = typeof value;
    switch (type) {
      case "bigint":
        return "type bigint (" + value + "n)";
      case "number":
        if (value === 0) return 1 / value === -Infinity ? "type number (-0)" : "type number (0)";
        if (value !== value) return "type number (NaN)";
        if (value === Infinity) return "type number (Infinity)";
        if (value === -Infinity) return "type number (-Infinity)";
        return "type number (" + value + ")";
      case "boolean":
        return value ? "type boolean (true)" : "type boolean (false)";
      case "symbol":
        return "type symbol (" + String(value) + ")";
      case "function":
        return "function " + value.name;
      case "object": {
        const name = value.constructor && value.constructor.name;
        if (typeof name === "string" && name !== "") {
          return "an instance of " + name;
        }
        return String(inspectValue(value, { depth: -1 }));
      }
      case "string": {
        if (value.length > 28) value = value.slice(0, 25) + "...";
        if (!value.includes("'")) return "type string ('" + value + "')";
        return "type string (" + JSON.stringify(value) + ")";
      }
      default: {
        let inspected = inspectValue(value);
        if (inspected.length > 28) inspected = inspected.slice(0, 25) + "...";
        return "type " + type + " (" + inspected + ")";
      }
    }
  };

  const TYPES = new Set([
    "string", "function", "number", "object", "Function", "Object",
    "boolean", "bigint", "symbol",
  ]);

  const formatList = (list, type) => {
    if (list.length === 0) return "";
    if (list.length === 1) return String(list[0]);
    if (list.length === 2) return list[0] + " " + type + " " + list[1];
    return list.slice(0, -1).join(", ") + ", " + type + " " + list[list.length - 1];
  };

  const formatExpected = (expected) => {
    const values = Array.isArray(expected) ? expected : [expected];
    const types = [];
    const instances = [];
    const other = [];
    for (const value of values) {
      if (TYPES.has(value)) {
        types.push(String(value).toLowerCase());
      } else if (/^[A-Z][a-zA-Z0-9]*$/.test(value)) {
        instances.push(value);
      } else {
        other.push(value);
      }
    }

    if (instances.length > 0) {
      const object = types.indexOf("object");
      if (object !== -1) {
        types.splice(object, 1);
        instances.push("Object");
      }
    }

    let message = "";
    if (types.length > 0) {
      message += (types.length > 1 ? "one of type " : "of type ") +
        formatList(types, "or");
      if (instances.length > 0 || other.length > 0) message += " or ";
    }
    if (instances.length > 0) {
      message += "an instance of " + formatList(instances, "or");
      if (other.length > 0) message += " or ";
    }
    if (other.length > 0) {
      if (other.length > 1) {
        message += "one of " + formatList(other, "or");
      } else {
        if (String(other[0]).toLowerCase() !== other[0]) message += "an ";
        message += String(other[0]);
      }
    }
    return message;
  };

  const inspectValue = (value, options) => {
    let inspected = require("node:thinpack/internal/util/inspect").inspect(value, options);
    if (inspected.length > 128) inspected = inspected.slice(0, 128) + "...";
    return inspected;
  };

  const inspectRaw = (value) =>
    require("node:thinpack/internal/util/inspect").inspect(value);

  class ERR_BUFFER_OUT_OF_BOUNDS extends RangeError {
    constructor(name) {
      super(
        name
          ? '"' + name + '" is outside of buffer bounds'
          : "Attempt to access memory outside buffer bounds",
      );
      this.code = "ERR_BUFFER_OUT_OF_BOUNDS";
    }
  }

  class ERR_INVALID_BUFFER_SIZE extends RangeError {
    constructor(size) {
      super("Buffer size must be a multiple of " + size);
      this.code = "ERR_INVALID_BUFFER_SIZE";
    }
  }

  class ERR_MISSING_ARGS extends TypeError {
    constructor(...names) {
      // Node's message shape: quoted names, "or" inside an alternative group,
      // commas and a final "and" between the rest.
      const wrap = (name) => '"' + name + '"';
      const list = names.map((name) =>
        Array.isArray(name) ? name.map(wrap).join(" or ") : wrap(name),
      );
      const joined =
        list.length <= 1
          ? list.join("")
          : list.slice(0, -1).join(", ") + " and " + list[list.length - 1];
      super(
        "The " +
          joined +
          " argument" +
          (names.length > 1 ? "s" : "") +
          " must be specified",
      );
      this.code = "ERR_MISSING_ARGS";
    }
  }

  class ERR_UNKNOWN_ENCODING extends TypeError {
    constructor(encoding) {
      super("Unknown encoding: " + encoding);
      this.code = "ERR_UNKNOWN_ENCODING";
    }
  }

  class ERR_INVALID_ARG_TYPE extends TypeError {
    constructor(name, expected, actual) {
      let message;
      if (name.endsWith(" argument")) {
        message = "The " + name + " ";
      } else {
        const type = name.includes(".") ? "property" : "argument";
        message = 'The "' + name + '" ' + type + " ";
      }
      super(message + "must be " + formatExpected(expected) +
        ". Received " + describe(actual));
      this.code = "ERR_INVALID_ARG_TYPE";
    }
  }

  const invalidArgValue = (Base) =>
    class extends Base {
      constructor(name, value, reason = "is invalid") {
        const type = name.includes(".") ? "property" : "argument";
        super(
          "The " + type + " '" + name + "' " + reason +
            ". Received " + inspectValue(value),
        );
        this.code = "ERR_INVALID_ARG_VALUE";
      }
    };

  const ERR_INVALID_ARG_VALUE = invalidArgValue(TypeError);
  ERR_INVALID_ARG_VALUE.RangeError = invalidArgValue(RangeError);

  class ERR_INVALID_THIS extends TypeError {
    constructor(what) {
      super('Value of "this" must be of type ' + what);
      this.code = "ERR_INVALID_THIS";
    }
  }

  class ERR_UNHANDLED_ERROR extends Error {
    constructor(error = undefined) {
      super(error === undefined ? "Unhandled error." : "Unhandled error. (" + error + ")");
      this.code = "ERR_UNHANDLED_ERROR";
    }
  }

  const addNumericalSeparator = (value) => {
    let result = "";
    let index = value.length;
    const start = value[0] === "-" ? 1 : 0;
    for (; index >= start + 4; index -= 3) {
      result = "_" + value.slice(index - 3, index) + result;
    }
    return value.slice(0, index) + result;
  };

  class ERR_OUT_OF_RANGE extends RangeError {
    constructor(name, range, value, replaceDefaultBoolean = false) {
      let message = replaceDefaultBoolean ? name :
        'The value of "' + name + '" is out of range.';
      let received;
      if (Number.isInteger(value) && Math.abs(value) > 2 ** 32) {
        received = addNumericalSeparator(String(value));
      } else if (typeof value === "bigint") {
        received = String(value);
        if (value > 2n ** 32n || value < -(2n ** 32n)) {
          received = addNumericalSeparator(received);
        }
        received += "n";
      } else {
        received = inspectRaw(value);
      }
      super(message + " It must be " + range + ". Received " + received);
      this.code = "ERR_OUT_OF_RANGE";
    }
  }
  const hideStackFrames = (fn) => {
    function wrapped(...args) {
      try {
        return Reflect.apply(fn, this, args);
      } catch (error) {
        if (Error.stackTraceLimit && error instanceof Error) {
          Error.captureStackTrace(error, wrapped);
        }
        throw error;
      }
    }
    wrapped.withoutStackTrace = fn;
    return wrapped;
  };

  const withHiddenStackFrames = (ErrorClass) => {
    ErrorClass.HideStackFramesError = class extends ErrorClass {
      constructor(...args) {
        if (isErrorStackTraceLimitWritable()) {
          const limit = Error.stackTraceLimit;
          Error.stackTraceLimit = 0;
          super(...args);
          Error.stackTraceLimit = limit;
        } else {
          super(...args);
        }
      }
    };
    return ErrorClass;
  };

  class ERR_ACCESS_DENIED extends Error {
    constructor(message, permission = "", resource = "") {
      super(message);
      this.code = "ERR_ACCESS_DENIED";
      this.permission = permission;
      this.resource = resource;
    }
  }

  class ERR_FS_FILE_TOO_LARGE extends RangeError {
    constructor(size) {
      super("File size (" + size + ") is greater than 2 GiB");
      this.code = "ERR_FS_FILE_TOO_LARGE";
    }
  }
  class ERR_FS_EISDIR extends Error {
    constructor(context) {
      const prefix = "Path is a directory";
      let message =
        prefix + ": " + context.syscall + " returned " +
        context.code + " (" + context.message + ")";
      if (context.path !== undefined) message += " " + context.path;
      if (context.dest !== undefined) message += " => " + context.dest;
      super(message);
      this.code = "ERR_FS_EISDIR";
      this.name = "SystemError";
      this.info = context;
      this.errno = context.errno;
      this.syscall = context.syscall;
      if (context.path !== undefined) this.path = context.path;
      if (context.dest !== undefined) this.dest = context.dest;
    }
  }

  class ERR_INVALID_FILE_URL_HOST extends TypeError {
    constructor(platform) {
      super('File URL host must be "localhost" or empty on ' + platform);
      this.code = "ERR_INVALID_FILE_URL_HOST";
    }
  }

  class ERR_INVALID_FILE_URL_PATH extends TypeError {
    constructor(reason, input) {
      super("File URL path " + reason);
      this.code = "ERR_INVALID_FILE_URL_PATH";
      this.input = input;
    }
  }

  class ERR_INVALID_URL_SCHEME extends TypeError {
    constructor(expected) {
      const schemes = typeof expected === "string" ? [expected] : expected;
      const reason = schemes.length === 2
        ? "one of scheme " + schemes[0] + " or " + schemes[1]
        : "of scheme " + schemes[0];
      super("The URL must be " + reason);
      this.code = "ERR_INVALID_URL_SCHEME";
    }
  }

  class ERR_INCOMPATIBLE_OPTION_PAIR extends TypeError {
    constructor(option1, option2) {
      super(
        'Option "' + option1 + '" cannot be used in combination with option "' +
          option2 + '"',
      );
      this.code = "ERR_INCOMPATIBLE_OPTION_PAIR";
    }
  }

  class ERR_NO_TEMPORAL extends Error {
    constructor() {
      super("Temporal is not supported in this environment");
      this.code = "ERR_NO_TEMPORAL";
    }
  }
  class UVException extends Error {
    constructor(context) {
      const { code, uvMessage } = uvErrorInfo(context.errno);
      let message = code + ": " + (context.message || uvMessage) + ", " + context.syscall;
      let path;
      let dest;
      if (context.path) {
        path = String(context.path);
        message += " '" + path + "'";
      }
      if (context.dest) {
        dest = String(context.dest);
        message += " -> '" + dest + "'";
      }
      super(message);
      this.code = code;
      this.errno = context.errno;
      this.syscall = context.syscall;
      if (path !== undefined) this.path = path;
      if (dest !== undefined) this.dest = dest;
    }
  }

  let systemErrorName;
  let systemErrorMessage;
  const uvErrorInfo = (errno) => {
    systemErrorName ??= require("node:thinpack/internal/util").getSystemErrorName;
    systemErrorMessage ??= require("node:thinpack/internal/util").getSystemErrorMessage;
    return { code: systemErrorName(errno), uvMessage: systemErrorMessage(errno) };
  };
  withHiddenStackFrames(ERR_INVALID_ARG_TYPE);
  withHiddenStackFrames(ERR_INVALID_ARG_VALUE);
  withHiddenStackFrames(ERR_OUT_OF_RANGE);
  withHiddenStackFrames(ERR_FS_EISDIR);
  withHiddenStackFrames(ERR_INCOMPATIBLE_OPTION_PAIR);
  class ERR_INVALID_URI extends URIError {
    constructor() {
      super("URI malformed");
      this.code = "ERR_INVALID_URI";
    }
  }

  class ERR_UNKNOWN_SIGNAL extends TypeError {
    constructor(signal) {
      super("Unknown signal: " + String(signal));
      this.code = "ERR_UNKNOWN_SIGNAL";
    }
  }

  class ERR_INTERNAL_ASSERTION extends Error {
    constructor(message) {
      const suffix = "This is caused by either a bug in Node.js " +
        "or incorrect usage of Node.js internals.\n" +
        "Please open an issue with this stack trace at " +
        "https://github.com/nodejs/node/issues\n";
      super(message === undefined ? suffix : message + "\n" + suffix);
      this.code = "ERR_INTERNAL_ASSERTION";
    }
  }

  class ERR_FALSY_VALUE_REJECTION extends Error {
    constructor(reason) {
      super("Promise was rejected with falsy value");
      this.code = "ERR_FALSY_VALUE_REJECTION";
      this.reason = reason;
    }
  }
  ERR_FALSY_VALUE_REJECTION.HideStackFramesError =
    class extends ERR_FALSY_VALUE_REJECTION {};

  class ERR_NOT_IN_SINGLE_EXECUTABLE_APPLICATION extends Error {
    constructor() {
      super("Operation cannot be invoked when not in a single-executable application");
      this.code = "ERR_NOT_IN_SINGLE_EXECUTABLE_APPLICATION";
    }
  }

  class ERR_SINGLE_EXECUTABLE_APPLICATION_ASSET_NOT_FOUND extends Error {
    constructor(key) {
      super("Cannot find asset " + key + " for the single executable application");
      this.code = "ERR_SINGLE_EXECUTABLE_APPLICATION_ASSET_NOT_FOUND";
    }
  }

  function aggregateTwoErrors(innerError, outerError) {
    if (innerError && outerError && innerError !== outerError) {
      if (Array.isArray(outerError.errors)) {
        outerError.errors.push(innerError);
        return outerError;
      }
      const error = new AggregateError([outerError, innerError], outerError.message);
      error.code = outerError.code;
      return error;
    }
    return innerError || outerError;
  }

  class AbortError extends Error {
    constructor(message = "The operation was aborted", options) {
      super(message, options);
      this.code = "ABORT_ERR";
      this.name = "AbortError";
    }
  }
  const opErrorParts = (error) => {
    const parts = String(error.message).split("\0");
    return [parts[0], parts[1]];
  };

  const opError = (error) => {
    const [code, detail] = opErrorParts(error);
    if (detail === undefined) return error;
    const failure = new Error(detail);
    failure.code = code;
    return failure;
  };
  const genericNodeError = (message, options) => {
    const error = new Error(message);
    if (options !== undefined && options !== null) {
      for (const key of Object.keys(options)) {
        if (key === "cause") error.cause = options.cause;
        else error[key] = options[key];
      }
    }
    return error;
  };
  const isErrorStackTraceLimitWritable = () => {
    const descriptor = Object.getOwnPropertyDescriptor(Error, "stackTraceLimit");
    if (descriptor === undefined) return Object.isExtensible(Error);
    return Object.hasOwn(descriptor, "writable") ? descriptor.writable : true;
  };

  let stackOverflowName;
  let stackOverflowMessage;
  const isStackOverflowError = (error) => {
    if (stackOverflowMessage === undefined) {
      try {
        const overflowStack = () => overflowStack();
        overflowStack();
      } catch (overflow) {
        stackOverflowName = overflow.name;
        stackOverflowMessage = overflow.message;
      }
    }
    return Boolean(error) &&
      error.name === stackOverflowName &&
      error.message === stackOverflowMessage;
  };
  class ErrnoException extends Error {
    constructor(errno, syscall, original) {
      const { getSystemErrorName } = require("node:thinpack/internal/util");
      const code = getSystemErrorName(errno);
      super(
        original ? syscall + " " + code + " " + original : syscall + " " + code,
      );
      this.errno = errno;
      this.code = code;
      this.syscall = syscall;
    }
  }

  class ExceptionWithHostPort extends Error {
    constructor(errno, syscall, address, port, additional) {
      const { getSystemErrorName } = require("node:thinpack/internal/util");
      const code = getSystemErrorName(errno);
      let details = "";
      if (port && port > 0) details = " " + address + ":" + port;
      else if (address) details = " " + address;
      if (additional) details += " - Local (" + additional + ")";
      super(syscall + " " + code + details);
      this.errno = errno;
      this.code = code;
      this.syscall = syscall;
      this.address = address;
      if (port) this.port = port;
    }
  }

  const formatMessage = (template, args) => {
    let index = 0;
    return template.replace(/%s/g, () => {
      const value = args[index];
      index += 1;
      return String(value);
    });
  };

  const streamError = (code, message, Base = Error) =>
    class extends Base {
      constructor(...args) {
        super(formatMessage(message, args));
        this.code = code;
      }
    };

  const ERR_ILLEGAL_CONSTRUCTOR = class extends TypeError {
    constructor() {
      super("Illegal constructor");
      this.code = "ERR_ILLEGAL_CONSTRUCTOR";
    }
  };
  const ERR_INVALID_RETURN_VALUE = class extends TypeError {
    constructor(input, name, value) {
      const type = describe(value);
      super(
        'Expected ' + input + " to be returned from the \"" + name +
          '" function but got ' + type + ".",
      );
      this.code = "ERR_INVALID_RETURN_VALUE";
    }
  };
  const ERR_METHOD_NOT_IMPLEMENTED = streamError(
    "ERR_METHOD_NOT_IMPLEMENTED",
    "The %s method is not implemented",
  );
  const ERR_MULTIPLE_CALLBACK = streamError(
    "ERR_MULTIPLE_CALLBACK",
    "Callback called multiple times",
  );
  const ERR_STREAM_ALREADY_FINISHED = streamError(
    "ERR_STREAM_ALREADY_FINISHED",
    "Cannot call %s after a stream was finished",
  );
  const ERR_STREAM_CANNOT_PIPE = streamError(
    "ERR_STREAM_CANNOT_PIPE",
    "Cannot pipe, not readable",
  );
  const ERR_STREAM_DESTROYED = streamError(
    "ERR_STREAM_DESTROYED",
    "Cannot call %s after a stream was destroyed",
  );
  const ERR_STREAM_NULL_VALUES = streamError(
    "ERR_STREAM_NULL_VALUES",
    "May not write null values to stream",
    TypeError,
  );
  const ERR_STREAM_PREMATURE_CLOSE = streamError(
    "ERR_STREAM_PREMATURE_CLOSE",
    "Premature close",
  );
  const ERR_STREAM_PUSH_AFTER_EOF = streamError(
    "ERR_STREAM_PUSH_AFTER_EOF",
    "stream.push() after EOF",
  );
  const ERR_STREAM_UNABLE_TO_PIPE = streamError(
    "ERR_STREAM_UNABLE_TO_PIPE",
    "Cannot pipe to a closed or destroyed stream",
  );
  const ERR_STREAM_UNSHIFT_AFTER_END_EVENT = streamError(
    "ERR_STREAM_UNSHIFT_AFTER_END_EVENT",
    "stream.unshift() after end event",
  );
  const ERR_STREAM_WRITE_AFTER_END = streamError(
    "ERR_STREAM_WRITE_AFTER_END",
    "write after end",
    TypeError,
  );
  const invalidState = (Base) =>
    class extends Base {
      constructor(...args) {
        const message = args.length === 0
          ? "Invalid state"
          : "Invalid state: " + args.map(String).join(" ");
        super(message);
        this.code = "ERR_INVALID_STATE";
      }
    };
  const ERR_INVALID_STATE = invalidState(Error);
  ERR_INVALID_STATE.Error = ERR_INVALID_STATE;
  ERR_INVALID_STATE.TypeError = invalidState(TypeError);
  ERR_INVALID_STATE.RangeError = invalidState(RangeError);

  module.exports = {
    AbortError,
    aggregateTwoErrors,
    ERR_ACCESS_DENIED,
    ERR_INVALID_ARG_TYPE,
    ErrnoException,
    ExceptionWithHostPort,
    UVException,
    genericNodeError,
    hideStackFrames,
    isErrorStackTraceLimitWritable,
    opError,
    opErrorParts,
    isStackOverflowError,
    kEnhanceStackBeforeInspector: Symbol("kEnhanceStackBeforeInspector"),
    codes: {
      ERR_ACCESS_DENIED,
      ERR_FS_EISDIR,
      ERR_FS_FILE_TOO_LARGE,
      ERR_INCOMPATIBLE_OPTION_PAIR,
      ERR_INVALID_FILE_URL_HOST,
      ERR_INVALID_FILE_URL_PATH,
      ERR_INVALID_URL_SCHEME,
      ERR_NO_TEMPORAL,
      ERR_BUFFER_OUT_OF_BOUNDS,
      ERR_INVALID_BUFFER_SIZE,
      ERR_MISSING_ARGS,
      ERR_UNKNOWN_ENCODING,
      ERR_FALSY_VALUE_REJECTION,
      ERR_INTERNAL_ASSERTION,
      ERR_INVALID_ARG_TYPE,
      ERR_INVALID_ARG_VALUE,
      ERR_INVALID_THIS,
      ERR_INVALID_URI,
      ERR_NOT_IN_SINGLE_EXECUTABLE_APPLICATION,
      ERR_SINGLE_EXECUTABLE_APPLICATION_ASSET_NOT_FOUND,
      ERR_NOT_IMPLEMENTED: class ERR_NOT_IMPLEMENTED extends Error {
        constructor(feature) {
          super((feature || "This feature") + " is not implemented yet");
          this.code = "ERR_NOT_IMPLEMENTED";
        }
      },
      ERR_ARG_NOT_ITERABLE: class extends TypeError {
        constructor(name) {
          super(`The "${name}" argument must be iterable`);
          this.code = "ERR_ARG_NOT_ITERABLE";
        }
      },
      ERR_MISSING_OPTION: class extends TypeError {
        constructor(name) {
          super(`"${name}" option is required`);
          this.code = "ERR_MISSING_OPTION";
        }
      },
      ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH: class extends RangeError {
        constructor() {
          super("Input buffers must have the same byte length");
          this.code = "ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH";
        }
      },
      ERR_DIR_CLOSED: class extends Error {
        constructor() {
          super("Directory handle was closed");
          this.code = "ERR_DIR_CLOSED";
        }
      },
      ERR_DIR_CONCURRENT_OPERATION: class extends Error {
        constructor() {
          super("Cannot do synchronous work on directory handle with concurrent asynchronous operations");
          this.code = "ERR_DIR_CONCURRENT_OPERATION";
        }
      },
      ERR_INVALID_STATE,

      ERR_OPERATION_FAILED: class extends Error {
        constructor(message) {
          super(message || "Operation failed");
          this.code = "ERR_OPERATION_FAILED";
        }
      },
      ERR_PARSE_ARGS_INVALID_OPTION_VALUE: class extends TypeError {
        constructor(message) {
          super(formatMessage("%s", [message]));
          this.code = "ERR_PARSE_ARGS_INVALID_OPTION_VALUE";
        }
      },
      ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL: class extends TypeError {
        constructor(value) {
          super(`Unexpected argument '${value}'. This command does not take positional arguments`);
          this.code = "ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL";
        }
      },
      ERR_PARSE_ARGS_UNKNOWN_OPTION: class extends TypeError {
        constructor(option, allowPositionals) {
          const suggest = allowPositionals
            ? `. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- ${JSON.stringify(option)}`
            : "";
          super(`Unknown option '${option}'${suggest}`);
          this.code = "ERR_PARSE_ARGS_UNKNOWN_OPTION";
        }
      },
      ERR_WORKER_UNSUPPORTED_OPERATION: class extends TypeError {
        constructor(operation) {
          super(`${operation} is not supported in workers`);
          this.code = "ERR_WORKER_UNSUPPORTED_OPERATION";
        }
      },
      ERR_FS_CP_EINVAL: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_EINVAL";
          this.errno = -22;
          this.syscall = "cp";
        }
      },
      ERR_FS_CP_DIR_TO_NON_DIR: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_DIR_TO_NON_DIR";
          this.errno = -20;
          this.syscall = "cp";
        }
      },
      ERR_FS_CP_NON_DIR_TO_DIR: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_NON_DIR_TO_DIR";
          this.errno = -20;
          this.syscall = "cp";
        }
      },
      ERR_FS_CP_SOCKET: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_SOCKET";
          this.errno = -22;
          this.syscall = "cp";
        }
      },
      ERR_FS_CP_FIFO_PIPE: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_FIFO_PIPE";
          this.errno = -22;
          this.syscall = "cp";
        }
      },
      ERR_FS_CP_UNKNOWN: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_UNKNOWN";
          this.errno = -22;
          this.syscall = "cp";
        }
      },
      ERR_FS_CP_EEXIST: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_EEXIST";
          this.errno = -17;
          this.syscall = "cp";
        }
      },
      ERR_FS_CP_SYMLINK_TO_SUBDIRECTORY: class extends Error {
        constructor(options) {
          super(typeof options === "object" && options !== null ? options.message : options);
          this.code = "ERR_FS_CP_SYMLINK_TO_SUBDIRECTORY";
          this.errno = -22;
          this.syscall = "cp";
        }
      },
      ERR_THROTTLED: class extends Error {
        constructor() {
          super("Throttled");
          this.code = "ERR_THROTTLED";
        }
      },
      ERR_ILLEGAL_CONSTRUCTOR,
      ERR_INVALID_RETURN_VALUE,
      ERR_METHOD_NOT_IMPLEMENTED,
      ERR_MULTIPLE_CALLBACK,
      ERR_STREAM_ALREADY_FINISHED,
      ERR_STREAM_CANNOT_PIPE,
      ERR_STREAM_DESTROYED,
      ERR_STREAM_NULL_VALUES,
      ERR_STREAM_PREMATURE_CLOSE,
      ERR_STREAM_PUSH_AFTER_EOF,
      ERR_STREAM_UNABLE_TO_PIPE,
      ERR_STREAM_UNSHIFT_AFTER_END_EVENT,
      ERR_STREAM_WRITE_AFTER_END,
      ERR_OUT_OF_RANGE,
      ERR_UNHANDLED_ERROR,
      ERR_UNKNOWN_SIGNAL,
    },
  };
})();
