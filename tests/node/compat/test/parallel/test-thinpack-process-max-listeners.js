'use strict';

const assert = require('node:assert/strict');
const events = require('node:events');

const { EventEmitter } = events;
const initial = EventEmitter.defaultMaxListeners;

// An unset limit follows the live EventEmitter default, as it does for Node's
// EventEmitter-based process.
assert.equal(process.getMaxListeners(), initial);
EventEmitter.defaultMaxListeners = 42;
assert.equal(process.getMaxListeners(), 42);
assert.equal(events.getMaxListeners(process), 42);
EventEmitter.defaultMaxListeners = initial;

// Both setters share the process object's `_maxListeners` state.
assert.equal(process.setMaxListeners(7), process);
assert.equal(process.getMaxListeners(), 7);
assert.equal(events.getMaxListeners(process), 7);

events.setMaxListeners(5, process);
assert.equal(process.getMaxListeners(), 5);
assert.equal(events.getMaxListeners(process), 5);

// 0 disables the limit instead of falling back to the default.
process.setMaxListeners(0);
assert.equal(process.getMaxListeners(), 0);
assert.equal(events.getMaxListeners(process), 0);

// Validation is EventEmitter's: a number >= 0, fractions included.
for (const value of [-1, NaN]) {
  assert.throws(() => process.setMaxListeners(value), {
    code: 'ERR_OUT_OF_RANGE',
    name: 'RangeError',
  });
}
for (const value of ['3', null, undefined]) {
  assert.throws(() => process.setMaxListeners(value), {
    code: 'ERR_INVALID_ARG_TYPE',
    name: 'TypeError',
  });
}
assert.equal(process.setMaxListeners(1.5).getMaxListeners(), 1.5);
