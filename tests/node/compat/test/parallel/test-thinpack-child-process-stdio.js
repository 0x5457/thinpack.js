'use strict';
// Regression: `stdio: 'inherit'` connects the child to the parent's descriptors instead of
// piping output that no JavaScript stream reads. The fixture runs the cases in a child
// runtime whose stdout and stderr this test captures, so an inherited grandchild's bytes
// appear in those pipes.

const assert = require('node:assert');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const fixture = path.join(__dirname, '..', 'fixtures', 'child-process-stdio.js');
const result = spawnSync(process.execPath, [fixture], { encoding: 'utf8' });

assert.strictEqual(result.status, 0, result.stderr);
assert.strictEqual(result.stdout, 'sync-out\nfd-out\nasync-out\nasync-close\n');
assert.strictEqual(result.stderr, 'sync-err\nfd-err\nasync-err\n');
