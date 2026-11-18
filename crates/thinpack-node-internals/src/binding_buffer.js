(() => {
  const { normalizeEncoding } = require("node:thinpack/internal/util");
  const kMaxLength = 9007199254740991;
  const kStringMaxLength = 536870888;

  const HEX_DIGITS = "0123456789abcdef";
  const BASE64_DIGITS =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const BASE64_URL_DIGITS =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  const ASCII = 0;
  const UTF8 = 1;
  const UCS2 = 3;
  const LATIN1 = 4;

  const toBytes = (input) =>
    input instanceof ArrayBuffer ? new Uint8Array(input) : input;
  const {
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
    op_binding_utf8_slice(toBytes(bytes), start, end);
  const latin1WriteInto = (target, offset, maxBytes, string) => {
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
  const hexValue = (code) => {
    if (code >= 0x30 && code <= 0x39) return code - 0x30;
    if (code >= 0x41 && code <= 0x46) return code - 0x37;
    if (code >= 0x61 && code <= 0x66) return code - 0x57;
    return -1;
  };
  const hexDecode = (string) => {
    const bytes = new Uint8Array(string.length >>> 1);
    let written = 0;
    for (; written * 2 + 1 < string.length; written += 1) {
      const high = hexValue(string.charCodeAt(written * 2));
      const low = hexValue(string.charCodeAt(written * 2 + 1));
      if (high < 0 || low < 0) break;
      bytes[written] = (high << 4) | low;
    }
    return bytes.subarray(0, written);
  };

  const hexSlice = (bytes, start, end) => {
    let result = "";
    for (let index = start; index < end; index += 1) {
      result += HEX_DIGITS[bytes[index] >> 4] + HEX_DIGITS[bytes[index] & 0x0f];
    }
    return result;
  };
  const base64Value = new Uint8Array(256).fill(0xff);
  for (let index = 0; index < 64; index += 1) {
    base64Value[BASE64_DIGITS.charCodeAt(index)] = index;
    base64Value[BASE64_URL_DIGITS.charCodeAt(index)] = index;
  }

  const base64Decode = (string) => {
    const bytes = new Uint8Array((string.length * 3) >>> 2);
    let written = 0;
    let accumulator = 0;
    let bits = 0;
    for (let index = 0; index < string.length; index += 1) {
      const code = string.charCodeAt(index);
      if (code === 0x3d) break;
      const value = code < 256 ? base64Value[code] : 0xff;
      if (value === 0xff) continue;
      accumulator = ((accumulator << 6) | value) & 0xffffff;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        bytes[written] = (accumulator >> bits) & 0xff;
        written += 1;
      }
    }
    return bytes.subarray(0, written);
  };

  const base64Encode = (bytes, start, end, digits) => {
    let result = "";
    let index = start;
    for (; index + 3 <= end; index += 3) {
      const chunk = (bytes[index] << 16) | (bytes[index + 1] << 8) | bytes[index + 2];
      result +=
        digits[(chunk >> 18) & 0x3f] +
        digits[(chunk >> 12) & 0x3f] +
        digits[(chunk >> 6) & 0x3f] +
        digits[chunk & 0x3f];
    }
    const remaining = end - index;
    if (remaining === 1) {
      const chunk = bytes[index] << 16;
      result += digits[(chunk >> 18) & 0x3f] + digits[(chunk >> 12) & 0x3f];
      result += digits === BASE64_DIGITS ? "==" : "";
    } else if (remaining === 2) {
      const chunk = (bytes[index] << 16) | (bytes[index + 1] << 8);
      result +=
        digits[(chunk >> 18) & 0x3f] +
        digits[(chunk >> 12) & 0x3f] +
        digits[(chunk >> 6) & 0x3f];
      result += digits === BASE64_DIGITS ? "=" : "";
    }
    return result;
  };
  const utf8Slice = (buffer, start, end) => utf8Decode(toBytes(buffer), start, end);

  const asciiSlice = (buffer, start, end) => {
    const bytes = toBytes(buffer);
    let result = "";
    for (let index = start; index < end; index += 1) {
      result += String.fromCharCode(bytes[index] & 0x7f);
    }
    return result;
  };

  const latin1Slice = (buffer, start, end) => {
    const bytes = toBytes(buffer);
    let result = "";
    for (let index = start; index < end; index += 1) {
      result += String.fromCharCode(bytes[index]);
    }
    return result;
  };

  const ucs2Slice = (buffer, start, end) => {
    const bytes = toBytes(buffer);
    let result = "";
    for (let index = start; index + 1 < end; index += 2) {
      result += String.fromCharCode(bytes[index] | (bytes[index + 1] << 8));
    }
    return result;
  };

  const writeBase64 = (digits) => (buffer, string, offset, length) => {
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
  const compareBytes = (left, leftStart, right, rightStart, length) => {
    for (let index = 0; index < length; index += 1) {
      const difference = left[leftStart + index] - right[rightStart + index];
      if (difference !== 0) return difference;
    }
    return 0;
  };

  const normalizeCompare = (value, leftLength, rightLength) => {
    if (value !== 0) return value > 0 ? 1 : -1;
    if (leftLength === rightLength) return 0;
    return leftLength > rightLength ? 1 : -1;
  };

  const compare = (left, right) =>
    normalizeCompare(
      compareBytes(left, 0, right, 0, Math.min(left.length, right.length)),
      left.length,
      right.length,
    );

  const compareOffset = (source, target, targetStart, sourceStart, targetEnd, sourceEnd) => {
    const length = Math.min(
      Math.min(sourceEnd - sourceStart, targetEnd - targetStart),
      source.length - sourceStart,
    );
    const value =
      length > 0 ? compareBytes(source, sourceStart, target, targetStart, length) : 0;
    return normalizeCompare(value, sourceEnd - sourceStart, targetEnd - targetStart);
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
    const matchesAt = (position) => {
      for (let index = 0; index < needleLength; index += 1) {
        if (haystack[position + index] !== needle[index]) return false;
      }
      return true;
    };

    if (forward) {
      for (let position = from; position <= last; position += unit) {
        if (matchesAt(position)) return position;
      }
      return -1;
    }
    for (let position = Math.min(from, last); position >= 0; position -= unit) {
      if (matchesAt(position)) return position;
    }
    return -1;
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
    const needle = value & 0xff;
    const start = indexOfOffset(haystack.length, offset, 1, forward);
    if (start <= -1 || haystack.length === 0) return -1;
    const searchEnd = Math.min(Math.max(end, 0), haystack.length);
    if (forward) {
      for (let index = start; index < searchEnd; index += 1) {
        if (haystack[index] === needle) return index;
      }
      return -1;
    }
    const backwardEnd = Math.min(start + 1, searchEnd);
    for (let index = backwardEnd - 1; index >= 0; index -= 1) {
      if (haystack[index] === needle) return index;
    }
    return -1;
  };
  const isAscii = (input) => {
    const bytes = toBytes(input);
    for (let index = 0; index < bytes.length; index += 1) {
      if (bytes[index] > 0x7f) return false;
    }
    return true;
  };
  const isUtf8 = (input) => {
    const bytes = toBytes(input);
    let index = 0;
    while (index < bytes.length) {
      const first = bytes[index];
      let needed;
      let lower = 0x80;
      let upper = 0xbf;
      if (first < 0x80) {
        index += 1;
        continue;
      } else if (first >= 0xc2 && first <= 0xdf) {
        needed = 1;
      } else if (first >= 0xe0 && first <= 0xef) {
        needed = 2;
        if (first === 0xe0) lower = 0xa0;
        if (first === 0xed) upper = 0x9f;
      } else if (first >= 0xf0 && first <= 0xf4) {
        needed = 3;
        if (first === 0xf0) lower = 0x90;
        if (first === 0xf4) upper = 0x8f;
      } else {
        return false;
      }
      index += 1;
      for (let consumed = 0; consumed < needed; consumed += 1) {
        const next = bytes[index];
        if (index >= bytes.length || next < lower || next > upper) return false;
        index += 1;
        lower = 0x80;
        upper = 0xbf;
      }
    }
    return true;
  };

  const swap = (bytes, size) => {
    for (let index = 0; index + size <= bytes.length; index += size) {
      for (let low = 0, high = size - 1; low < high; low += 1, high -= 1) {
        const held = bytes[index + low];
        bytes[index + low] = bytes[index + high];
        bytes[index + high] = held;
      }
    }
  };
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
    return base64Encode(bytes, 0, bytes.length, BASE64_DIGITS);
  };

  module.exports = {
    arrayBufferAlignedOffset: () => 0,
    asciiSlice,
    asciiWriteStatic: (buffer, string, offset, length) =>
      latin1WriteInto(buffer, offset, length, string),
    atob,
    base64Slice: (buffer, start, end) =>
      base64Encode(toBytes(buffer), start, end, BASE64_DIGITS),
    base64urlSlice: (buffer, start, end) =>
      base64Encode(toBytes(buffer), start, end, BASE64_URL_DIGITS),
    base64Write: writeBase64(BASE64_DIGITS),
    base64urlWrite: writeBase64(BASE64_URL_DIGITS),
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
    isUtf8,
    kMaxLength,
    kStringMaxLength,
    latin1Slice,
    latin1WriteStatic: (buffer, string, offset, length) =>
      latin1WriteInto(buffer, offset, length, string),
    setDetachKey: () => {},
    swap16: (buffer) => swap(buffer, 2),
    swap32: (buffer) => swap(buffer, 4),
    swap64: (buffer) => swap(buffer, 8),
    ucs2Slice,
    ucs2Write: (buffer, string, offset, length) =>
      ucs2WriteInto(buffer, offset, length, string),
    utf8Slice,
    utf8WriteStatic: (buffer, string, offset, length) =>
      utf8WriteInto(buffer, offset, length, string),
  };
})();
