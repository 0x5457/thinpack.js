// Vendored from Node.js v26.13.1 lib/internal/util/trace_sigint.js (MIT, https://github.com/nodejs/node).
// Patched: require("internal/...") -> require("node:thinpack/internal/..."). worker_threads is loaded lazily so util can run without that capability.

'use strict';

let isMainThread;
function currentIsMainThread() {
  if (isMainThread === undefined) {
    try {
      isMainThread = require('worker_threads').isMainThread;
    } catch {
      isMainThread = true;
    }
  }
  return isMainThread;
}
const {
  ERR_WORKER_UNSUPPORTED_OPERATION,
} = require('node:thinpack/internal/errors').codes;

let sigintWatchdog;
function getSigintWatchdog() {
  if (!sigintWatchdog) {
    const { SigintWatchdog } = require('node:thinpack/internal/watchdog');
    sigintWatchdog = new SigintWatchdog();
  }
  return sigintWatchdog;
}

function setTraceSigInt(enable) {
  if (!currentIsMainThread())
    throw new ERR_WORKER_UNSUPPORTED_OPERATION('Calling util.setTraceSigInt');
  if (enable) {
    getSigintWatchdog().start();
  } else {
    getSigintWatchdog().stop();
  }
};

module.exports = {
  setTraceSigInt,
};
