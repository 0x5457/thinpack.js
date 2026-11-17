'use strict';
// Adapted from Node.js `test/common/fixtures.js` (MIT).
// Port of Node.js `test/common/fixtures.js`, limited to a local `test/fixtures`
// tree. Fixture files are copied from the Node.js checkout only when a ported
// test needs them.

const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');

const fixturesDir = path.join(__dirname, '..', 'fixtures');

function fixturesPath(...args) {
  args.unshift(fixturesDir);
  return Reflect.apply(path.join, this, args);
}

function fixturesFileURL(...args) {
  return pathToFileURL(Reflect.apply(fixturesPath, this, args));
}

function readFixtureSync(args, enc) {
  if (Array.isArray(args)) {
    return fs.readFileSync(Reflect.apply(fixturesPath, this, args), enc);
  }
  return fs.readFileSync(fixturesPath(args), enc);
}

module.exports = {
  fixturesDir,
  path: fixturesPath,
  fileURL: fixturesFileURL,
  readSync: readFixtureSync,
};
