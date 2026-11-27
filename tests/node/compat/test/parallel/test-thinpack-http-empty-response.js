'use strict';

// An implicit single end() has a known length in Node. HEAD gets no automatic
// framing header, and earlier writes or an explicit header keep their framing.

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
  let endCallbacks = 0;
  const server = http.createServer((req, res) => {
    res.sendDate = false;
    res.statusCode = 404;
    if (req.url === '/head-length') res.setHeader('content-length', '7');
    if (req.url === '/writehead') res.writeHead(404);
    if (req.url === '/flush') res.flushHeaders();
    if (req.url === '/empty-write') res.write('');
    if (req.url === '/first-write') res.write('x');
    res.once('finish', () => observed.set(`${req.method} ${req.url}`, res.getHeader('content-length')));
    switch (req.url) {
      case '/body':
        return res.end('héllo');
      case '/encoded':
        return res.end('e9', 'hex', () => { endCallbacks += 1; });
      case '/buffer':
        return res.end(Buffer.from('x'), () => { endCallbacks += 1; });
      case '/empty-string':
        return res.end('');
      case '/explicit-body':
        res.writeHead(404);
        return res.end(Buffer.from('x'));
      case '/flushed-body':
        res.flushHeaders();
        return res.end('x');
      case '/no-length':
        res.removeHeader('Content-Length');
        return res.end('x');
      case '/buffered-write':
        res.cork();
        res.write('');
        return res.end('x');
      default:
        return res.end();
    }
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

    const bodyBytes = Buffer.from('héllo').toString('latin1');
    const getBody = await request('1.1', 'GET', '/body');
    assert.deepEqual(getBody, {
      status: 404, length: '6', encoding: undefined, body: bodyBytes,
    });
    const encoded = await request('1.1', 'GET', '/encoded');
    assert.deepEqual(encoded, { status: 404, length: '1', encoding: undefined, body: 'é' });
    const buffer = await request('1.1', 'GET', '/buffer');
    assert.deepEqual(buffer, { status: 404, length: '1', encoding: undefined, body: 'x' });
    assert.deepEqual(await request('1.1', 'GET', '/empty-string'), empty);
    assert.equal(endCallbacks, 2);

    for (const path of ['/explicit-body', '/flushed-body', '/no-length', '/buffered-write']) {
      const response = await request('1.1', 'GET', path);
      assert.deepEqual(response, {
        status: 404, length: undefined, encoding: 'chunked', body: '1\r\nx\r\n0\r\n\r\n',
      });
    }
    const headBody = await request('1.1', 'HEAD', '/body');
    assert.deepEqual(headBody, { status: 404, length: undefined, encoding: undefined, body: '' });
    const oldHttpBody = await request('1.0', 'GET', '/body');
    assert.deepEqual(oldHttpBody, {
      status: 404, length: undefined, encoding: undefined, body: bodyBytes,
    });

    assert.equal(observed.get('GET /body'), undefined);
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
