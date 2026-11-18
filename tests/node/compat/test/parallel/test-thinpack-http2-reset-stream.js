'use strict';

const assert = require('node:assert/strict');
const http2 = require('node:http2');

const server = http2.createServer();
server.on('stream', (stream) => {
  stream.on('error', () => {});
  // Destroy with an error to send an RST_STREAM instead of a normal end.
  stream.respond({ ':status': 200 });
  stream.write('partial response', () => {
    setTimeout(() => stream.destroy(new Error('reset HTTP/2 stream')), 10);
  });
});

(async () => {
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });

  const session = http2.connect(`http://127.0.0.1:${server.address().port}`);
  session.on('error', () => {});
  const request = session.request({ ':path': '/' });
  request.on('error', () => {});
  request.resume();

  try {
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error('reset client stream did not close'));
      }, 2000);
      request.once('close', () => {
        clearTimeout(timeout);
        resolve();
      });
      request.end();
    });

    assert.notEqual(request.rstCode, http2.constants.NGHTTP2_NO_ERROR);
    assert.equal(request.destroyed, true);
  } finally {
    session.destroy();
    server.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
