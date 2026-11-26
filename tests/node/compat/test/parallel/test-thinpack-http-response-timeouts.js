'use strict';

// The ordinary HTTP boundary Fastify uses: Node-shaped server timeout properties and
// chaining, a callable ServerResponse with assignSocket, the client URL/options and
// timeout argument forms, and the connection, header, request, and keep-alive deadlines.

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

const exchange = (port, payload) => new Promise((resolve, reject) => {
  const socket = net.connect(port, '127.0.0.1');
  let data = '';
  const timer = setTimeout(() => {
    socket.destroy();
    reject(new Error(`exchange with ${port} did not close`));
  }, 2000);
  socket.setEncoding('utf8');
  socket.on('data', (chunk) => { data += chunk; });
  socket.once('error', reject);
  socket.once('connect', () => socket.write(payload));
  socket.once('close', () => {
    clearTimeout(timer);
    resolve(data);
  });
});

(async () => {
  const defaults = http.createServer();
  assert.deepEqual(
    [defaults.timeout, defaults.keepAliveTimeout, defaults.keepAliveTimeoutBuffer,
      defaults.headersTimeout, defaults.requestTimeout],
    [0, 5000, 1000, 60000, 300000],
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

  // Incomplete headers and bodies receive the ordinary 408 response.
  const incomplete = http.createServer({
    headersTimeout: 40,
    requestTimeout: 80,
    connectionsCheckingInterval: 10,
  }, () => {});
  const incompletePort = await listen(incomplete);
  try {
    assert.equal(
      await exchange(incompletePort, 'GET /slow HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Partial: '),
      'HTTP/1.1 408 Request Timeout\r\nConnection: close\r\n\r\n',
    );
    assert.equal(
      await exchange(
        incompletePort,
        'POST /slow HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 5\r\n\r\nab',
      ),
      'HTTP/1.1 408 Request Timeout\r\nConnection: close\r\n\r\n',
    );
  } finally {
    await close(incomplete);
  }

  // A keep-alive connection handles another request, then closes after its idle
  // deadline. The deadline leaves ample margin for the client to observe the first
  // response and send the second request before the server closes the idle socket.
  const keepAliveTimeout = 1000;
  const withKeepAlive = http.createServer({
    keepAliveTimeout,
    keepAliveTimeoutBuffer: 0,
    connectionsCheckingInterval: 10,
  }, (req, res) => {
    res.setHeader('content-length', '2');
    res.end('ok');
  });
  const keepAlivePort = await listen(withKeepAlive);
  const keepAliveSocket = net.connect(keepAlivePort, '127.0.0.1');
  let keepAliveData = '';
  let keepAliveClosedAt;
  keepAliveSocket.setEncoding('utf8');
  keepAliveSocket.on('data', (chunk) => { keepAliveData += chunk; });
  keepAliveSocket.on('error', () => {});
  keepAliveSocket.once('close', () => { keepAliveClosedAt = process.hrtime.bigint(); });
  const keepAliveResponses = () => keepAliveData.split('\r\n\r\nok').length - 1;

  // Wait on socket events rather than a polling timer, so a delayed poll cannot be
  // mistaken for a delayed response.
  const waitForResponses = (count, message) => new Promise((resolve, reject) => {
    if (keepAliveClosedAt !== undefined && keepAliveResponses() < count) {
      reject(new Error(`${message} (socket closed first)`));
      return;
    }
    let timeout;
    const finish = (error) => {
      clearTimeout(timeout);
      keepAliveSocket.off('data', check);
      keepAliveSocket.off('close', onClose);
      if (error) reject(error); else resolve();
    };
    const check = () => {
      if (keepAliveResponses() >= count) finish();
    };
    const onClose = () => finish(new Error(`${message} (socket closed first)`));
    keepAliveSocket.on('data', check);
    keepAliveSocket.once('close', onClose);
    timeout = setTimeout(() => finish(new Error(message)), 5000);
    check();
  });
  try {
    await new Promise((resolve, reject) => {
      keepAliveSocket.once('connect', resolve);
      keepAliveSocket.once('error', reject);
    });
    keepAliveSocket.write('GET /1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n');
    await waitForResponses(1, 'first keep-alive response missing');
    const secondRequestAt = process.hrtime.bigint();
    keepAliveSocket.write('GET /2 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n');
    await waitForResponses(2, 'second keep-alive response missing');
    assert.equal(keepAliveResponses(), 2);
    assert.equal((keepAliveData.match(/HTTP\/1\.1 200 OK/g) ?? []).length, 2);
    assert.equal((keepAliveData.match(/Connection: keep-alive/g) ?? []).length, 2);
    await new Promise((resolve, reject) => {
      if (keepAliveClosedAt !== undefined) {
        resolve();
        return;
      }
      const timeout = setTimeout(
        () => reject(new Error('keep-alive socket did not close')),
        keepAliveTimeout + 5000,
      );
      keepAliveSocket.once('close', () => {
        clearTimeout(timeout);
        resolve();
      });
    });
    // The idle deadline is armed after the second request, so a slow scheduler can
    // only make this interval longer. The tolerance covers timer granularity.
    const idle = Number((keepAliveClosedAt - secondRequestAt) / 1000000n);
    assert.ok(idle >= keepAliveTimeout - 50, `closed after ${idle}ms of idle time`);
  } finally {
    keepAliveSocket.destroy();
    await close(withKeepAlive);
  }

  // Pipelined requests keep their order even when the first handler outlives
  // keepAliveTimeout; an idle deadline must not apply while work is outstanding.
  const pipelined = http.createServer({
    keepAliveTimeout: 20,
    keepAliveTimeoutBuffer: 0,
    connectionsCheckingInterval: 10,
  }, (req, res) => {
    res.setHeader('content-length', '3');
    if (req.url === '/1') setTimeout(() => res.end('one'), 50);
    else res.end('two');
  });
  const pipelinedPort = await listen(pipelined);
  try {
    const data = await exchange(
      pipelinedPort,
      'GET /1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n' +
      'GET /2 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n',
    );
    assert.match(data, /\r\n\r\noneHTTP\/1\.1 200 OK[\s\S]*\r\n\r\ntwo$/);
  } finally {
    await close(pipelined);
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
