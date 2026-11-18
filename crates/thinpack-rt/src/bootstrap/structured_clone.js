(() => {
  const dataCloneError = (message) => {
    const error = new Error(message);
    error.name = "DataCloneError";
    return error;
  };

  // `worker_threads.markAsUncloneable` marks an object so cloning it throws; the set lives on
  // globalThis because the worker_threads module owns the marking and this bootstrap owns the
  // cloning, and neither can reach the other's scope.
  const uncloneable = (globalThis.__thinpack_uncloneable ??= new WeakSet());

  const cloneValue = (value, seen) => {
    if (value === null || typeof value !== "object") {
      if (typeof value === "function" || typeof value === "symbol") {
        throw dataCloneError("Value could not be cloned");
      }
      return value;
    }
    if (seen.has(value)) return seen.get(value);
    if (uncloneable.has(value)) throw dataCloneError("Value could not be cloned");

    if (value instanceof Date) return new Date(value.getTime());
    if (value instanceof RegExp) return new RegExp(value.source, value.flags);
    if (value instanceof ArrayBuffer) {
      const copy = new ArrayBuffer(value.byteLength);
      new Uint8Array(copy).set(new Uint8Array(value));
      return copy;
    }
    if (ArrayBuffer.isView(value)) {
      const buffer = cloneValue(value.buffer, seen);
      if (value instanceof DataView) {
        return new DataView(buffer, value.byteOffset, value.byteLength);
      }
      return new value.constructor(buffer, value.byteOffset, value.length);
    }
    if (typeof Blob !== "undefined" && value instanceof Blob) {
      const blob = new Blob([value], { type: value.type });
      if (typeof File !== "undefined" && value instanceof File) {
        return new File([blob], value.name, {
          type: value.type,
          lastModified: value.lastModified,
        });
      }
      return blob;
    }
    if (value instanceof Map) {
      const copy = new Map();
      seen.set(value, copy);
      for (const [key, entry] of value) {
        copy.set(cloneValue(key, seen), cloneValue(entry, seen));
      }
      return copy;
    }
    if (value instanceof Set) {
      const copy = new Set();
      seen.set(value, copy);
      for (const entry of value) copy.add(cloneValue(entry, seen));
      return copy;
    }
    if (Array.isArray(value)) {
      const copy = [];
      seen.set(value, copy);
      for (const entry of value) copy.push(cloneValue(entry, seen));
      return copy;
    }

    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      throw dataCloneError("Value could not be cloned");
    }
    const copy = Object.create(prototype);
    seen.set(value, copy);
    for (const key of Object.keys(value)) {
      copy[key] = cloneValue(value[key], seen);
    }
    return copy;
  };

  const structuredClone = (value, options = {}) => {
    if (options && options.transfer !== undefined && options.transfer.length > 0) {
      const error = new Error("structuredClone with a transfer list is not implemented yet");
      error.code = "ERR_NOT_IMPLEMENTED";
      throw error;
    }
    return cloneValue(value, new Map());
  };

  Object.defineProperty(globalThis, "structuredClone", {
    value: structuredClone,
    writable: true,
    configurable: true,
    enumerable: false,
  });
  Object.defineProperty(globalThis, "__thinpack_structured_clone", {
    value: structuredClone,
    writable: false,
    configurable: false,
    enumerable: false,
  });
})();
