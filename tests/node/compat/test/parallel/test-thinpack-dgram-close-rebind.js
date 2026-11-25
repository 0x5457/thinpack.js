'use strict';
const common = require('../common');
const assert = require('assert');
const dgram = require('dgram');

// Closing a bound socket must cancel its pending receive and release the port
// for an immediate rebind, not leave per-runtime state behind.
const first = dgram.createSocket('udp4');
first.on('error', common.mustNotCall());
first.bind(0, '127.0.0.1', common.mustCall(() => {
  const port = first.address().port;
  first.close(common.mustCall(() => {
    const second = dgram.createSocket('udp4');
    second.on('error', common.mustNotCall());
    second.on('message', common.mustCall((message, rinfo) => {
      assert.strictEqual(message.toString(), 'after-close');
      assert.strictEqual(rinfo.address, '127.0.0.1');
      second.close(common.mustCall());
    }));
    second.bind(port, '127.0.0.1', common.mustCall(() => {
      const sender = dgram.createSocket('udp4');
      sender.send(
        'after-close',
        port,
        '127.0.0.1',
        common.mustCall((error, bytes) => {
          assert.ifError(error);
          assert.strictEqual(bytes, 11);
          sender.close();
        }),
      );
    }));
  }));
}));
