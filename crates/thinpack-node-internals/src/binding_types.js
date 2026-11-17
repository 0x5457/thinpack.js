(() => {
  const tag = (value) => Object.prototype.toString.call(value);

  const hasTag = (value, expected) =>
    value !== null &&
    (typeof value === "object" || typeof value === "function") &&
    tag(value) === expected;

  const isArgumentsObject = (value) => hasTag(value, "[object Arguments]");
  const isArrayBuffer = (value) => hasTag(value, "[object ArrayBuffer]");
  const isAsyncFunction = (value) => hasTag(value, "[object AsyncFunction]");
  const isBigIntObject = (value) => hasTag(value, "[object BigInt]");
  const isBooleanObject = (value) => hasTag(value, "[object Boolean]");
  const isDate = (value) => hasTag(value, "[object Date]");
  const isGeneratorFunction = (value) =>
    hasTag(value, "[object GeneratorFunction]");
  const isGeneratorObject = (value) => hasTag(value, "[object Generator]");
  const isMap = (value) => hasTag(value, "[object Map]");
  const isMapIterator = (value) => hasTag(value, "[object Map Iterator]");
  const isModuleNamespaceObject = (value) => hasTag(value, "[object Module]");
  const isNativeError = (value) => hasTag(value, "[object Error]");
  const isNumberObject = (value) => hasTag(value, "[object Number]");
  const isPromise = (value) => hasTag(value, "[object Promise]");
  const isRegExp = (value) => hasTag(value, "[object RegExp]");
  const isSet = (value) => hasTag(value, "[object Set]");
  const isSetIterator = (value) => hasTag(value, "[object Set Iterator]");
  const isSharedArrayBuffer = (value) =>
    hasTag(value, "[object SharedArrayBuffer]");
  const isStringObject = (value) => hasTag(value, "[object String]");
  const isSymbolObject = (value) => hasTag(value, "[object Symbol]");
  const isWeakMap = (value) => hasTag(value, "[object WeakMap]");
  const isWeakSet = (value) => hasTag(value, "[object WeakSet]");

  module.exports = {
    isAnyArrayBuffer: (value) =>
      isArrayBuffer(value) || isSharedArrayBuffer(value),
    isArgumentsObject,
    isArrayBuffer,
    isAsyncFunction,
    isBigIntObject,
    isBooleanObject,
    isBoxedPrimitive: (value) =>
      isBigIntObject(value) ||
      isBooleanObject(value) ||
      isNumberObject(value) ||
      isStringObject(value) ||
      isSymbolObject(value),
    isDate,
    isExternal: () => false,
    isGeneratorFunction,
    isGeneratorObject,
    isMap,
    isMapIterator,
    isModuleNamespaceObject,
    isNativeError,
    isNumberObject,
    isPromise,
    isProxy: () => false,
    isRegExp,
    isSet,
    isSetIterator,
    isSharedArrayBuffer,
    isStringObject,
    isSymbolObject,
    isWeakMap,
    isWeakSet,
  };
})();
