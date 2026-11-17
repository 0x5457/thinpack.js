'use strict';
// Minimal stand-in for Node's test/common harness, covering the helpers the ported
// cluster tests use. Node's own test/common pulls in far more of the runtime than the
// cluster tests need.

const assert = require('assert');

const checks = [];

function record(label, fn, minimum, maximum) {
  let count = 0;
  const wrapped = function(...args) {
    count += 1;
    if (count > maximum) {
      throw new Error(`${label} called more than ${maximum} time(s)`);
    }
    return fn?.apply(this, args);
  };
  checks.push(() => {
    assert(count >= minimum,
           `${label} called ${count} time(s), expected at least ${minimum}`);
    assert(count <= maximum,
           `${label} called ${count} time(s), expected at most ${maximum}`);
  });
  return wrapped;
}

function mustCall(fn, exact = 1) {
  if (typeof fn === 'number') {
    exact = fn;
    fn = undefined;
  }
  return record(fn?.name || 'mustCall', fn, exact, exact);
}

function mustCallAtLeast(fn, minimum = 1) {
  if (typeof fn === 'number') {
    minimum = fn;
    fn = undefined;
  }
  return record(fn?.name || 'mustCallAtLeast', fn, minimum, Number.MAX_SAFE_INTEGER);
}
const mustSucceed = (fn) =>
  mustCall((error, ...args) => {
    assert.ifError(error);
    return fn?.(...args);
  });

const mustNotCall = (message) =>
  record(message || 'mustNotCall', () => {
    throw new Error(message || 'function should not be called');
  }, 0, 0);

process.on('exit', () => {
  for (const check of checks) check();
});

function isAlive(pid) {
  try {
    process.kill(pid, 'SIGCONT');
    return true;
  } catch {
    return false;
  }
}

module.exports = {
  mustCall,
  mustCallAtLeast,
  mustNotCall,
  mustSucceed,
  isAlive,
  localhostIPv4: '127.0.0.1',
  platform: process.platform,
  isLinux: process.platform === 'linux',
  isMacOS: process.platform === 'darwin',
  isWindows: process.platform === 'win32',
  enoughTestMem: () => true,
  skip: (message) => {
    console.log(`1..0 # Skipped: ${message}`);
    process.exit(0);
  },
};
