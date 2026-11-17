'use strict';
// Adapted from Node.js `test/common/tmpdir.js` (MIT).
// Per-process scratch directory, modelled on Node.js `test/common/tmpdir.js`.
//
// The runner assigns every test file its own `TEST_SERIAL_ID`, so files that
// run concurrently never share a directory.

const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');

const testRoot = process.env.NODE_TEST_DIR ?
  fs.realpathSync(process.env.NODE_TEST_DIR) : path.resolve(__dirname, '..');

const serial = process.env.TEST_SERIAL_ID ||
  process.env.TEST_THREAD_ID ||
  String(process.pid);
const tmpdirName = `.tmp.${serial}`;
let tmpPath = path.join(testRoot, tmpdirName);

let firstRefresh = true;

function rmSync() {
  fs.rmSync(tmpPath, { maxRetries: 3, recursive: true, force: true });
}

function refresh() {
  rmSync();
  fs.mkdirSync(tmpPath, { recursive: true });

  if (firstRefresh) {
    firstRefresh = false;
    process.on('exit', () => {
      try {
        process.chdir(testRoot);
      } catch {
        // The test already removed its working directory.
      }
      try {
        rmSync();
      } catch (error) {
        console.error('Cannot clean tmpdir:', tmpPath, error.message);
      }
    });
  }
}

function resolve(...paths) {
  return path.resolve(tmpPath, ...paths);
}

function fileURL(...paths) {
  const fullPath = path.resolve(tmpPath + path.sep, ...paths);
  return pathToFileURL(fullPath);
}

function hasEnoughSpace(size) {
  const { bavail, bsize } = fs.statfsSync(tmpPath);
  return bavail >= Math.ceil(size / bsize);
}

module.exports = {
  fileURL,
  hasEnoughSpace,
  refresh,
  resolve,

  get path() {
    return tmpPath;
  },
  set path(newPath) {
    tmpPath = path.resolve(newPath);
  },
};
