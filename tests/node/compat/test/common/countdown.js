'use strict';
// Adapted from Node.js `test/common/countdown.js` (MIT).

const assert = require('assert');
const kLimit = Symbol('limit');
const kCallback = Symbol('callback');
const common = require('./');

class Countdown {
  constructor(limit, callback) {
    assert.strictEqual(typeof limit, 'number');
    assert.strictEqual(typeof callback, 'function');
    this[kLimit] = limit;
    this[kCallback] = common.mustCall(callback);
  }

  dec() {
    assert(this[kLimit] > 0, 'Countdown expired');
    if (--this[kLimit] === 0) this[kCallback]();
    return this[kLimit];
  }

  get remaining() {
    return this[kLimit];
  }
}

module.exports = Countdown;
