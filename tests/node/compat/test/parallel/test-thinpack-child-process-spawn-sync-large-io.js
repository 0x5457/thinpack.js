'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');

if (process.platform !== 'win32') {
  const outputLength = 256 * 1024;
  // The child fills stdout before it starts consuming the large stdin payload.
  const result = spawnSync(
    '/bin/sh',
    ['-c', `head -c ${outputLength} /dev/zero; cat >/dev/null`],
    { input: Buffer.alloc(1024 * 1024, 0x61) },
  );

  assert.equal(result.error, undefined);
  assert.equal(result.status, 0);
  assert.equal(result.stdout.length, outputLength);
  assert.ok(result.stdout.every((byte) => byte === 0));
}
