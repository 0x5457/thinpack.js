'use strict';

const assert = require('node:assert/strict');
const http2 = require('node:http2');

const BODY = Buffer.from([0x00, 0x01, 0x7f, 0x80, 0xff]);
const GOAWAY_OPAQUE = Buffer.from([0xde, 0xad]);

const server = http2.createServer();
let serverSession;

server.on('session', (session) => {
  serverSession = session;
  assert.equal(session.socket.remoteAddress, '127.0.0.1');
  assert.ok(session.socket.localPort > 0);
});

server.on('stream', (stream, headers) => {
  if (headers[':path'] === '/goaway') {
    stream.respond({ ':status': 204 }, { endStream: true });
    setTimeout(() => {
      serverSession.goaway(http2.constants.NGHTTP2_NO_ERROR, 0, GOAWAY_OPAQUE);
    }, 10);
    return;
  }
  assert.equal(headers['x-echo'], 'yes');
  assert.equal(headers['x-repeat'], 'one, two');
  stream.respond(
    {
      ':status': 200,
      'content-type': 'application/octet-stream',
      'set-cookie': ['a=1; Path=/', 'b=2; Path=/'],
      'x-repeat': ['first', 'second'],
    },
    { waitForTrailers: true },
  );
  stream.on('wantTrailers', () => {
    stream.sendTrailers({ 'x-trailer': 'done' });
  });
  stream.end(BODY);
});

(async () => {
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  assert.ok(server.address().port > 0);

  const session = http2.connect(`http://127.0.0.1:${server.address().port}`);
  session.on('error', () => {});
  try {
    const request = session.request({
      ':path': '/typed',
      'x-echo': 'yes',
      'x-repeat': ['one', 'two'],
    });
    request.on('error', () => {});

    const response = new Promise((resolve) => request.once('response', resolve));
    const trailers = new Promise((resolve) => request.once('trailers', resolve));
    request.end();

    const headers = await response;
    assert.equal(Number(headers[':status']), 200);
    assert.deepEqual(headers['set-cookie'], ['a=1; Path=/', 'b=2; Path=/']);
    assert.equal(headers['x-repeat'], 'first, second');
    assert.equal((await trailers)['x-trailer'], 'done');

    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    assert.deepEqual(Buffer.concat(chunks), BODY);

    const goaway = new Promise((resolve) => {
      session.once('goaway', (code, lastStreamID, opaque) => {
        resolve({ code, lastStreamID, opaque });
      });
    });
    const control = session.request({ ':path': '/goaway' });
    control.on('error', () => {});
    control.resume();
    control.end();

    const event = await goaway;
    assert.equal(event.code, http2.constants.NGHTTP2_NO_ERROR);
    assert.equal(typeof event.lastStreamID, 'number');
    assert.deepEqual(Buffer.from(event.opaque), GOAWAY_OPAQUE);
  } finally {
    session.destroy();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
