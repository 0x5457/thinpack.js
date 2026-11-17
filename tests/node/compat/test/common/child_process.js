'use strict';
// Adapted from Node.js `test/common/child_process.js` (MIT).
// Port of the part of Node.js `test/common/child_process.js` used by the
// ported tests.

const assert = require('assert');
const { spawnSync } = require('child_process');

function checkOutput(actual, expected) {
  if (expected instanceof RegExp) return expected.test(actual);
  if (typeof expected === 'function') {
    expected(actual);
    return true;
  }
  return actual === expected;
}

function expectSyncExit(caller, spawnArgs, expectations) {
  const child = spawnSync(...spawnArgs);
  const failures = [];

  if (expectations.status !== undefined && child.status !== expectations.status) {
    failures.push(`process terminated with status ${child.status}, expected ${expectations.status}`);
  }
  if (expectations.signal !== undefined && child.signal !== expectations.signal) {
    failures.push(`process terminated with signal ${child.signal}, expected ${expectations.signal}`);
  }

  for (const stream of ['stdout', 'stderr']) {
    const expected = expectations[stream];
    if (expected === undefined) continue;
    const actual = child[stream]?.toString() ?? '';
    if (!checkOutput(expectations.trim ? actual.trim() : actual, expected)) {
      failures.push(`${stream} did not match ${expected}`);
    }
  }

  if (failures.length !== 0) {
    const error = new Error(failures.join('\n'));
    error.stdout = child.stdout?.toString();
    error.stderr = child.stderr?.toString();
    Error.captureStackTrace(error, caller);
    throw error;
  }
  return child;
}

function spawnSyncAndExit(...args) {
  const expectations = args.pop();
  return expectSyncExit(spawnSyncAndExit, args, expectations);
}

function spawnSyncAndExitWithoutError(...args) {
  return expectSyncExit(spawnSyncAndExitWithoutError, args, {
    status: 0,
    signal: null,
  });
}

function spawnSyncAndAssert(...args) {
  const expectations = args.pop();
  return expectSyncExit(spawnSyncAndAssert, args, {
    status: 0,
    signal: null,
    ...expectations,
  });
}

module.exports = {
  spawnSyncAndAssert,
  spawnSyncAndExit,
  spawnSyncAndExitWithoutError,
};
