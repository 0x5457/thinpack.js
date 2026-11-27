'use strict';

const assert = require('node:assert/strict');
const util = require('node:util');

// EOF (-4095) is in every platform's table. Each call must return a fresh
// tuple, and rewriting one must not reach later results or the shared table.
const first = util.getSystemErrorMap().get(-4095);
assert.notStrictEqual(first, util.getSystemErrorMap().get(-4095));
assert.deepEqual(first, ['EOF', 'end of file']);

first[0] = 'NOT_EOF';
first[1] = 'not end of file';
assert.deepEqual(util.getSystemErrorMap().get(-4095), ['EOF', 'end of file']);
assert.equal(util.getSystemErrorName(-4095), 'EOF');
assert.equal(util.getSystemErrorMessage(-4095), 'end of file');
