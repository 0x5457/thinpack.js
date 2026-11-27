'use strict';

// An empty implicit end() is a known-length empty response in Node: ordinary HTTP/1.1
// GET gets Content-Length: 0, HEAD gets no framing header, and once the header is
// composed (writeHead, flushHeaders, or any write, including an empty one) the existing
// framing stays unchanged.

const assert = require('node:assert/strict');
const http = require('node:http');
const net = require('node:net');

const listen = (server) => new Promise((resolve, reject) => {
  server.once('error', reject);
  server.listen(0, '127.0.0.1', () => resolve(server.address().port));
});

const close = (server) => new Promise((resolve) => server.close(resolve));

const exchange = (port, version, method, path) => new Promise((resolve, reject) => {
  const socket = net.connect(port, '127.0.0.1');
  let raw = '';
  const timer = setTimeout(() => {
    socket.destroy();
    reject(new Error(`${method} ${path} did not close`));
  }, 2000);
  socket.setEncoding('latin1');
  socket.on('data', (chunk) => { raw += chunk; });
  socket.once('error', reject);
  socket.once('connect', () => {
    socket.write(
      `${method} ${path} HTTP/${version}\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n`,
    );
  });
  socket.once('close', () => {
    clearTimeout(timer);
    const split = raw.indexOf('\r\n\r\n');
    assert.notEqual(split, -1, `incomplete response for ${method} ${path}`);
    const lines = raw.slice(0, split).split('\r\n');
    const headers = new Map();
    for (const line of lines.slice(1)) {
      const colon = line.indexOf(':');
      headers.set(line.slice(0, colon).toLowerCase(), line.slice(colon + 1).trim());
    }
    resolve({
      status: Number(lines[0].split(' ')[1]),
      length: headers.get('content-length'),
      encoding: headers.get('transfer-encoding'),
      body: raw.slice(split + 4),
    });
  });
});

(async () => {
  const observed = new Map();
  const server = http.createServer((req, res) => {
    res.sendDate = false;
    res.statusCode = 404;
    if (req.url === '/head-length') res.setHeader('content-length', '7');
    if (req.url === '/writehead') res.writeHead(404);
    if (req.url === '/flush') res.flushHeaders();
    if (req.url === '/empty-write') res.write('');
    if (req.url === '/first-write') res.write('x');
    res.once('finish', () => observed.set(`${req.method} ${req.url}`, res.getHeader('content-length')));
    res.end();
  });
  const port = await listen(server);
  const request = (version, method, path) => exchange(port, version, method, path);

  try {
    const empty = await request('1.1', 'GET', '/empty');
    assert.deepEqual(empty, { status: 404, length: '0', encoding: undefined, body: '' });

    const head = await request('1.1', 'HEAD', '/empty');
    assert.deepEqual(head, { status: 404, length: undefined, encoding: undefined, body: '' });

    const explicitHead = await request('1.1', 'HEAD', '/head-length');
    assert.deepEqual(
      explicitHead,
      { status: 404, length: '7', encoding: undefined, body: '' },
    );

    for (const path of ['/writehead', '/flush', '/empty-write', '/first-write']) {
      const get = await request('1.1', 'GET', path);
      assert.deepEqual(get, {
        status: 404,
        length: undefined,
        encoding: 'chunked',
        body: path === '/first-write' ? '1\r\nx\r\n0\r\n\r\n' : '0\r\n\r\n',
      });
      const headControl = await request('1.1', 'HEAD', path);
      assert.deepEqual(
        headControl,
        { status: 404, length: undefined, encoding: undefined, body: '' },
      );
    }

    const http10 = await request('1.0', 'GET', '/empty');
    assert.deepEqual(http10, { status: 404, length: undefined, encoding: undefined, body: '' });

    assert.equal(observed.get('GET /empty'), undefined);
    assert.equal(observed.get('HEAD /empty'), undefined);
    assert.equal(observed.get('HEAD /head-length'), '7');
  } finally {
    await close(server);
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
