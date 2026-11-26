'use strict';

// Clipanion calls tty.WriteStream.prototype.getColorDepth() without a stream.
// Node's color methods read only the environment and their arguments.
const assert = require('node:assert/strict');
const { WriteStream } = require('node:tty');

const { prototype } = WriteStream;

assert.equal(prototype.getColorDepth({ FORCE_COLOR: '1' }), 4);
assert.equal(prototype.getColorDepth({ FORCE_COLOR: '2' }), 8);
assert.equal(prototype.getColorDepth({ FORCE_COLOR: '3' }), 24);
assert.equal(prototype.getColorDepth({ TERM: 'xterm-256color' }), 8);
assert.equal(prototype.getColorDepth({ TERM: 'dumb' }), 1);
assert.equal(prototype.getColorDepth({ NO_COLOR: '1' }), 1);

assert.equal(prototype.hasColors(16, { FORCE_COLOR: '1' }), true);
assert.equal(prototype.hasColors(256, { FORCE_COLOR: '2' }), true);
assert.equal(prototype.hasColors(256, { FORCE_COLOR: '1' }), false);
assert.equal(prototype.hasColors({ FORCE_COLOR: '3' }), true);
assert.equal(prototype.hasColors({ FORCE_COLOR: '0' }), false);

// An omitted environment falls back to process.env without a receiver.
assert.equal(prototype.getColorDepth(), prototype.getColorDepth(process.env));
assert.equal(prototype.hasColors(), prototype.hasColors(process.env));

assert.equal(prototype.getColorDepth.length, 0);
assert.equal(prototype.hasColors.length, 2);

assert.throws(() => prototype.hasColors(1), { code: 'ERR_OUT_OF_RANGE' });
assert.throws(() => prototype.hasColors('16'), { code: 'ERR_INVALID_ARG_TYPE' });
