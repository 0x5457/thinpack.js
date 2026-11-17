(() => {
  const kSep = "\0";

  const toBytes = (part) => {
    if (part instanceof Uint8Array) return part;
    if (ArrayBuffer.isView(part)) {
      return new Uint8Array(part.buffer, part.byteOffset, part.byteLength);
    }
    return new Uint8Array(part);
  };

  class BlobData {
    constructor(parts, length) {
      this.parts = parts;
      this.length = length;
    }

    getReader() {
      const parts = this.parts;
      let index = 0;
      return {
        pull(callback) {
          if (index >= parts.length) {
            callback(0);
            return;
          }
          callback(1, parts[index]);
          index += 1;
        },
      };
    }

    slice(start, end) {
      const joined = new Uint8Array(this.length);
      let offset = 0;
      for (const part of this.parts) {
        joined.set(part, offset);
        offset += part.length;
      }
      const sliced = joined.subarray(start, end);
      return new BlobData([sliced], sliced.length);
    }
  }

  const flatten = (source) => {
    if (source instanceof BlobData) {
      return source.parts;
    }
    return [toBytes(source)];
  };

  const createBlob = (sources, length) => {
    const parts = [];
    for (const source of sources) {
      for (const part of flatten(source)) {
        parts.push(part);
      }
    }
    return new BlobData(parts, length);
  };
  const concat = (buffers) => {
    let total = 0;
    for (const buffer of buffers) total += buffer.byteLength;
    const joined = new Uint8Array(total);
    let offset = 0;
    for (const buffer of buffers) {
      joined.set(new Uint8Array(buffer.buffer ?? buffer, buffer.byteOffset ?? 0, buffer.byteLength ?? buffer.length), offset);
      offset += buffer.byteLength;
    }
    return joined.buffer;
  };
  const createBlobFromFilePath = () => undefined;
  const getDataObject = () => undefined;

  module.exports = {
    createBlob,
    createBlobFromFilePath,
    concat,
    getDataObject,
  };
})();
