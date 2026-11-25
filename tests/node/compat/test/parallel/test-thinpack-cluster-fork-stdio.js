'use strict';
// The primary's view of a worker — piped stdio, the exit code and the signal — comes from
// the child-process layer this runtime shares with `node:child_process`. Both fork stdio
// policies go through it: `silent` pipes the three streams, and the default inherits them.

const common = require('../common');
const assert = require('node:assert');
const cluster = require('node:cluster');

const MODE = 'THINPACK_CLUSTER_STDIO';

if (cluster.isPrimary) {
  cluster.setupPrimary({ silent: true });
  const piped = cluster.fork({ [MODE]: 'piped' });
  assert.notStrictEqual(piped.process.stdin, null);
  assert.notStrictEqual(piped.process.stdout, null);
  assert.notStrictEqual(piped.process.stderr, null);

  let stdout = '';
  let stderr = '';
  piped.process.stdout.setEncoding('utf8');
  piped.process.stderr.setEncoding('utf8');
  piped.process.stdout.on('data', (chunk) => { stdout += chunk; });
  piped.process.stderr.on('data', (chunk) => { stderr += chunk; });
  piped.process.stdin.end('ping\n');

  // `close` waits for every piped stream to end, so the collected output is complete.
  piped.process.once('close', common.mustCall((code, signal) => {
    assert.strictEqual(code, 42);
    assert.strictEqual(signal, null);
    assert.strictEqual(stdout, 'stdout:ping\n');
    assert.strictEqual(stderr, 'stderr\n');

    cluster.setupPrimary({ silent: false });
    const inherited = cluster.fork({ [MODE]: 'inherit' });
    assert.strictEqual(inherited.process.stdin, null);
    assert.strictEqual(inherited.process.stdout, null);
    assert.strictEqual(inherited.process.stderr, null);
    inherited.process.once('close', common.mustCall((code, signal) => {
      assert.strictEqual(code, 0);
      assert.strictEqual(signal, null);
      cluster.disconnect(common.mustCall());
    }));
  }));
} else if (process.env[MODE] === 'piped') {
  process.stdin.setEncoding('utf8');
  let input = '';
  process.stdin.on('data', (chunk) => { input += chunk; });
  process.stdin.on('end', common.mustCall(() => {
    process.stdout.write(`stdout:${input}`);
    process.stderr.write('stderr\n');
    process.exit(42);
  }));
} else {
  process.exit(0);
}
