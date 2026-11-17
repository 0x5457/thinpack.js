// Vendored from Node.js v26.13.1 lib/internal/assert.js (MIT, https://github.com/nodejs/node).
// Patched: require("internal/...") -> require("node:thinpack/internal/...").

'use strict';

let error;
function lazyError() {
  return error ??= require('node:thinpack/internal/errors').codes.ERR_INTERNAL_ASSERTION;
}

function assert(value, message) {
  if (!value) {
    const ERR_INTERNAL_ASSERTION = lazyError();
    throw new ERR_INTERNAL_ASSERTION(message);
  }
}

function fail(message) {
  const ERR_INTERNAL_ASSERTION = lazyError();
  throw new ERR_INTERNAL_ASSERTION(message);
}

assert.fail = fail;

module.exports = assert;
