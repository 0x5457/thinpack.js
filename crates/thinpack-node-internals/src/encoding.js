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

  // A multi-byte character may straddle two `decode(bytes, { stream: true })`
  // calls; the incomplete tail is buffered on the decoder and completed by the
  // next call, so a character split across reads is not replaced with U+FFFD.
  // A non-stream call consumes any buffered tail and flushes a still-incomplete
  // one, like Node.
  const tailLengthOf = (bytes) => {
    for (let back = 1; back <= 3 && back <= bytes.length; back += 1) {
      const byte = bytes[bytes.length - back];
      if ((byte & 0xc0) === 0x80) continue;
      let expected;
      if (byte < 0x80) return 0;
      if (byte >= 0xf0) expected = 4;
      else if (byte >= 0xe0) expected = 3;
      else if (byte >= 0xc0) expected = 2;
      else return 0;
      return back < expected ? back : 0;
    }
    return 0;
  };

  const decodeLatin1 = (bytes) => {
    let out = "";
    for (const byte of bytes) out += String.fromCharCode(byte);
    return out;
  };

  const EMPTY_BYTES = new Uint8Array(0);

  const concatBytes = (left, right) => {
    if (left.length === 0) return right === undefined ? left : toBytes(right);
    if (right === undefined) return left;
    const bytes = new Uint8Array(left.length + toBytes(right).length);
    bytes.set(left, 0);
    bytes.set(toBytes(right), left.length);
    return bytes;
  };

  class TextDecoder {
    constructor(encoding = "utf-8") {
      const label = String(encoding).toLowerCase();
      if (label === "utf-8" || label === "utf8") {
        this.encoding = "utf-8";
      } else if (label === "utf-16le" || label === "utf16le") {
        this.encoding = "utf-16le";
      } else if (label === "latin1" || label === "ascii" || label === "windows-1252") {
        this.encoding = "windows-1252";
      } else {
        throw unsupported(encoding);
      }
      this.fatal = false;
      this.ignoreBOM = false;
      this._tail = EMPTY_BYTES;
    }

    decode(input, options) {
      if (input === undefined && this._tail.length === 0) return "";
      const stream = options?.stream === true;
      const bytes = concatBytes(this._tail, input);
      let tail = 0;
      let out;
      if (this.encoding === "utf-8") {
        tail = stream ? tailLengthOf(bytes) : 0;
        out = decodeUtf8(bytes.subarray(0, bytes.length - tail));
      } else if (this.encoding === "utf-16le") {
        tail = stream ? bytes.length & 1 : 0;
        out = decodeUtf16le(bytes.subarray(0, bytes.length - tail));
        if ((bytes.length & 1) === 1 && !stream) out += "\uFFFD";
      } else {
        out = decodeLatin1(bytes);
      }
      this._tail = tail === 0 ? EMPTY_BYTES : bytes.slice(bytes.length - tail);
      return out;
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
