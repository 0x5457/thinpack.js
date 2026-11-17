(() => {
  const { Buffer } = require("node:buffer");

  const kIncompleteCharactersStart = 0;
  const kIncompleteCharactersEnd = 4;
  const kMissingBytes = 4;
  const kBufferedBytes = 5;
  const kEncodingField = 6;
  const kSize = 7;

  const encodings = ["ascii", "utf8", "base64", "utf16le", "latin1", "hex"];

  const reset = (state) => {
    state[kMissingBytes] = 0;
    state[kBufferedBytes] = 0;
  };
  const incompleteUtf8Tail = (bytes) => {
    const length = bytes.length;
    for (let back = 1; back <= 3 && back <= length; back += 1) {
      const byte = bytes[length - back];
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

  const decodeUtf8 = (state, buffer) => {
    const buffered = state[kBufferedBytes];
    const bytes =
      buffered === 0
        ? buffer
        : Buffer.concat([
            Buffer.from(state.subarray(kIncompleteCharactersStart, buffered)),
            buffer,
          ]);
    const tail = incompleteUtf8Tail(bytes);
    const complete = tail === 0 ? bytes.length : bytes.length - tail;
    const decoded = bytes.toString("utf8", 0, complete);
    state[kBufferedBytes] = tail;
    state[kMissingBytes] = tail === 0 ? 0 : 0;
    if (tail > 0) {
      bytes.copy(
        state,
        kIncompleteCharactersStart,
        complete,
        complete + tail,
      );
    }
    return decoded;
  };

  const decodeUtf16le = (state, buffer) => {
    const buffered = state[kBufferedBytes];
    const bytes =
      buffered === 0
        ? buffer
        : Buffer.concat([
            Buffer.from(state.subarray(kIncompleteCharactersStart, buffered)),
            buffer,
          ]);
    const tail = bytes.length & 1;
    const complete = bytes.length - tail;
    const decoded = bytes.toString("utf16le", 0, complete);
    state[kBufferedBytes] = tail;
    state[kMissingBytes] = tail;
    if (tail > 0) {
      bytes.copy(state, kIncompleteCharactersStart, complete);
    }
    return decoded;
  };

  const decode = (state, buffer) => {
    const encoding = encodings[state[kEncodingField]];
    if (encoding === undefined) return "";
    if (encoding === "utf8") return decodeUtf8(state, buffer);
    if (encoding === "utf16le") return decodeUtf16le(state, buffer);
    return Buffer.from(buffer).toString(encoding);
  };

  const flush = (state) => {
    const buffered = state[kBufferedBytes];
    if (buffered === 0) return "";
    const encoding = encodings[state[kEncodingField]];
    const bytes = Buffer.from(
      state.subarray(kIncompleteCharactersStart, buffered),
    );
    reset(state);
    return bytes.toString(encoding);
  };

  module.exports = {
    kIncompleteCharactersStart,
    kIncompleteCharactersEnd,
    kMissingBytes,
    kBufferedBytes,
    kEncodingField,
    kSize,
    decode,
    flush,
  };
})();
