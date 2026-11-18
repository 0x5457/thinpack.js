'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');

(async () => {
  const server = http.createServer((req, res) => {
    res.statusCode = 404;
    res.statusMessage = 'Not Here';
    res.setHeader('content-type', 'text/plain');
    res.end('missing');
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });

  try {
    const response = await fetch(`http://127.0.0.1:${server.address().port}/gone`);
    assert.equal(response.status, 404);
    assert.equal(response.statusText, 'Not Here');
    assert.equal(response.ok, false);
    assert.equal(await response.text(), 'missing');
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
