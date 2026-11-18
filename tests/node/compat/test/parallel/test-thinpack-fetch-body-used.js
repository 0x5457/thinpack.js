'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');

(async () => {
  const server = http.createServer((req, res) => {
    res.setHeader('content-type', 'text/plain');
    res.end('read me once');
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const url = `http://127.0.0.1:${server.address().port}/once`;

  try {
    const response = await fetch(url);
    assert.equal(await response.text(), 'read me once');
    assert.equal(response.bodyUsed, true);
    await assert.rejects(response.text(), {
      name: 'TypeError',
      message: 'Body is unusable: Body has already been read',
    });
    assert.throws(() => response.clone(), {
      name: 'TypeError',
      message: 'Response.clone: Body has already been consumed.',
    });

    // A clone reads the same bytes independently of the original.
    const original = await fetch(url);
    const clone = original.clone();
    assert.deepEqual(await original.bytes(), new Uint8Array(Buffer.from('read me once')));
    assert.equal(await clone.text(), 'read me once');
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
