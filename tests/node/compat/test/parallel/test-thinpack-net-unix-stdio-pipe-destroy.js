'use strict';

if (process.platform !== 'win32') {
  const assert = require('node:assert/strict');
  const net = require('node:net');

  const socket = new net.Socket({ fd: 0, readable: true, writable: false });
  socket.on('data', () => assert.fail('the idle stdio pipe unexpectedly produced data'));
  socket.on('error', (error) => {
    console.error(error);
    process.exitCode = 1;
  });
  socket.once('close', () => assert.equal(socket.destroyed, true));
  setTimeout(() => socket.destroy(), 25);
}
