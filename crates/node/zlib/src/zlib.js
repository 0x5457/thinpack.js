(() => {
  const { Buffer } = require("node:buffer");
  const { Transform } = require("node:stream");
  const { codes, opError } = require("node:thinpack/internal/errors");
  const { zlib: constants } = require("node:thinpack/internal/constants");

  const notImplemented = (name) => new codes.ERR_NOT_IMPLEMENTED(`zlib.${name}`);

  const levelOf = (options) => {
    if (typeof options === "number") return options;
    return options?.level ?? constants.Z_DEFAULT_COMPRESSION;
  };

  const toBuffer = (data) => {
    if (typeof data === "string") return Buffer.from(data);
    if (ArrayBuffer.isView(data)) {
      return Buffer.from(data.buffer, data.byteOffset, data.byteLength);
    }
    return Buffer.from(data);
  };

  const zstdLevel = (options) => {
    const params = options?.params ?? {};
    return params[constants.ZSTD_c_compressionLevel] ?? constants.ZSTD_CLEVEL_DEFAULT;
  };

  const brotliParams = (options) => {
    const params = options?.params ?? {};
    return {
      quality: params[constants.BROTLI_PARAM_QUALITY] ?? constants.BROTLI_DEFAULT_QUALITY,
      lgwin: params[constants.BROTLI_PARAM_LGWIN] ?? constants.BROTLI_DEFAULT_WINDOW,
      mode: params[constants.BROTLI_PARAM_MODE] ?? constants.BROTLI_MODE_GENERIC,
    };
  };

  class ZlibTransform extends Transform {
    #format;
    #level;
    #brotli;
    #zstd;
    #chunks;

    constructor(format, options) {
      super();
      this.#format = format;
      this.#level = levelOf(options);
      this.#brotli = brotliParams(options);
      this.#zstd = zstdLevel(options);
      this.#chunks = [];
    }

    _transform(chunk, encoding, callback) {
      this.#chunks.push(Buffer.from(chunk));
      callback();
    }

    _flush(callback) {
      const input = Buffer.concat(this.#chunks);
      try {
        let output;
        if (this.#format === "brotli") {
          const { quality, lgwin, mode } = this.#brotli;
          output = Deno.core.ops.op_zlib_brotli_compress(input, quality, lgwin, mode);
        } else if (this.#format === "unbrotli") {
          output = Deno.core.ops.op_zlib_brotli_decompress(input);
        } else if (this.#format === "zstd") {
          output = Deno.core.ops.op_zlib_zstd_compress(input, this.#zstd);
        } else if (this.#format === "unzstd") {
          output = Deno.core.ops.op_zlib_zstd_decompress(input);
        } else if (DECOMPRESS.has(this.#format)) {
          output = Deno.core.ops.op_zlib_decompress(opFormat(this.#format, input), input);
        } else {
          output = Deno.core.ops.op_zlib_compress(this.#format, input, this.#level);
        }
        callback(null, Buffer.from(output));
      } catch (error) {
        callback(opError(error));
      }
    }
  }

  const detect = (buffer) =>
    buffer.length >= 2 && buffer[0] === 0x1f && buffer[1] === 0x8b ? "gzip" : "deflate";

  const DECOMPRESS = new Set(["ungzip", "inflate", "inflate-raw", "unzip"]);
  const opFormat = (format, input) => {
    if (format === "ungzip") return "gzip";
    if (format === "unzip") return detect(input);
    if (format === "inflate") return "deflate";
    if (format === "inflate-raw") return "deflate-raw";
    return format;
  };

  const sync = (format, data) => {
    const input = toBuffer(data);
    try {
      if (DECOMPRESS.has(format)) {
        return Buffer.from(Deno.core.ops.op_zlib_decompress(opFormat(format, input), input));
      }
      return Buffer.from(Deno.core.ops.op_zlib_compress(format, input, constants.Z_DEFAULT_COMPRESSION));
    } catch (error) {
      throw opError(error);
    }
  };

  const brotliSync = (decompress, data, options) => {
    const input = toBuffer(data);
    try {
      if (decompress) {
        return Buffer.from(Deno.core.ops.op_zlib_brotli_decompress(input));
      }
      const { quality, lgwin, mode } = brotliParams(options);
      return Buffer.from(Deno.core.ops.op_zlib_brotli_compress(input, quality, lgwin, mode));
    } catch (error) {
      throw opError(error);
    }
  };

  const zstdSync = (decompress, data, options) => {
    const input = toBuffer(data);
    try {
      if (decompress) return Buffer.from(Deno.core.ops.op_zlib_zstd_decompress(input));
      return Buffer.from(Deno.core.ops.op_zlib_zstd_compress(input, zstdLevel(options)));
    } catch (error) {
      throw opError(error);
    }
  };

  const zstdCallback = (decompress, data, options, callback) => {
    if (typeof options === "function") {
      callback = options;
      options = {};
    }
    queueMicrotask(() => {
      try {
        callback(null, zstdSync(decompress, data, options));
      } catch (error) {
        callback(opError(error));
      }
    });
  };

  const brotliCallback = (decompress, data, options, callback) => {
    if (typeof options === "function") {
      callback = options;
      options = {};
    }
    queueMicrotask(() => {
      try {
        callback(null, brotliSync(decompress, data, options));
      } catch (error) {
        callback(opError(error));
      }
    });
  };

  const callbackStyle = (format, data, options, callback) => {
    if (typeof options === "function") {
      callback = options;
      options = {};
    }
    queueMicrotask(() => {
      try {
        if (DECOMPRESS.has(format)) {
          callback(null, sync(format, data));
          return;
        }
        callback(null, Buffer.from(Deno.core.ops.op_zlib_compress(format, toBuffer(data), levelOf(options))));
      } catch (error) {
        callback(opError(error));
      }
    });
  };

  const stream = (format) => (options) => new ZlibTransform(format, options);

  const crcTable = new Uint32Array(256);
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    crcTable[index] = value >>> 0;
  }

  const crc32 = (data, value = 0) => {
    let crc = (Number(value) ^ 0xffffffff) >>> 0;
    for (const byte of toBuffer(data)) {
      crc = (crc >>> 8) ^ crcTable[(crc ^ byte) & 0xff];
    }
    return (crc ^ 0xffffffff) >>> 0;
  };

  module.exports = {
    constants,
    createGzip: stream("gzip"),
    createGunzip: stream("ungzip"),
    createDeflate: stream("deflate"),
    createInflate: stream("inflate"),
    createDeflateRaw: stream("deflate-raw"),
    createInflateRaw: stream("inflate-raw"),
    createUnzip: stream("unzip"),
    createBrotliCompress: stream("brotli"),
    createBrotliDecompress: stream("unbrotli"),
    createZstdCompress: stream("zstd"),
    createZstdDecompress: stream("unzstd"),
    gzip: (data, options, cb) => callbackStyle("gzip", data, options, cb),
    gunzip: (data, options, cb) => callbackStyle("ungzip", data, options, cb),
    deflate: (data, options, cb) => callbackStyle("deflate", data, options, cb),
    inflate: (data, options, cb) => callbackStyle("inflate", data, options, cb),
    deflateRaw: (data, options, cb) => callbackStyle("deflate-raw", data, options, cb),
    inflateRaw: (data, options, cb) => callbackStyle("inflate-raw", data, options, cb),
    unzip: (data, options, cb) => callbackStyle("unzip", data, options, cb),
    brotliCompress: (data, options, cb) => brotliCallback(false, data, options, cb),
    brotliDecompress: (data, options, cb) => brotliCallback(true, data, options, cb),
    gzipSync: (data) => sync("gzip", data),
    gunzipSync: (data) => sync("ungzip", data),
    deflateSync: (data) => sync("deflate", data),
    inflateSync: (data) => sync("inflate", data),
    deflateRawSync: (data) => sync("deflate-raw", data),
    inflateRawSync: (data) => sync("inflate-raw", data),
    unzipSync: (data) => sync("unzip", data),
    brotliCompressSync: (data, options) => brotliSync(false, data, options),
    brotliDecompressSync: (data, options) => brotliSync(true, data, options),
    zstdCompress: (data, options, cb) => zstdCallback(false, data, options, cb),
    zstdDecompress: (data, options, cb) => zstdCallback(true, data, options, cb),
    crc32,
    zstdCompressSync: (data, options) => zstdSync(false, data, options),
    zstdDecompressSync: (data, options) => zstdSync(true, data, options),
  };
})();
