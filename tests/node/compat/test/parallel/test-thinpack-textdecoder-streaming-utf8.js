'use strict';

const assert = require('node:assert/strict');

const decoder = new TextDecoder('utf-8');
assert.equal(decoder.decode(Uint8Array.of(0xe2), { stream: true }), '');
assert.equal(decoder.decode(Uint8Array.of(0x82, 0xac), { stream: true }), '€');
assert.equal(decoder.decode(), '');
