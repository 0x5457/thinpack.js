'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');

(async () => {
  const seen = [];
  const server = http.createServer((req, res) => {
    seen.push([req.method, req.url]);
    if (req.url === '/moved') {
      res.writeHead(302, { location: '/target' });
      res.end();
      return;
    }
    res.setHeader('content-type', 'text/plain');
    res.end(`${req.method} ${req.url}`);
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const base = `http://127.0.0.1:${server.address().port}`;

  try {
    const followed = await fetch(`${base}/moved`, { method: 'POST', body: 'dropped' });
    assert.equal(followed.status, 200);
    assert.equal(followed.redirected, true);
    assert.equal(followed.url, `${base}/target`);
    assert.equal(await followed.text(), 'GET /target');
    assert.deepEqual(seen, [['POST', '/moved'], ['GET', '/target']]);

    seen.length = 0;
    const manual = await fetch(`${base}/moved`, { redirect: 'manual' });
    assert.equal(manual.status, 302);
    assert.equal(manual.headers.get('location'), '/target');
    assert.equal(manual.redirected, false);
    assert.deepEqual(seen, [['GET', '/moved']]);
    await manual.text();

    seen.length = 0;
    await assert.rejects(fetch(`${base}/moved`, { redirect: 'error' }), {
      name: 'TypeError',
      message: 'fetch failed',
    });
    assert.deepEqual(seen, [['GET', '/moved']]);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
