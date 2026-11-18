(() => {
  const { normalizeEncoding } = require("node:thinpack/internal/util");
  const kMaxLength = 9007199254740991;
  const kStringMaxLength = 536870888;

  const ASCII = 0;
  const UTF8 = 1;
  const UCS2 = 3;
  const LATIN1 = 4;

  const LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;
  const UCS2_CHUNK = 4096;

  // Every op below reads bytes: a `Uint32Array` view of the same memory has to be
  // reduced to its byte representation first, the way Node's binding does.
  const toBytes = (input) => {
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (input instanceof Uint8Array) return input;
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  };
  const {
    op_binding_ascii_slice,
    op_binding_base64_decode,
    op_binding_base64_slice,
    op_binding_compare,
    op_binding_hex_decode,
    op_binding_hex_slice,
    op_binding_index_of,
    op_binding_index_of_byte,
    op_binding_is_ascii,
    op_binding_is_utf8,
    op_binding_latin1_slice,
    op_binding_latin1_write,
    op_binding_swap,
    op_binding_utf8_length,
    op_binding_utf8_slice,
    op_binding_utf8_write,
  } = Deno.core.ops;
  const utf8Length = (string) => op_binding_utf8_length(string);
  const utf8WriteInto = (target, offset, maxBytes, string) =>
    op_binding_utf8_write(toBytes(target), offset, maxBytes, string);

  const utf8Encode = (string) => {
    const bytes = new Uint8Array(utf8Length(string));
    utf8WriteInto(bytes, 0, bytes.length, string);
    return bytes;
  };
  const utf8Decode = (bytes, start, end) =>
    op_binding_utf8_slice(bytes, start, end);
  // `ucs2` writes stay here: they consume raw UTF-16 code units, and a JS string
  // may hold unpaired surrogates that the engine's Rust-facing conversions
  // replace with U+FFFD. The `latin1`/`ascii` write op truncates the units itself,
  // but on QuickJS it recovers them from the string's UTF-8 bytes, so only a
  // well-formed string, which round-trips exactly, may take it.
  const latin1WriteInto = (target, offset, maxBytes, string) => {
    if (string.isWellFormed()) {
      return op_binding_latin1_write(target, offset, maxBytes, string);
    }
    const written = Math.min(maxBytes, string.length);
    for (let index = 0; index < written; index += 1) {
      target[offset + index] = string.charCodeAt(index) & 0xff;
    }
    return written;
  };

  const latin1Encode = (string) => {
    const bytes = new Uint8Array(string.length);
    latin1WriteInto(bytes, 0, bytes.length, string);
    return bytes;
  };

  const ucs2WriteInto = (target, offset, maxBytes, string) => {
    const characters = Math.min(maxBytes >>> 1, string.length);
    for (let index = 0; index < characters; index += 1) {
      const code = string.charCodeAt(index);
      target[offset + index * 2] = code & 0xff;
      target[offset + index * 2 + 1] = code >> 8;
    }
    return characters * 2;
  };

  const ucs2Encode = (string) => {
    const bytes = new Uint8Array(string.length * 2);
    ucs2WriteInto(bytes, 0, bytes.length, string);
    return bytes;
  };
  const hexDecode = (string) => op_binding_hex_decode(string);

  const hexSlice = (bytes, start, end) =>
    op_binding_hex_slice(toBytes(bytes), start, end);
  const base64Decode = (string) => op_binding_base64_decode(string);
  const base64Encode = (bytes, start, end, url) =>
    op_binding_base64_slice(toBytes(bytes), start, end, url);

  const utf8Slice = (buffer, start, end) => utf8Decode(toBytes(buffer), start, end);

  const asciiSlice = (buffer, start, end) =>
    op_binding_ascii_slice(toBytes(buffer), start, end);

  const latin1Slice = (buffer, start, end) =>
    op_binding_latin1_slice(toBytes(buffer), start, end);

  const ucs2Slice = (buffer, start, end) => {
    const bytes = toBytes(buffer);
    // A `Uint16Array` view needs an even address, and units stay on the parity of
    // `start`, so an odd offset cannot be viewed at all.
    if (
      !LITTLE_ENDIAN ||
      start < 0 ||
      end > bytes.length ||
      end - start < 2 ||
      (bytes.byteOffset + start) % 2 !== 0
    ) {
      let result = "";
      for (let index = start; index + 1 < end; index += 2) {
        result += String.fromCharCode(bytes[index] | (bytes[index + 1] << 8));
      }
      return result;
    }
    const units = new Uint16Array(
      bytes.buffer,
      bytes.byteOffset + start,
      (end - start) >> 1,
    );
    const parts = [];
    for (let offset = 0; offset < units.length; offset += UCS2_CHUNK) {
      parts.push(
        String.fromCharCode.apply(null, units.subarray(offset, offset + UCS2_CHUNK)),
      );
    }
    return parts.length === 1 ? parts[0] : parts.join("");
  };

  const writeBase64 = (buffer, string, offset, length) => {
    const decoded = base64Decode(string);
    const written = Math.min(length, decoded.length);
    buffer.set(decoded.subarray(0, written), offset);
    return written;
  };

  const hexWrite = (buffer, string, offset, length) => {
    const decoded = hexDecode(string);
    const written = Math.min(length, decoded.length);
    buffer.set(decoded.subarray(0, written), offset);
    return written;
  };
  const normalizeCompare = (value, leftLength, rightLength) => {
    if (value !== 0) return value > 0 ? 1 : -1;
    if (leftLength === rightLength) return 0;
    return leftLength > rightLength ? 1 : -1;
  };

  const compare = (left, right) => op_binding_compare(toBytes(left), toBytes(right));

  // The subarrays are views, so the scan itself is the native comparison.
  const compareOffset = (source, target, targetStart, sourceStart, targetEnd, sourceEnd) => {
    const length = Math.min(
      Math.min(sourceEnd - sourceStart, targetEnd - targetStart),
      source.length - sourceStart,
    );
    const value =
      length > 0
        ? compare(
            source.subarray(sourceStart, sourceStart + length),
            target.subarray(targetStart, targetStart + length),
          )
        : 0;
    return normalizeCompare(value, sourceEnd - sourceStart, targetEnd - targetStart);
  };

  const indexOfOffset = (length, offset, needleLength, forward) => {
    if (offset < 0) {
      if (offset + length >= 0) return length + offset;
      return forward || needleLength === 0 ? 0 : -1;
    }
    if (offset + needleLength <= length) return offset;
    if (needleLength === 0) return length;
    return forward ? -1 : length - 1;
  };
  const indexOfUnits = (haystack, needle, offset, forward, end, unit) => {
    const haystackLength = unit === 2 ? haystack.length & ~1 : haystack.length;
    const needleLength = unit === 2 ? needle.length & ~1 : needle.length;
    const searchEnd = Math.min(Math.max(end, 0), haystackLength);
    const start = indexOfOffset(haystackLength, offset, needleLength, forward);

    if (needleLength === 0) {
      return needle.length !== 0 ? -1 : Math.min(start, searchEnd);
    }
    if (haystackLength === 0 || start <= -1) return -1;
    if (unit === 2 && searchEnd < 2) return -1;

    let from = start;
    if (from >= searchEnd) {
      if (forward || searchEnd === 0) return -1;
      from = searchEnd - unit;
    }
    if ((forward && needleLength + from > searchEnd) || needleLength > searchEnd) {
      return -1;
    }

    const last = searchEnd - needleLength;
    return op_binding_index_of(
      haystack,
      needle,
      forward ? from : Math.min(from, last),
      last,
      unit,
      forward,
    );
  };

  const indexOfBuffer = (haystack, needle, offset, encoding, forward, end) =>
    indexOfUnits(haystack, needle, offset, forward, end, encoding === UCS2 ? 2 : 1);

  const indexOfString = (haystack, string, offset, encoding, forward, end) => {
    const needle =
      encoding === ASCII || encoding === LATIN1
        ? latin1Encode(string)
        : encoding === UCS2
          ? ucs2Encode(string)
          : utf8Encode(string);
    return indexOfUnits(haystack, needle, offset, forward, end, encoding === UCS2 ? 2 : 1);
  };

  const indexOfNumber = (haystack, value, offset, forward, end) => {
    const start = indexOfOffset(haystack.length, offset, 1, forward);
    if (start <= -1 || haystack.length === 0) return -1;
    const searchEnd = Math.min(Math.max(end, 0), haystack.length);
    return op_binding_index_of_byte(haystack, value & 0xff, start, searchEnd, forward);
  };
  const fill = (buffer, value, offset, end, encoding) => {
    const fillLength = end - offset;
    if (offset > end || fillLength + offset > buffer.byteLength) return -2;

    if (typeof value === "number") {
      buffer.fill(value & 0xff, offset, end);
      return undefined;
    }

    const pattern = ArrayBuffer.isView(value)
      ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
      : fillPattern(String(value), encoding);
    const written = Math.min(pattern.length, fillLength);
    if (written === 0) return -1;

    buffer.set(pattern.subarray(0, written), offset);
    for (let position = written; position < fillLength; position += written) {
      const count = Math.min(written, fillLength - position);
      buffer.copyWithin(offset + position, offset, offset + count);
    }
    return undefined;
  };
  const copy = (source, target, targetStart, sourceStart, toCopy) => {
    const length = Math.max(
      0,
      Math.min(toCopy, source.byteLength - sourceStart, target.byteLength - targetStart),
    );
    target.set(source.subarray(sourceStart, sourceStart + length), targetStart);
    return length;
  };
  const fillPattern = (string, encoding) => {
    switch (normalizeEncoding(encoding)) {
      case "hex":
        return hexDecode(string);
      case "base64":
      case "base64url":
        return base64Decode(string);
      case "utf16le":
        return ucs2Encode(string);
      case "latin1":
      case "ascii":
        return latin1Encode(string);
      default:
        return utf8Encode(string);
    }
  };
  const isAscii = (input) => op_binding_is_ascii(toBytes(input));
  const isUtf8 = (input) => op_binding_is_utf8(toBytes(input));

  const swap = (bytes, size) => op_binding_swap(bytes, size);
  const asciiWhitespace = new Uint8Array(256);
  for (const code of [0x09, 0x0a, 0x0c, 0x0d, 0x20]) asciiWhitespace[code] = 1;
  const atob = (string) => {
    let payload = "";
    for (let index = 0; index < string.length; index += 1) {
      const code = string.charCodeAt(index);
      if (code < 256 && asciiWhitespace[code] === 1) continue;
      payload += string[index];
    }
    if (payload.length % 4 === 1) return -1;
    for (let index = 0; index < payload.length; index += 1) {
      const code = payload.charCodeAt(index);
      if (code === 0x3d) break;
      if (code > 255 || base64Value[code] === 0xff) return -2;
    }
    const decoded = base64Decode(payload);
    return latin1Slice(decoded, 0, decoded.length);
  };

  const btoa = (string) => {
    const bytes = new Uint8Array(string.length);
    for (let index = 0; index < string.length; index += 1) {
      const code = string.charCodeAt(index);
      if (code > 0xff) return -1;
      bytes[index] = code;
    }
    return base64Encode(bytes, 0, bytes.length, false);
  };

  module.exports = {
    arrayBufferAlignedOffset: () => 0,
    asciiSlice,
    asciiWriteStatic: (buffer, string, offset, length) =>
      latin1WriteInto(buffer, offset, length, string),
    atob,
    base64Slice: (buffer, start, end) =>
      base64Encode(toBytes(buffer), start, end, false),
    base64urlSlice: (buffer, start, end) =>
      base64Encode(toBytes(buffer), start, end, true),
    base64Write: writeBase64,
    base64urlWrite: writeBase64,
    btoa,
    byteLengthUtf8: (string) => utf8Length(string),
    compare,
    compareOffset,
    copy,
    createUnsafeArrayBuffer: (size) => new ArrayBuffer(size),
    fill,
    hexSlice,
    hexWrite,
    indexOfBuffer,
    indexOfNumber,
    indexOfString,
    isAscii,
    isLatin1: (string) => op_binding_is_latin1(string),
    isUtf8,
    kMaxLength,
    kStringMaxLength,
    latin1Slice,
    latin1WriteStatic: (buffer, string, offset, length) =>
      latin1WriteInto(buffer, offset, length, string),
    setDetachKey: () => {},
    swap16: (buffer) => swap(buffer, 2),
    swap32: (buffer) => swap(buffer, 4),
    stringLengthUtf8: (buffer) => op_binding_string_length_utf8(toBytes(buffer)),
    swap64: (buffer) => swap(buffer, 8),
    ucs2Slice,
    ucs2Write: (buffer, string, offset, length) =>
      ucs2WriteInto(buffer, offset, length, string),
    utf8Slice,
    utf8WriteStatic: (buffer, string, offset, length) =>
      utf8WriteInto(buffer, offset, length, string),
  };
})();
