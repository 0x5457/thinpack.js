// Copyright Node.js contributors. All rights reserved.
// Copyright 2018-2026 the Deno authors. MIT license.
// Adapted from Node.js lib/test.js and Deno ext/node/polyfills/testing.ts.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

'use strict';

const runner = require('node:thinpack/internal/test_runner');

module.exports = runner.test;
module.exports.after = runner.after;
module.exports.afterEach = runner.afterEach;
module.exports.before = runner.before;
module.exports.beforeEach = runner.beforeEach;
module.exports.describe = runner.suite;
module.exports.getTestContext = runner.getTestContext;
module.exports.it = runner.test;
module.exports.mock = runner.mock;
module.exports.run = runner.run;
module.exports.suite = runner.suite;
module.exports.test = runner.test;
module.exports.TestContext = runner.TestContext;
