'use strict';

const assert = require('node:assert');
const { spawn, spawnSync } = require('node:child_process');

// A grandchild that writes one marker to each output stream. Reusing this runtime keeps
// the test independent of a platform shell.
const emit = (marker) => [
  process.execPath,
  ['-e', `process.stdout.write(${JSON.stringify(marker + '-out\n')});` +
         `process.stderr.write(${JSON.stringify(marker + '-err\n')})`],
];

const inherited = spawnSync(...emit('sync'), { stdio: 'inherit' });
assert.strictEqual(inherited.status, 0);
assert.deepStrictEqual(inherited.output, [null, null, null]);
assert.strictEqual(inherited.stdout, null);
assert.strictEqual(inherited.stderr, null);

// The explicit descriptor form of `inherit`.
const descriptors = spawnSync(...emit('fd'), { stdio: [0, 1, 2] });
assert.strictEqual(descriptors.status, 0);
assert.strictEqual(descriptors.stdout, null);
assert.strictEqual(descriptors.stderr, null);

const piped = spawnSync(...emit('pipe'), { encoding: 'utf8' });
assert.strictEqual(piped.status, 0);
assert.strictEqual(piped.stdout, 'pipe-out\n');
assert.strictEqual(piped.stderr, 'pipe-err\n');

const ignored = spawnSync(...emit('ignored'), { stdio: 'ignore' });
assert.strictEqual(ignored.status, 0);
assert.strictEqual(ignored.stdout, null);
assert.strictEqual(ignored.stderr, null);

// The asynchronous shape `npm run` uses; its output joins this process's own.
const child = spawn(...emit('async'), { stdio: 'inherit' });
assert.strictEqual(child.stdin, null);
assert.strictEqual(child.stdout, null);
assert.strictEqual(child.stderr, null);
child.on('close', (code) => {
  assert.strictEqual(code, 0);
  process.stdout.write('async-close\n');
});
