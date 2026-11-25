'use strict';

// Round-trip ordinary HTTP/1.1 messages through the parser both sides share: fixed-length and
// chunked framing in each direction, a streamed response body, HEAD, headers in both
// directions, and sequential requests that reuse one keep-alive connection.

const assert = require('node:assert/strict');
const http = require('node:http');
const net = require('node:net');

const listen = (server) => new Promise((resolve, reject) => {
  server.once('error', reject);
  server.listen(0, '127.0.0.1', () => resolve(server.address().port));
});

const close = (server) => new Promise((resolve) => server.close(resolve));

const request = (port, options, body) => new Promise((resolve, reject) => {
  const req = http.request({ host: '127.0.0.1', port, ...options }, (res) => {
    const chunks = [];
    res.on('data', (chunk) => chunks.push(chunk));
    res.on('end', () => resolve({
      status: res.statusCode,
      headers: res.headers,
      body: Buffer.concat(chunks).toString(),
      complete: res.complete,
    }));
  });
  req.once('error', reject);
  req.end(body);
});

(async () => {
  let connections = 0;
  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      const body = Buffer.concat(chunks).toString();
      res.setHeader('x-request-header', req.headers['x-request-header']);
      res.setHeader('x-method', req.method);
      res.setHeader('x-split-name', req.headers['x-split-name'] ?? '');
      if (req.url === '/fixed') {
        const payload = `${req.method} ${body}`;
        res.setHeader('content-length', String(Buffer.byteLength(payload)));
        res.end(payload);
      } else if (req.url === '/chunked') {
        res.write('part one, ');
        setTimeout(() => res.end('part two'), 5);
      } else {
        res.setHeader('content-length', String(body.length));
        res.end(body);
      }
    });
  });
  server.on('connection', () => {
    connections += 1;
  });
  const port = await listen(server);

  try {
    const fixed = await request(port, {
      path: '/fixed',
      method: 'PUT',
      agent: false,
      headers: { 'x-request-header': 'fixed' },
    }, 'fixed body');
    assert.equal(fixed.status, 200);
    assert.equal(fixed.headers['content-length'], '14');
    assert.equal(fixed.headers['x-request-header'], 'fixed');
    assert.equal(fixed.headers['x-method'], 'PUT');
    assert.equal(fixed.body, 'PUT fixed body');
    assert.equal(fixed.complete, true);

    const chunked = await request(port, { path: '/chunked', agent: false });
    assert.equal(chunked.headers['transfer-encoding'], 'chunked');
    assert.equal(chunked.body, 'part one, part two');
    assert.equal(chunked.complete, true);

    const head = await request(port, { path: '/fixed', method: 'HEAD', agent: false });
    assert.equal(head.status, 200);
    assert.equal(head.body, '');
    assert.equal(head.complete, true);

    // A well-formed request whose header name straddles two TCP writes.
    const raw = net.connect(port, '127.0.0.1');
    await new Promise((resolve) => raw.once('connect', resolve));
    raw.write('POST /echo HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Split');
    setTimeout(() => {
      raw.write('-Name: joined\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc');
    }, 5);
    let rawResponse = '';
    raw.on('data', (chunk) => {
      rawResponse += chunk;
    });
    await new Promise((resolve) => raw.once('close', resolve));
    assert.match(rawResponse, /\r\nx-split-name: joined\r\n/i);
    assert.match(rawResponse, /\r\n\r\nabc$/);

    const agent = new http.Agent({ keepAlive: true, maxSockets: 1 });
    connections = 0;
    try {
      const paths = ['/echo', '/chunked', '/echo'];
      const bodies = [];
      for (const path of paths) {
        bodies.push((await request(port, { path, method: 'POST', agent }, path)).body);
      }
      assert.deepEqual(bodies, ['/echo', 'part one, part two', '/echo']);
      assert.equal(connections, 1);
    } finally {
      agent.destroy();
    }
  } finally {
    await close(server);
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
