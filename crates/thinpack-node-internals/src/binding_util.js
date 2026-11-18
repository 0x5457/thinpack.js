(() => {
  const types = require("node:thinpack/internal/binding/types");
  const { codes } = require("node:thinpack/internal/errors");
  const { op_binding_get_own_non_index_properties } = Deno.core.ops;
  const ALL_PROPERTIES = 0;
  const ONLY_ENUMERABLE = 2;
  const SKIP_SYMBOLS = 8;
  const kPending = 0;
  const kFulfilled = 1;
  const kRejected = 2;

  const getOwnNonIndexProperties = (object, filter) =>
    op_binding_get_own_non_index_properties(object, filter);
  const getConstructorName = (object) => {
    let current = object;
    while (current !== null) {
      const descriptor = Object.getOwnPropertyDescriptor(current, "constructor");
      if (
        descriptor !== undefined &&
        typeof descriptor.value === "function" &&
        descriptor.value.name !== ""
      ) {
        return descriptor.value.name;
      }
      current = Object.getPrototypeOf(current);
    }
    return "";
  };
  // `Deno.core.getPromiseDetails` returns the engine's real promise state
  // (0 pending, 1 fulfilled, 2 rejected) and settlement value.
  const getPromiseDetails = (value) =>
    types.isPromise(value) ? Deno.core.getPromiseDetails(value) : undefined;
  const getProxyDetails = () => undefined;
  const getExternalValue = () => undefined;
  const previewEntries = (value, withIsKeyValue) => {
    let entries;
    let isKeyValue = false;
    if (types.isMap(value)) {
      entries = [];
      for (const [key, entryValue] of value) entries.push(key, entryValue);
      isKeyValue = true;
    } else if (types.isSet(value)) {
      entries = [...value];
    } else if (types.isMapIterator(value)) {
      entries = [];
      isKeyValue = true;
    } else if (types.isSetIterator(value)) {
      entries = [];
    } else {
      return undefined;
    }
    return withIsKeyValue === true ? [entries, isKeyValue] : entries;
  };
  const getCallSites = (frameCount) => {
    const stackTraceLimit = Error.stackTraceLimit;
    const prepareStackTrace = Error.prepareStackTrace;
    Error.stackTraceLimit = frameCount + 1;
    Error.prepareStackTrace = (_, frames) => frames;
    const error = new Error();
    Error.captureStackTrace(error, getCallSites);
    const frames = error.stack;
    Error.stackTraceLimit = stackTraceLimit;
    Error.prepareStackTrace = prepareStackTrace;

    return frames.map((frame) => ({
      __proto__: null,
      functionName: frame.getFunctionName() ?? "",
      scriptName: frame.getFileName() ?? "",
      lineNumber: frame.getLineNumber() ?? 0,
      columnNumber: frame.getColumnNumber() ?? 0,
      column: frame.getColumnNumber() ?? 0,
    }));
  };
  const isInsideNodeModules = () => false;
  const parseEnv = () => {
    throw new codes.ERR_NOT_IMPLEMENTED("util.parseEnv");
  };
  const getSharedArrayBufferGrowable = (buffer) =>
    Boolean(buffer && buffer.growable === true);

  module.exports = {
    getSharedArrayBufferGrowable,
    markPromiseAsHandled: () => false,
    constants: { ALL_PROPERTIES, ONLY_ENUMERABLE, SKIP_SYMBOLS, kPending, kFulfilled, kRejected },
    getCallSites,
    getConstructorName,
    getExternalValue,
    getOwnNonIndexProperties,
    getPromiseDetails,
    getProxyDetails,
    isInsideNodeModules,
    parseEnv,
    privateSymbols: {
      untransferable_object_private_symbol: Symbol(
        "untransferable_object_private_symbol",
      ),
    },
    previewEntries,
  };
})();
