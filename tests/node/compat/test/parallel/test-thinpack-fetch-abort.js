'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');

(async () => {
  const server = http.createServer((req, res) => {
    setTimeout(() => res.end('late'), 250);
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const url = `http://127.0.0.1:${server.address().port}/slow`;

  try {
    const controller = new AbortController();
    const pending = fetch(url, { signal: controller.signal });
    controller.abort();
    await assert.rejects(pending, { name: 'AbortError' });

    const aborted = new AbortController();
    aborted.abort();
    await assert.rejects(fetch(url, { signal: aborted.signal }), { name: 'AbortError' });

    // Aborting while the body is still arriving errors the body stream.
    const streaming = new AbortController();
    const response = await fetch(url, { signal: streaming.signal });
    streaming.abort();
    await assert.rejects(response.text(), { name: 'AbortError' });
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
