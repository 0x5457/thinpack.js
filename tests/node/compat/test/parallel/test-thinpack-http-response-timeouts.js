'use strict';

// The ordinary HTTP boundary Fastify uses: Node-shaped server timeout properties and
// chaining, a callable ServerResponse with assignSocket, the client URL/options and
// timeout argument forms, and a handled server connection timeout.

const assert = require('node:assert/strict');
const http = require('node:http');
const net = require('node:net');
const { Writable } = require('node:stream');
const { inherits } = require('node:util');

const listen = (server) => new Promise((resolve, reject) => {
  server.once('error', reject);
  server.listen(0, '127.0.0.1', () => resolve(server.address().port));
});

const close = (server) => new Promise((resolve) => server.close(resolve));

(async () => {
  const defaults = http.createServer();
  assert.deepEqual(
    [defaults.timeout, defaults.keepAliveTimeout, defaults.headersTimeout, defaults.requestTimeout],
    [0, 5000, 60000, 300000],
  );
  assert.equal(defaults.setTimeout(17, () => {}), defaults);
  assert.equal(defaults.timeout, 17);

  // light-my-request constructs the response with call() and attaches its own socket.
  function Response(req) {
    http.ServerResponse.call(this, req);
  }
  inherits(Response, http.ServerResponse);

  let wire = '';
  const socket = new Writable({
    write(chunk, encoding, callback) {
      wire += chunk.toString();
      callback();
    },
  });
  const response = new Response({ method: 'GET', httpVersion: '1.1' });
  assert.equal(response instanceof http.ServerResponse, true);
  assert.equal(response.statusCode, 200);
  assert.equal(typeof response.assignSocket, 'function');
  response.assignSocket(socket);
  assert.equal(response.connection, socket);
  const finished = new Promise((resolve) => response.once('finish', resolve));
  response.writeHead(200, { 'content-length': '2' });
  response.end('ok');
  await finished;
  assert.match(wire, /^HTTP\/1\.1 200 OK\r\n/);
  assert.match(wire, /\r\nConnection: keep-alive\r\n/);
  assert.match(wire, /\r\n\r\nok$/);

  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      const body = Buffer.concat(chunks).toString();
      const payload = JSON.stringify({ body });
      res.writeHead(body === '{"ok":true}' ? 200 : 400, {
        'content-type': 'application/json',
        'content-length': String(Buffer.byteLength(payload)),
      });
      res.end(payload);
    });
  });
  const port = await listen(server);
  try {
    const request = (payload) => new Promise((resolve, reject) => {
      const req = http.request(new URL('/echo', `http://127.0.0.1:${port}`), {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
      }, (res) => {
        let text = '';
        res.setEncoding('utf8');
        res.on('data', (chunk) => { text += chunk; });
        res.on('end', () => resolve({ status: res.statusCode, body: JSON.parse(text) }));
      });
      req.on('error', reject);
      req.setTimeout(2000, () => req.destroy(new Error('request timed out')));
      req.end(payload);
    });
    assert.deepEqual(await request('{"ok":true}'), {
      status: 200,
      body: { body: '{"ok":true}' },
    });
    assert.deepEqual(await request('{}'), { status: 400, body: { body: '{}' } });
  } finally {
    await close(server);
  }

  // A handled server timeout is advisory: the idle connection stays open and usable.
  let timedOut = 0;
  let resolveTimeout;
  const timeoutFired = new Promise((resolve) => { resolveTimeout = resolve; });
  const timed = http.createServer((req, res) => {
    res.setHeader('content-length', '2');
    res.end('ok');
  });
  timed.setTimeout(40, () => {
    timedOut += 1;
    resolveTimeout();
  });
  const timedPort = await listen(timed);
  const timedSocket = net.connect(timedPort, '127.0.0.1');
  try {
    await new Promise((resolve, reject) => {
      timedSocket.once('connect', resolve);
      timedSocket.once('error', reject);
    });
    await timeoutFired;
    assert.equal(timedOut, 1);
    assert.equal(timedSocket.destroyed, false);

    // The handled timeout did not poison the connection.
    const response = new Promise((resolve, reject) => {
      let data = '';
      timedSocket.setEncoding('utf8');
      timedSocket.on('data', (chunk) => { data += chunk; });
      timedSocket.once('close', () => resolve(data));
      timedSocket.once('error', reject);
    });
    timedSocket.write('GET /after-timeout HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n');
    assert.match(await response, /^HTTP\/1\.1 200 OK\r\n[\s\S]*\r\n\r\nok$/);
  } finally {
    timedSocket.destroy();
    await close(timed);
  }

  // With no timeout listener, Node closes the timed-out socket.
  const unhandled = http.createServer();
  unhandled.setTimeout(20);
  const unhandledPort = await listen(unhandled);
  const unhandledSocket = net.connect(unhandledPort, '127.0.0.1');
  try {
    await new Promise((resolve) => {
      unhandledSocket.once('error', resolve);
      unhandledSocket.once('close', resolve);
    });
    assert.equal(unhandledSocket.destroyed, true);
  } finally {
    unhandledSocket.destroy();
    await close(unhandled);
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
