'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');

(async () => {
  const server = http.createServer((req, res) => {
    assert.equal(req.method, 'GET');
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ path: req.url }));
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });

  try {
    const url = `http://127.0.0.1:${server.address().port}/json?x=1`;
    const response = await fetch(url);
    assert.equal(response.status, 200);
    assert.equal(response.statusText, 'OK');
    assert.equal(response.ok, true);
    assert.equal(response.type, 'basic');
    assert.equal(response.url, url);
    assert.equal(response.redirected, false);
    assert.equal(response.headers.get('content-type'), 'application/json');
    assert.deepEqual(await response.json(), { path: '/json?x=1' });
    assert.equal(response.bodyUsed, true);
    assert.equal(response.body.locked, true, 'the body stream was read');
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
