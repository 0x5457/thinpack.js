// Shared regression for packed::tls-duplex-client and the local loopback
// oracle. Run directly with: node tests/fixtures/tls/duplex.cjs tests/fixtures/tls
const assert = require('node:assert/strict');
const { once } = require('node:events');
const { readFileSync } = require('node:fs');
const net = require('node:net');
const { Duplex, PassThrough, duplexPair } = require('node:stream');
const tls = require('node:tls');

const thinpack = process.versions.thinpack !== undefined;
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
const greeting = Buffer.from('welcome');
const echoed = Buffer.from('held');
const echoPayload = Buffer.alloc(256 * 1024);
const delayedPayload = Buffer.alloc(192 * 1024);
for (let index = 0; index < echoPayload.length; index += 1) echoPayload[index] = index % 251;
for (let index = 0; index < delayedPayload.length; index += 1) {
  delayedPayload[index] = (index * 7) % 251;
}

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => (resolve = done));
  return { promise, resolve };
};

const waitFor = async (predicate, what) => {
  const deadline = Date.now() + 10000;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await delay(5);
  }
};

// One ordinary trusted fixture: a TLS server behind a normal TCP relay. Every
// connection opens with an eager greeting; its first plaintext byte selects an
// echo reply or the 192 KiB delayed reply.
const startTrustedFixture = async (files) => {
  const sockets = new Set();
  const server = tls.createServer(
    { key: files.key, cert: files.cert, ALPNProtocols: ['h1'] },
    (socket) => {
      sockets.add(socket);
      socket.once('close', () => sockets.delete(socket));
      socket.on('error', () => {});
      socket.write(greeting);
      let mode = null;
      const chunks = [];
      socket.on('data', (chunk) => {
        if (mode === null) mode = chunk[0] === 0x64 ? 'delayed' : 'echo';
        if (mode === 'delayed') {
          const write = (offset) => {
            if (offset >= delayedPayload.length) { socket.end(); return; }
            const part = delayedPayload.subarray(offset, offset + 32768);
            socket.write(part, () => write(offset + part.length));
          };
          write(0);
          return;
        }
        chunks.push(chunk);
      });
      socket.once('end', () => {
        if (mode === 'echo') socket.end(Buffer.concat(chunks));
      });
    },
  );
  server.on('error', (error) => { throw error; });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const relaySockets = new Set();
  const relay = net.createServer({ allowHalfOpen: true }, (client) => {
    const upstream = net.connect(server.address().port, '127.0.0.1');
    for (const socket of [client, upstream]) {
      relaySockets.add(socket);
      socket.once('close', () => relaySockets.delete(socket));
    }
    client.pipe(upstream);
    upstream.pipe(client);
    client.on('error', (error) => upstream.destroy(error));
    upstream.on('error', (error) => client.destroy(error));
  });
  relay.on('error', (error) => { throw error; });
  relay.listen(0, '127.0.0.1');
  await once(relay, 'listening');
  return {
    connect: () => net.connect({
      port: relay.address().port,
      host: '127.0.0.1',
      allowHalfOpen: true,
    }),
    close: async () => {
      for (const socket of sockets) socket.destroy();
      for (const socket of relaySockets) socket.destroy();
      await new Promise((resolve) => relay.close(resolve));
      await new Promise((resolve) => server.close(resolve));
    },
  };
};

// A byte-mode Duplex over the relayed TCP socket. ignorePause() models a
// transport whose peer response and close_notify are already in memory.
const openTransport = async (fixture) => {
  const tcp = fixture.connect();
  await once(tcp, 'connect');
  let ignorePause = false;
  let rawBytes = 0;
  let tcpEnded = false;
  let pendingWrites = 0;
  let held = null;
  let failure = null;
  let failedWrites = 0;
  let writesAfterFailure = 0;
  const transport = new Duplex({
    allowHalfOpen: true,
    read() { tcp.resume(); },
    write(chunk, encoding, callback) {
      if (failure !== null) {
        const error = failure;
        failure = null;
        failedWrites += 1;
        callback(error);
        return;
      }
      if (failedWrites !== 0) writesAfterFailure += 1;
      pendingWrites += 1;
      tcp.write(chunk, (error) => {
        pendingWrites -= 1;
        if (held === null) { callback(error); return; }
        const gate = held;
        held = null;
        gate.started.resolve();
        gate.release.promise.then(() => callback(error));
      });
    },
    final(callback) { tcp.end(callback); },
    destroy(error, callback) {
      tcp.destroy(error);
      callback(error);
    },
  });
  const pause = transport.pause.bind(transport);
  transport.pause = () => {
    if (!ignorePause) pause();
  };
  transport.on('error', () => {});
  tcp.on('data', (chunk) => {
    rawBytes += chunk.length;
    if (!transport.push(chunk) && !ignorePause) tcp.pause();
  });
  tcp.once('end', () => {
    tcpEnded = true;
    transport.push(null);
  });
  tcp.on('error', (error) => transport.destroy(error));
  return {
    transport,
    ignorePause() {
      ignorePause = true;
      transport.resume();
      tcp.resume();
    },
    holdNextWrite() {
      const started = deferred();
      const release = deferred();
      held = { started, release };
      return { started: started.promise, release: release.resolve };
    },
    failNextWrite(error) { failure = error; },
    idle: async () => {
      while (pendingWrites !== 0) await delay(5);
      await delay(50);
    },
    rawBytes: () => rawBytes,
    tcpEnded: () => tcpEnded,
    failedWrites: () => failedWrites,
    writesAfterFailure: () => writesAfterFailure,
  };
};

const openClient = (transport, ca) => {
  const counts = { secureConnect: 0, finish: 0, end: 0, close: 0, error: 0 };
  const received = [];
  let dataBeforeSecure = false;
  let clientError = null;
  const secure = deferred();
  const finished = deferred();
  const ended = deferred();
  const closed = deferred();
  const client = tls.connect({
    socket: transport,
    servername: 'localhost',
    ca,
    ALPNProtocols: ['h1'],
  });
  client.on('secureConnect', () => { counts.secureConnect += 1; secure.resolve(); });
  client.on('data', (chunk) => {
    if (counts.secureConnect !== 1) dataBeforeSecure = true;
    assert.equal(client.authorized, true, 'authorized before first plaintext');
    received.push(chunk);
  });
  client.on('finish', () => { counts.finish += 1; finished.resolve(); });
  client.on('end', () => { counts.end += 1; ended.resolve(); });
  client.on('close', () => { counts.close += 1; closed.resolve(); });
  client.on('error', (error) => { counts.error += 1; clientError = error; });
  return {
    client,
    counts,
    received,
    bytes: () => Buffer.concat(received),
    dataBeforeSecure: () => dataBeforeSecure,
    error: () => clientError,
    secure: secure.promise,
    finished: finished.promise,
    ended: ended.promise,
    closed: closed.promise,
    write: (chunk) => new Promise((resolve, reject) => {
      client.write(chunk, (error) => (error ? reject(error) : resolve()));
    }),
  };
};

const awaitGreeting = async (state) => {
  await state.secure;
  await waitFor(() => state.bytes().length >= greeting.length, 'greeting');
  assert.ok(state.bytes().equals(greeting), 'eager greeting bytes');
  assert.equal(state.dataBeforeSecure(), false, 'secureConnect before first plaintext');
  assert.equal(state.client.authorized, true);
};

const runEcho = async (fixture, files) => {
  const link = await openTransport(fixture);
  const state = openClient(link.transport, files.ca);
  await awaitGreeting(state);
  assert.equal(state.client.servername, 'localhost');
  assert.equal(state.client.alpnProtocol, 'h1');
  await link.idle();
  const hold = link.holdNextWrite();
  let settled = false;
  let writeError;
  const writeDone = new Promise((resolve) => {
    state.client.write(Buffer.concat([echoed, echoPayload]), (error) => {
      settled = true;
      writeError = error;
      resolve();
    });
  });
  await hold.started;
  await delay(50);
  assert.equal(settled, false, 'TLS write callback waited for the transport');
  hold.release();
  await writeDone;
  assert.ok(writeError === undefined || writeError === null, 'gated write error');
  state.client.end();
  await state.finished;
  assert.equal(state.client.writableFinished, true);
  await state.ended;
  const expected = Buffer.concat([greeting, echoed, echoPayload]);
  assert.equal(state.bytes().length, expected.length);
  assert.ok(state.bytes().equals(expected), 'echo response bytes');
  await state.closed;
  assert.equal(state.counts.finish, 1);
  assert.equal(state.counts.end, 1);
  assert.equal(state.counts.close, 1);
  assert.equal(state.error(), null);
};

const runDelayed = async (fixture, files) => {
  const link = await openTransport(fixture);
  const state = openClient(link.transport, files.ca);
  await awaitGreeting(state);
  link.ignorePause();
  state.client.pause();
  await state.write(Buffer.from('d'));
  await waitFor(() => link.tcpEnded(), 'peer response and close_notify');
  await delay(50);
  assert.equal(state.client.isPaused(), true, 'readable stayed paused');
  state.client.resume();
  await state.ended;
  const expected = Buffer.concat([greeting, delayedPayload]);
  assert.equal(state.bytes().length, expected.length);
  assert.ok(state.bytes().equals(expected), 'delayed response bytes');
  assert.equal(state.counts.end, 1);
  assert.equal(state.error(), null);
  state.client.destroy();
  await state.closed;
  assert.equal(state.counts.close, 1);
  assert.ok(link.rawBytes() > delayedPayload.length);
};

const runWriteFailure = async (fixture, files) => {
  const link = await openTransport(fixture);
  const state = openClient(link.transport, files.ca);
  await state.secure;
  await link.idle();
  const localError = new Error('local transport refused the write');
  localError.code = 'EIO';
  link.failNextWrite(localError);
  let callbacks = 0;
  let callbackError;
  state.client.write(Buffer.alloc(64 * 1024, 0x2b), (error) => {
    callbacks += 1;
    callbackError = error;
  });
  await waitFor(() => callbacks === 1, 'failed write callback');
  assert.equal(callbackError.code, 'EIO');
  assert.equal(link.failedWrites(), 1);
  if (thinpack) {
    assert.equal(link.writesAfterFailure(), 0, 'remaining plaintext advanced after the error');
  }
  await state.closed;
  if (thinpack) {
    assert.equal(state.counts.error, 1);
    assert.equal(state.error(), callbackError);
  } else {
    assert.ok(state.counts.error >= 1);
  }
  assert.equal(state.counts.close, 1);
};

const runThinpackLifetime = async (files) => {
  const probe = new tls.TLSSocket(new PassThrough());
  const JSStreamSocket = probe._handle._parentWrap.constructor;
  assert.ok(JSStreamSocket.prototype instanceof net.Socket);
  probe.destroy();
  const [plain, peer] = duplexPair();
  const echo = new JSStreamSocket(plain);
  echo.encrypted = false;
  peer.on('data', (chunk) => peer.write(chunk));
  const echoedBytes = once(echo, 'data');
  echo.write('echo');
  assert.equal(String((await echoedBytes)[0]), 'echo');
  const echoClosed = once(echo, 'close');
  echo.destroy();
  await echoClosed;
  peer.destroy();

  let idleReads = 0;
  let idleWrites = 0;
  let idleEnds = 0;
  let idleDestroys = 0;
  const idleStream = new Duplex({
    read() { idleReads += 1; },
    write(chunk, encoding, callback) { idleWrites += 1; callback(); },
    final(callback) { idleEnds += 1; callback(); },
    destroy(error, callback) { idleDestroys += 1; callback(error); },
  });
  const idle = new tls.TLSSocket(idleStream, { ca: files.ca, servername: 'localhost' });
  assert.equal(idleReads, 0);
  assert.equal(idleWrites, 0);
  assert.equal(idleEnds, 0);
  assert.ok(idle._handle._parentWrap instanceof net.Socket);
  idle.on('data', () => {});
  idle.resume();
  await delay(0);
  assert.equal(idleReads, 0);
  const idleWrite = new Promise((resolve) => idle.write('queued', resolve));
  idle.destroy();
  assert.ok((await idleWrite) instanceof Error);
  assert.equal(idleDestroys, 1);
  assert.equal(idleEnds, 0);

  const earlySource = new PassThrough();
  const early = new tls.TLSSocket(earlySource, { ca: files.ca, servername: 'localhost' });
  let earlyErrors = 0;
  let earlyCloses = 0;
  early.on('error', () => { earlyErrors += 1; });
  early.on('close', () => { earlyCloses += 1; });
  earlySource.destroy(new Error('early adapter failure'));
  await delay(50);
  assert.equal(earlyErrors, 0);
  assert.equal(earlyCloses, 0);
  const earlyErrored = once(early, 'error');
  early._start();
  assert.equal((await earlyErrored)[0].message, 'early adapter failure');

  const earlyClosedSource = new PassThrough();
  const earlyClosed = new tls.TLSSocket(earlyClosedSource, {
    ca: files.ca,
    servername: 'localhost',
  });
  let earlyCloseErrors = 0;
  earlyClosed.on('error', () => { earlyCloseErrors += 1; });
  earlyClosedSource.destroy();
  await delay(50);
  assert.equal(earlyCloseErrors, 0);
  const earlyClosedOnce = once(earlyClosed, 'close');
  earlyClosed._start();
  await earlyClosedOnce;
  assert.equal(earlyCloseErrors, 0);

  const rejectedServer = new tls.TLSSocket(new PassThrough(), { isServer: true });
  const rejectedServerError = once(rejectedServer, 'error');
  rejectedServer._start();
  assert.equal((await rejectedServerError)[0].code, 'ERR_NOT_IMPLEMENTED');
  const rejectedConnect = new tls.TLSSocket(new PassThrough());
  const rejectedConnectError = once(rejectedConnect, 'error');
  rejectedConnect.connect({ isServer: true, ca: files.ca, servername: 'localhost' });
  assert.equal((await rejectedConnectError)[0].code, 'ERR_NOT_IMPLEMENTED');
};

const main = async (root = process.argv[2] ?? '/app') => {
  const files = {
    ca: readFileSync(`${root}/ca.pem`),
    key: readFileSync(`${root}/server.key`),
    cert: readFileSync(`${root}/server.pem`),
  };
  if (thinpack) await runThinpackLifetime(files);
  const fixture = await startTrustedFixture(files);
  try {
    await runEcho(fixture, files);
    console.log('gated write, eager greeting, echo EOF ok');
    await runDelayed(fixture, files);
    console.log('delayed read ok');
    await runWriteFailure(fixture, files);
    console.log('transport write error ok');
    console.log('tls duplex client ok');
  } finally {
    await fixture.close();
  }
};

module.exports = { main };

if (require.main === module) {
  main().catch((error) => {
    console.error(error);
    process.exit(1);
  });
}
