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
    getHeapStatistics: () => ({
      total_heap_size: 0,
      used_heap_size: 0,
      external_memory: 0,
      heap_size_limit: 0,
    }),
    getHeapSpaceStatistics: () => [],
    cachedDataVersionTag: () => 0,
    setFlagsFromString() {},
    getV8Version: () => "v8x",
    Serializer() {
      throw new codes.ERR_NOT_IMPLEMENTED("v8.Serializer");
    },
    Deserializer() {
      throw new codes.ERR_NOT_IMPLEMENTED("v8.Deserializer");
    },
  };
})();
