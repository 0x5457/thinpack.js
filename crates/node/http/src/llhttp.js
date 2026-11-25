// One HTTP/1.1 parser for both sides, driven by the llhttp build undici already carries.
//
// `vendor/llhttp-wasm.js` (generated from the vendored undici bundle) holds the wasm binary
// of llhttp, the parser Node.js itself uses (MIT, https://github.com/nodejs/llhttp). llhttp
// owns the protocol: framing, chunked decoding, header syntax, `Transfer-Encoding` and
// keep-alive. This module only binds its callbacks to plain JS objects.
'use strict';

const wasmBase64 = require('node:thinpack/internal/llhttp-wasm');

const REQUEST = 1;
const RESPONSE = 2;

const OK = 0;
const PAUSED_UPGRADE = 22;

// A protocol error, as opposed to an exception thrown by a handler while it runs.
class ParseError extends Error {}

// One wasm instance per process. llhttp's parser structs and the scratch buffer live in
// its linear memory, and its callbacks reach back through module scope.
let engine = null;
let active = null;
let chunk = null;
let chunkBase = 0;
let scratchPtr = 0;
let scratchSize = 0;
let scratchView = null;

const readCString = (pointer) => {
  const memory = new Uint8Array(engine.memory.buffer);
  let end = pointer;
  while (memory[end] !== 0) end += 1;
  return Buffer.from(memory.subarray(pointer, end)).toString('latin1');
};

// Header and body callbacks receive wasm addresses; the bytes they describe are the input
// chunk that llhttp is executing, so a view over it avoids copying the payload out again.
const pieceOf = (address, length) =>
  chunk.subarray(address - chunkBase, address - chunkBase + length);

const load = () => {
  if (engine !== null) return engine;
  engine = new WebAssembly.Instance(new WebAssembly.Module(Buffer.from(wasmBase64, 'base64')), {
    env: {
      wasm_on_message_begin: () => active.onMessageBegin(),
      wasm_on_url: (parser, address, length) => active.onUrl(pieceOf(address, length)),
      wasm_on_status: (parser, address, length) => active.onStatus(pieceOf(address, length)),
      wasm_on_header_field: (parser, address, length) =>
        active.onHeaderField(pieceOf(address, length)),
      wasm_on_header_value: (parser, address, length) =>
        active.onHeaderValue(pieceOf(address, length)),
      wasm_on_headers_complete: (parser, statusCode, upgrade, keepAlive) =>
        active.onHeadersComplete(statusCode, upgrade === 1, keepAlive === 1),
      wasm_on_body: (parser, address, length) => active.onBody(pieceOf(address, length)),
      wasm_on_message_complete: () => active.onMessageComplete(),
    },
  }).exports;
  return engine;
};

class Parser {
  #handlers;
  #pointer;
  #maxHeaderSize;
  #headerSize = 0;
  #failure = null;

  constructor(type, handlers, maxHeaderSize) {
    load();
    this.#handlers = handlers;
    this.#maxHeaderSize = maxHeaderSize;
    this.#pointer = engine.llhttp_alloc(type);
  }

  get method() {
    return readCString(engine.llhttp_method_name(engine.llhttp_get_method(this.#pointer)));
  }

  get version() {
    return `${engine.llhttp_get_http_major(this.#pointer)}.${engine.llhttp_get_http_minor(this.#pointer)}`;
  }

  get keepAlive() {
    return engine.llhttp_should_keep_alive(this.#pointer) === 1;
  }

  /// Feeds one socket chunk. Returns the bytes past an upgrade the parser stopped at.
  execute(data) {
    if (data.length > scratchSize) {
      if (scratchPtr !== 0) engine.free(scratchPtr);
      scratchSize = Math.ceil(data.length / 4096) * 4096;
      scratchPtr = engine.malloc(scratchSize);
      scratchView = null;
    }
    if (scratchView === null || scratchView.buffer !== engine.memory.buffer) {
      scratchView = new Uint8Array(engine.memory.buffer, scratchPtr, scratchSize);
    }
    scratchView.set(data);
    return this.#run(() => engine.llhttp_execute(this.#pointer, scratchPtr, data.length), data);
  }

  /// Reports the end of the connection: completes a body delimited by EOF, or fails when
  /// the peer stopped inside a body it announced a length for.
  finish() {
    this.#run(() => engine.llhttp_finish(this.#pointer), null);
  }

  destroy() {
    if (this.#pointer === 0) return;
    engine.llhttp_free(this.#pointer);
    this.#pointer = 0;
  }

  #run(parse, data) {
    active = this;
    chunk = data ?? EMPTY;
    chunkBase = scratchPtr;
    let result;
    try {
      result = parse();
    } finally {
      active = null;
      chunk = null;
    }
    if (this.#failure !== null) {
      const failure = this.#failure;
      this.#failure = null;
      throw failure;
    }
    if (result === OK) return null;
    if (result === PAUSED_UPGRADE) {
      return data.subarray(engine.llhttp_get_error_pos(this.#pointer) - scratchPtr);
    }
    throw new ParseError(
      `Parse Error: ${engine.llhttp_errno_name(result)} ` +
        readCString(engine.llhttp_get_error_reason(this.#pointer)),
    );
  }

  // A handler that throws stops llhttp; the error is rethrown once the parser returns.
  #dispatch(name, ...args) {
    try {
      const result = this.#handlers[name]?.(...args);
      return typeof result === 'number' ? result : 0;
    } catch (error) {
      this.#failure = error;
      return -1;
    }
  }

  #count(piece) {
    this.#headerSize += piece.length;
    if (this.#headerSize > this.#maxHeaderSize) {
      this.#failure = new ParseError(
        `Parse Error: header size exceeds ${this.#maxHeaderSize} bytes`,
      );
      return false;
    }
    return true;
  }

  onMessageBegin() {
    this.#headerSize = 0;
    return this.#dispatch('onMessageBegin');
  }

  onUrl(piece) {
    return this.#count(piece) ? this.#dispatch('onUrl', piece) : -1;
  }

  onStatus(piece) {
    return this.#count(piece) ? this.#dispatch('onStatus', piece) : -1;
  }

  onHeaderField(piece) {
    return this.#count(piece) ? this.#dispatch('onHeaderField', piece) : -1;
  }

  onHeaderValue(piece) {
    return this.#count(piece) ? this.#dispatch('onHeaderValue', piece) : -1;
  }

  onHeadersComplete(statusCode, upgrade, keepAlive) {
    return this.#dispatch('onHeadersComplete', statusCode, upgrade, keepAlive);
  }

  onBody(piece) {
    return this.#dispatch('onBody', piece);
  }

  onMessageComplete() {
    return this.#dispatch('onMessageComplete');
  }
}

const EMPTY = Buffer.alloc(0);

module.exports = { Parser, ParseError, REQUEST, RESPONSE };
