'use strict';

const assert = require('node:assert/strict');
const net = require('node:net');

const watchdog = setTimeout(() => {
  console.error('net early write watchdog');
  process.exit(3);
}, 10000);
watchdog.unref();

const listen = (server) => new Promise((resolve, reject) => {
  server.once('error', reject);
  server.listen(0, '127.0.0.1', () => resolve(server.address().port));
});
const closeServer = (server) => new Promise((resolve) => server.close(resolve));
const record = (callbacks) => (error) => callbacks.push(error?.code ?? 'ok');
const BEFORE = 'ERR_SOCKET_CLOSED_BEFORE_CONNECTION';
const REFUSED_ERRNO = { win32: -4078, darwin: -61 }[process.platform] ?? -111;

const client = (socket) => {
  const state = { data: '', finish: 0, end: 0, error: null };
  socket.on('data', (chunk) => { state.data += chunk; });
  socket.on('finish', () => { state.finish += 1; });
  socket.on('end', () => { state.end += 1; });
  socket.on('error', (error) => { state.error = error; });
  return {
    socket,
    state,
    callbacks: [],
    done: new Promise((resolve) => socket.once('close', resolve)),
  };
};

const assertReply = (state) =>
  assert.deepEqual(state, { data: 'R', finish: 1, end: 1, error: null });

async function queuedWrites() {
  let connected = false;
  const server = net.createServer({ allowHalfOpen: true }, (peer) => {
    const chunks = [];
    peer.on('data', (chunk) => {
      assert.equal(connected, true, 'peer received data before connect');
      chunks.push(chunk);
    });
    peer.on('end', () => {
      assert.equal(Buffer.concat(chunks).toString(), 'ABC');
      peer.end('R');
    });
  });
  const port = await listen(server);

  class CountingSocket extends net.Socket {
    dispatches = 0;
    _write(chunk, encoding, callback) {
      if (String(chunk) === 'A') this.dispatches += 1;
      super._write(chunk, encoding, callback);
    }
  }

  const { socket, state, callbacks, done } = client(
    new CountingSocket({ highWaterMark: 1 }),
  );
  socket.on('connect', () => { connected = true; });
  socket.connect({ port, host: '127.0.0.1' });
  const writeA = socket.write('A', record(callbacks));
  const writeB = socket.write('B', record(callbacks));
  socket.end('C', record(callbacks));
  await done;
  await closeServer(server);

  assert.deepEqual([writeA, writeB], [false, false]);
  assert.deepEqual(callbacks, ['ok', 'ok', 'ok']);
  assert.equal(socket.dispatches, 1);
  assertReply(state);
}

async function emptyEnd() {
  let connected = false;
  const server = net.createServer({ allowHalfOpen: true }, (peer) => {
    peer.resume();
    peer.on('end', () => peer.end('R'));
  });
  const port = await listen(server);

  const { socket, state, callbacks, done } = client(
    net.connect({ port, host: '127.0.0.1' }),
  );
  socket.on('connect', () => { connected = true; });
  socket.end(record(callbacks));
  await done;
  await closeServer(server);

  assert.equal(connected, true);
  assert.deepEqual(callbacks, ['ok']);
  assertReply(state);
}

async function failure(port, destroy) {
  const { socket, state, callbacks, done } = client(
    net.connect({ port, host: '127.0.0.1' }),
  );
  socket.write('A', record(callbacks));
  socket.end('C', record(callbacks));
  if (destroy) socket.destroy();
  await done;
  return { state, callbacks };
}

async function destroyedAndRefused() {
  const server = net.createServer((peer) => peer.destroy());
  const port = await listen(server);
  const destroyed = await failure(port, true);
  await closeServer(server);

  const probe = net.createServer();
  const refusedPort = await listen(probe);
  await closeServer(probe);
  const refused = await failure(refusedPort, false);

  assert.deepEqual(
    [[destroyed.state.error, destroyed.callbacks],
     [refused.state.error.code, refused.state.error.errno, refused.callbacks]],
    [[null, [BEFORE, BEFORE]],
     ['ECONNREFUSED', REFUSED_ERRNO, [BEFORE, 'ECONNREFUSED']]],
  );
}

(async () => {
  await queuedWrites();
  await emptyEnd();
  await destroyedAndRefused();
  clearTimeout(watchdog);
  console.log('net early write ok');
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
