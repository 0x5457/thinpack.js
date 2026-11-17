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

  const REPLACEMENT = "\ufffd";
  const decodeUtf8 = (bytes) => {
    let index = 0;
    let out = "";
    if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
      index = 3;
    }
    while (index < bytes.length) {
      const first = bytes[index];
      if (first < 0x80) {
        out += String.fromCharCode(first);
        index += 1;
        continue;
      }

      let needed;
      let code;
      let minimum;
      if ((first & 0xe0) === 0xc0) {
        needed = 1;
        code = first & 0x1f;
        minimum = 0x80;
      } else if ((first & 0xf0) === 0xe0) {
        needed = 2;
        code = first & 0x0f;
        minimum = 0x800;
      } else if ((first & 0xf8) === 0xf0) {
        needed = 3;
        code = first & 0x07;
        minimum = 0x10000;
      } else {
        out += REPLACEMENT;
        index += 1;
        continue;
      }

      let valid = true;
      for (let offset = 1; offset <= needed; offset += 1) {
        const byte = bytes[index + offset];
        if (byte === undefined || (byte & 0xc0) !== 0x80) {
          valid = false;
          break;
        }
        code = (code << 6) | (byte & 0x3f);
      }
      const validCode = code >= minimum && code <= 0x10ffff &&
        !(code >= 0xd800 && code <= 0xdfff);
      if (!valid || !validCode) {
        out += REPLACEMENT;
        index += 1;
        continue;
      }
      out += String.fromCodePoint(code);
      index += needed + 1;
    }
    return out;
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
      const bytes = Buffer.from(String(source), "utf8");
      const written = Math.min(bytes.length, destination.length);
      destination.set(bytes.subarray(0, written));
      return { read: String(source).length, written };
    }
  }

  const getUtf8Decoder = () => new TextDecoder("utf-8");

  module.exports = { TextDecoder, TextEncoder, getUtf8Decoder };
})();
