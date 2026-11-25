'use strict';

// A connected UDP socket keeps its default peer across a disconnect and a
// reconnect; a real datagram must reach the loopback server after each connect.

const common = require('../common');
const assert = require('assert');
const dgram = require('dgram');

const server = dgram.createSocket('udp4');
const client = dgram.createSocket('udp4');
const received = [];

server.on('message', common.mustCall((message, { address }) => {
  received.push(message.toString());
  assert.strictEqual(address, '127.0.0.1');
  if (received.length === 2) {
    assert.deepStrictEqual(received, ['first', 'second']);
    client.close();
    server.close();
  }
}, 2));

server.bind(0, '127.0.0.1', common.mustCall(() => {
  const { port } = server.address();

  client.connect(port, '127.0.0.1', common.mustCall((error) => {
    assert.ifError(error);
    const remote = client.remoteAddress();
    assert.strictEqual(remote.address, '127.0.0.1');
    assert.strictEqual(remote.family, 'IPv4');
    assert.strictEqual(remote.port, port);

    client.send('first', common.mustCall((error) => {
      assert.ifError(error);

      client.disconnect();
      assert.throws(() => client.remoteAddress(), {
        code: 'ERR_SOCKET_DGRAM_NOT_CONNECTED',
      });

      client.connect(port, '127.0.0.1', common.mustCall((error) => {
        assert.ifError(error);
        assert.strictEqual(client.remoteAddress().port, port);
        client.send('second', common.mustCall((error) => assert.ifError(error)));
      }));
    }));
  }));
}));
