// Vendored from Node.js v26.13.1 lib/stream/web.js (MIT, https://github.com/nodejs/node).
// Patched: require("internal/...") -> require("node:thinpack/internal/...").

'use strict';

const {
  TransformStream,
  TransformStreamDefaultController,
} = require('node:thinpack/internal/webstreams/transformstream');

const {
  WritableStream,
  WritableStreamDefaultController,
  WritableStreamDefaultWriter,
} = require('node:thinpack/internal/webstreams/writablestream');

const {
  ReadableStream,
  ReadableStreamDefaultReader,
  ReadableStreamBYOBReader,
  ReadableStreamBYOBRequest,
  ReadableByteStreamController,
  ReadableStreamDefaultController,
  isReadableStream,
  readableStreamTee,
} = require('node:thinpack/internal/webstreams/readablestream');

const {
  codes: {
    ERR_INVALID_ARG_TYPE,
  },
} = require('node:thinpack/internal/errors');

const {
  validateBoolean,
} = require('node:thinpack/internal/validators');

const {
  ByteLengthQueuingStrategy,
  CountQueuingStrategy,
} = require('node:thinpack/internal/webstreams/queuingstrategies');

const {
  TextEncoderStream,
  TextDecoderStream,
} = require('node:thinpack/internal/webstreams/encoding');

const {
  CompressionStream,
  DecompressionStream,
} = require('node:thinpack/internal/webstreams/compression');

function ReadableStreamTee(stream, cloneForBranch2 = false) {
  if (!isReadableStream(stream)) {
    throw new ERR_INVALID_ARG_TYPE('stream', 'ReadableStream', stream);
  }
  validateBoolean(cloneForBranch2, 'cloneForBranch2');
  return readableStreamTee(stream, cloneForBranch2);
}

module.exports = {
  ReadableStream,
  ReadableStreamTee,
  ReadableStreamDefaultReader,
  ReadableStreamBYOBReader,
  ReadableStreamBYOBRequest,
  ReadableByteStreamController,
  ReadableStreamDefaultController,
  TransformStream,
  TransformStreamDefaultController,
  WritableStream,
  WritableStreamDefaultWriter,
  WritableStreamDefaultController,
  ByteLengthQueuingStrategy,
  CountQueuingStrategy,
  TextEncoderStream,
  TextDecoderStream,
  CompressionStream,
  DecompressionStream,
};
