(() => {
  const { codes } = require("node:thinpack/internal/errors");

  const unsupported = (encoding) => {
    const error = new RangeError(
      "The encoding label provided ('" + encoding + "') is invalid.",
    );
    error.code = "ERR_ENCODING_NOT_SUPPORTED";
    return error;
  };

  const toBytes = (input) => {
    if (input instanceof Uint8Array) return input;
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (ArrayBuffer.isView(input)) {
      return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    }
    throw new TypeError(
      "The input argument must be an instance of ArrayBuffer or ArrayBufferView",
    );
  };

  const { op_binding_utf8_slice, op_binding_utf8_units, op_binding_utf8_write } =
    Deno.core.ops;

  // `TextDecoder` drops a leading BOM unless `ignoreBOM` is set; decoding itself
  // is the buffer binding's maximal-subpart replacement, not one U+FFFD per byte.
  const decodeUtf8 = (bytes) => {
    const decoded = op_binding_utf8_slice(bytes, 0, bytes.length);
    return decoded.charCodeAt(0) === 0xfeff ? decoded.slice(1) : decoded;
  };

  const decodeUtf16le = (bytes) => {
    let index = 0;
    if (bytes.length >= 2 && bytes[0] === 0xff && bytes[1] === 0xfe) index = 2;
    let out = "";
    for (; index + 1 < bytes.length; index += 2) {
      out += String.fromCharCode(bytes[index] | (bytes[index + 1] << 8));
    }
    return out;
  };

  const decodeLatin1 = (bytes) => {
    let out = "";
    for (const byte of bytes) out += String.fromCharCode(byte);
    return out;
  };

  class TextDecoder {
    constructor(encoding = "utf-8") {
      const label = String(encoding).toLowerCase();
      if (label === "utf-8" || label === "utf8") {
        this._decode = decodeUtf8;
        this.encoding = "utf-8";
      } else if (label === "utf-16le" || label === "utf16le") {
        this._decode = decodeUtf16le;
        this.encoding = "utf-16le";
      } else if (label === "latin1" || label === "ascii" || label === "windows-1252") {
        this._decode = decodeLatin1;
        this.encoding = "windows-1252";
      } else {
        throw unsupported(encoding);
      }
      this.fatal = false;
      this.ignoreBOM = false;
    }

    decode(input) {
      if (input === undefined) return "";
      return this._decode(toBytes(input));
    }
  }
  class TextEncoder {
    constructor() {
      this.encoding = "utf-8";
    }

    encode(input = "") {
      return new Uint8Array(Buffer.from(String(input), "utf8"));
    }

    encodeInto(source, destination) {
      const text = String(source);
      const written = op_binding_utf8_write(destination, 0, destination.length, text);
      return {
        read: op_binding_utf8_units(destination.subarray(0, written)),
        written,
      };
    }
  }

  const getUtf8Decoder = () => new TextDecoder("utf-8");

  module.exports = { TextDecoder, TextEncoder, getUtf8Decoder };
})();
