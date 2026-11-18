'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');

(async () => {
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      res.setHeader('content-type', req.headers['content-type'] ?? 'application/octet-stream');
      res.end(Buffer.concat(chunks));
    });
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const url = `http://127.0.0.1:${server.address().port}/echo`;

  try {
    const form = new FormData();
    form.append('field', 'value');
    form.append('file', new Blob(['file payload'], { type: 'text/plain' }), 'payload.txt');

    const response = await fetch(url, { method: 'POST', body: form });
    const contentType = response.headers.get('content-type');
    const boundary = /^multipart\/form-data; boundary=(.+)$/.exec(contentType)?.[1];
    assert.ok(boundary, `unexpected content type: ${contentType}`);

    const body = await response.text();
    assert.ok(body.startsWith(`--${boundary}\r\n`));
    assert.ok(body.endsWith(`--${boundary}--\r\n`));
    assert.ok(body.includes(
      'Content-Disposition: form-data; name="field"\r\n\r\nvalue\r\n',
    ));
    assert.ok(body.includes(
      'Content-Disposition: form-data; name="file"; filename="payload.txt"\r\n' +
        'Content-Type: text/plain\r\n\r\nfile payload\r\n',
    ));

    // The serialized body parses back into the same fields.
    const echoed = new Response(body, { headers: { 'content-type': contentType } });
    const parsed = await echoed.formData();
    assert.equal(parsed.get('field'), 'value');
    const file = parsed.get('file');
    assert.equal(file.name, 'payload.txt');
    assert.equal(file.type, 'text/plain');
    assert.equal(await file.text(), 'file payload');

    // A FormData serializes on demand, so it is usable as a body more than once -- undici
    // re-encodes it per request, and Node has no "spent" state for it.
    const again = await fetch(url, { method: 'POST', body: form });
    assert.match(again.headers.get('content-type'), /^multipart\/form-data; boundary=/);

    const params = new URLSearchParams({ a: '1' });
    const encoded = await fetch(url, { method: 'POST', body: params });
    assert.equal(encoded.headers.get('content-type'), 'application/x-www-form-urlencoded;charset=UTF-8');
    assert.equal(await encoded.text(), 'a=1');
    assert.equal(new Request(url, { method: 'POST', body: params }).method, 'POST');
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
