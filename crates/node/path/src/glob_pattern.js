'use strict';

const {
  SafeMap,
} = primordials;

const {
  kEmptyObject,
  isWindows,
  isMacOS,
} = require('node:thinpack/internal/util');
const { validateString } = require('node:thinpack/internal/validators');

let minimatch;
function lazyMinimatch() {
  minimatch ??= require('node:thinpack/internal/deps/minimatch/index');
  return minimatch;
}

function createMatcher(pattern, options = kEmptyObject) {
  const opts = {
    __proto__: null,
    nocase: isWindows || isMacOS,
    windowsPathsNoEscape: true,
    nonegate: true,
    nocomment: true,
    optimizationLevel: 2,
    platform: process.platform,
    nocaseMagicOnly: true,
    ...options,
  };
  return new (lazyMinimatch().Minimatch)(pattern, opts);
}

const kGlobPatternCacheLimit = 250;
const patternFnCache = new SafeMap();

function matchGlobPattern(path, pattern, windows = isWindows) {
  validateString(path, 'path');
  validateString(pattern, 'pattern');

  const cacheKey = `${windows ? 'win32' : 'posix'}:${pattern}`;
  let matcher;
  if (patternFnCache.has(cacheKey)) {
    matcher = patternFnCache.get(cacheKey);
  } else {
    matcher = createMatcher(pattern, {
      platform: windows ? 'win32' : 'posix',
    });
    patternFnCache.set(cacheKey, matcher);

    if (patternFnCache.size >= kGlobPatternCacheLimit) {
      patternFnCache.delete(patternFnCache.keys().next().value);
    }
  }

  return matcher.match(path);
}

module.exports = {
  __proto__: null,
  createMatcher,
  matchGlobPattern,
};
