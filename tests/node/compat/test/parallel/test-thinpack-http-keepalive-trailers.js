'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');
const net = require('node:net');

(async () => {
  let connectionCount = 0;
  const sockets = new Set();
  const server = net.createServer((socket) => {
    connectionCount += 1;
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));

    let input = Buffer.alloc(0);
    let requestCount = 0;
    let trailersSent = false;
    let secondRequestSeen = false;
    let secondResponseSent = false;

    const sendSecondResponse = () => {
      if (!secondRequestSeen || !trailersSent || secondResponseSent) return;
      secondResponseSent = true;
      socket.write(
        'HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: keep-alive\r\n\r\n',
      );
      setTimeout(() => socket.write('second'), 5);
    };

    socket.on('data', (chunk) => {
      input = Buffer.concat([input, chunk]);
      for (;;) {
        const end = input.indexOf('\r\n\r\n');
        if (end < 0) return;
        input = input.subarray(end + 4);
        requestCount += 1;

        if (requestCount === 1) {
          socket.write(
            'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n' +
              'Trailer: X-Trailer\r\nConnection: keep-alive\r\n\r\n',
          );
          setTimeout(() => socket.write('5\r\nfirst\r\n'), 5);
          setTimeout(() => socket.write('0\r\n'), 15);
          // The trailers arrive after the terminator so premature socket reuse is observable.
          setTimeout(() => {
            socket.write('X-Trailer: done\r\n\r\n');
            trailersSent = true;
            sendSecondResponse();
          }, 100);
        } else if (requestCount === 2) {
          secondRequestSeen = true;
          sendSecondResponse();
        }
      }
    });
  });

  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });

  const agent = new http.Agent({ keepAlive: true, maxSockets: 1 });
  const port = server.address().port;
  const request = (path) => new Promise((resolve, reject) => {
    const req = http.get({ host: '127.0.0.1', port, path, agent }, (response) => {
      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.once('end', () => {
        resolve(Buffer.concat(chunks).toString());
      });
    });
    req.once('error', reject);
  });

  try {
    assert.equal(await request('/first'), 'first');
    assert.equal(await request('/second'), 'second');
    assert.equal(connectionCount, 1);
  } finally {
    agent.destroy();
    for (const socket of sockets) socket.destroy();
    await new Promise((resolve) => server.close(resolve));
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
