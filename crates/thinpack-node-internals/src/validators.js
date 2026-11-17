(() => {
  const { codes, hideStackFrames } = require("node:thinpack/internal/errors");
  const kValidateObjectAllowNullable = 1 << 0;
  const kValidateObjectAllowArray = 1 << 1;
  const kValidateObjectAllowFunction = 1 << 2;

  const isInt32 = (value) => value === (value | 0);

  const isUint32 = (value) => value === (value >>> 0);

  const validateArray = hideStackFrames((value, name) => {
    if (!Array.isArray(value)) {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "Array", value);
    }
  });

  const validateBuffer = hideStackFrames((value, name) => {
    if (!ArrayBuffer.isView(value)) {
      throw new codes.ERR_INVALID_ARG_TYPE(
        name,
        ["Buffer", "TypedArray", "DataView"],
        value,
      );
    }
  });

  const validateString = hideStackFrames((value, name) => {
    if (typeof value !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "string", value);
    }
  });

  const validateBoolean = hideStackFrames((value, name) => {
    if (typeof value !== "boolean") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "boolean", value);
    }
  });

  const validateFunction = hideStackFrames((value, name) => {
    if (typeof value !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "Function", value);
    }
  });

  const validateNumber = hideStackFrames((value, name, min, max) => {
    if (typeof value !== "number") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "number", value);
    }
    const belowMin = min != null && value < min;
    const aboveMax = max != null && value > max;
    const notANumber = (min != null || max != null) && Number.isNaN(value);
    if (belowMin || aboveMax || notANumber) {
      const range = [
        min != null ? ">= " + min : "",
        max != null ? "<= " + max : "",
      ]
        .filter((part) => part !== "")
        .join(" && ");
      throw new codes.ERR_OUT_OF_RANGE(name, range, value);
    }
  });

  const validateInteger = hideStackFrames((
    value,
    name,
    min = Number.MIN_SAFE_INTEGER,
    max = Number.MAX_SAFE_INTEGER,
  ) => {
    if (typeof value !== "number") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "number", value);
    }
    if (!Number.isInteger(value)) {
      throw new codes.ERR_OUT_OF_RANGE(name, "an integer", value);
    }
    if (value < min || value > max) {
      throw new codes.ERR_OUT_OF_RANGE(name, ">= " + min + " && <= " + max, value);
    }
  });
  const validateInt32 = hideStackFrames((
    value,
    name,
    min = -2147483648,
    max = 2147483647,
  ) => {
    if (typeof value !== "number") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "number", value);
    }
    if (!Number.isInteger(value)) {
      throw new codes.ERR_OUT_OF_RANGE(name, "an integer", value);
    }
    if (value < min || value > max) {
      throw new codes.ERR_OUT_OF_RANGE(name, ">= " + min + " && <= " + max, value);
    }
  });

  const validateUint32 = hideStackFrames((value, name, positive = false) => {
    if (typeof value !== "number") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "number", value);
    }
    if (!Number.isInteger(value)) {
      throw new codes.ERR_OUT_OF_RANGE(name, "an integer", value);
    }
    const min = positive ? 1 : 0;
    const max = 4294967295;
    if (value < min || value > max) {
      throw new codes.ERR_OUT_OF_RANGE(name, ">= " + min + " && <= " + max, value);
    }
  });

  const validateOneOf = hideStackFrames((value, name, oneOf) => {
    if (oneOf.includes(value)) return;
    const allowed = oneOf
      .map((entry) =>
        typeof entry === "string" ? "'" + entry + "'" : String(entry))
      .join(", ");
    throw new codes.ERR_INVALID_ARG_VALUE(
      name,
      value,
      "must be one of: " + allowed,
    );
  });

  const validateDictionary = hideStackFrames((value, name) => {
    if (value != null && typeof value !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "Object", value);
    }
  });

  const validateStringArray = hideStackFrames((value, name) => {
    if (!Array.isArray(value) || value.some((entry) => typeof entry !== "string")) {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "Array of strings", value);
    }
  });

  const validateBooleanArray = hideStackFrames((value, name) => {
    if (!Array.isArray(value) || value.some((entry) => typeof entry !== "boolean")) {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "Array of booleans", value);
    }
  });

  const validateUnion = hideStackFrames((value, name, allowed) => {
    if (!allowed.includes(value)) {
      throw new codes.ERR_INVALID_ARG_TYPE(
        name,
        allowed.map((entry) => `'${entry}'`).join(" or "),
        value,
      );
    }
  });

  const validateIgnoreOption = hideStackFrames((ignore, name) => {
    if (ignore === undefined) return;
    const entries = Array.isArray(ignore) ? ignore : [ignore];
    for (const entry of entries) {
      if (typeof entry === "string" || typeof entry === "function" || entry instanceof RegExp) {
        continue;
      }
      throw new codes.ERR_INVALID_ARG_TYPE(name, "string, RegExp, or function", entry);
    }
  });

  const validateObject = hideStackFrames((value, name, options) => {
    if (options === undefined) {
      if (value === null || Array.isArray(value) || typeof value !== "object") {
        throw new codes.ERR_INVALID_ARG_TYPE(name, "object", value);
      }
      return;
    }
    if ((options & kValidateObjectAllowNullable) === 0 && value === null) {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "object", value);
    }
    if ((options & kValidateObjectAllowArray) === 0 && Array.isArray(value)) {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "object", value);
    }
    const throwOnFunction = (options & kValidateObjectAllowFunction) === 0;
    if (throwOnFunction && typeof value !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "object", value);
    }
  });
  const validateAbortSignal = hideStackFrames((signal, name) => {
    if (signal === undefined) return;
    if (
      signal === null ||
      typeof signal !== "object" ||
      !("aborted" in signal)
    ) {
      throw new codes.ERR_INVALID_ARG_TYPE(name, "AbortSignal", signal);
    }
  });
  let normalizeEncoding;
  const validateEncoding = hideStackFrames((data, encoding) => {
    normalizeEncoding ??= require("node:thinpack/internal/util").normalizeEncoding;
    const length = data.length;
    if (normalizeEncoding(encoding) === "hex" && length % 2 !== 0) {
      throw new codes.ERR_INVALID_ARG_VALUE(
        "encoding",
        encoding,
        "is invalid for data of length " + length,
      );
    }
  });

  const octalReg = /^[0-7]+$/;
  const modeDesc = "must be a 32-bit unsigned integer or an octal string";

  const parseFileMode = hideStackFrames((value, name, def) => {
    value ??= def;
    if (typeof value === "string") {
      if (octalReg.test(value) === false) {
        throw new codes.ERR_INVALID_ARG_VALUE(name, value, modeDesc);
      }
      value = Number.parseInt(value, 8);
    }
    validateUint32.withoutStackTrace(value, name);
    return value + 0;
  });

  module.exports = {
    validateDictionary,
    isInt32,
    isUint32,
    kValidateObjectAllowArray,
    kValidateObjectAllowFunction,
    kValidateObjectAllowNullable,
    parseFileMode,
    validateAbortSignal,
    validateArray,
    validateBooleanArray,
    validateStringArray,
    validateUnion,
    validateBoolean,
    validateBuffer,
    validateEncoding,
    validateFunction,
    validateIgnoreOption,
    validateInt32,
    validateInteger,
    validateNumber,
    validateObject,
    validateOneOf,
    validateString,
    validateUint32,
  };
})();
