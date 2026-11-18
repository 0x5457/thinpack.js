'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');
const zlib = require('node:zlib');

(async () => {
  const payload = Buffer.from('gzip encoded payload');
  const server = http.createServer((req, res) => {
    res.setHeader('content-encoding', 'gzip');
    res.setHeader('content-type', 'text/plain');
    res.end(zlib.gzipSync(payload));
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });

  try {
    const response = await fetch(`http://127.0.0.1:${server.address().port}/gzip`);
    assert.equal(response.headers.get('content-encoding'), 'gzip');
    assert.equal(await response.text(), payload.toString());
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
