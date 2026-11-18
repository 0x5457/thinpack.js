(() => {
  const { Buffer } = require("node:buffer");
  const { codes } = require("node:thinpack/internal/errors");

  const serialize = (value) => Buffer.from(Deno.core.serialize(value));
  const deserialize = (buffer) => {
    const bytes = ArrayBuffer.isView(buffer)
      ? new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength)
      : new Uint8Array(buffer);
    return Deno.core.deserialize(bytes);
  };

  module.exports = {
    serialize,
    deserialize,
    startupSnapshot: {
      isBuildingSnapshot: () => false,
      addSerializeCallback() {},
      addDeserializeCallback() {},
      setDeserializeMainFunction() {},
    },
    getHeapStatistics: () => {
      // The engine binding reports real heap usage on both backends
      // (`JS_ComputeMemoryUsage` under QuickJS, V8's `GetHeapStatistics`
      // under the V8 backend); it exposes no limit/peak counters, so
      // `heap_size_limit` stays an honest zero.
      const { heapTotal, heapUsed, external } = Deno.core.memoryUsage();
      return {
        total_heap_size: heapTotal,
        used_heap_size: heapUsed,
        external_memory: external,
        heap_size_limit: 0,
      };
    },
    getHeapSpaceStatistics: () => [],
    cachedDataVersionTag: () => 0,
    setFlagsFromString() {},
    // The V8 masquerade string is owned by `process.versions.v8`; reading it
    // back keeps `getV8Version()` and `process.versions.v8` consistent.
    getV8Version: () => process.versions.v8,
    Serializer() {
      throw new codes.ERR_NOT_IMPLEMENTED("v8.Serializer");
    },
    Deserializer() {
      throw new codes.ERR_NOT_IMPLEMENTED("v8.Deserializer");
    },
  };
})();
