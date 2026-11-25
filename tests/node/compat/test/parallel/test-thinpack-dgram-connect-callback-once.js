'use strict';

// A throw from a 'connect' listener must not be mistaken for a connect failure.
// Node records the connected state and queues the successful emit outside the
// catch, so the throw escapes as an uncaught exception; the socket stays
// connected and the listener is not called a second time with the thrown error.
//
// This runtime has no process.on('uncaughtException'), so the throw ends the
// child process. The parent asserts on what the child reported before the throw
// and on the error that ended it.

const assert = require('assert');
const { spawnSync } = require('child_process');
const dgram = require('dgram');

function child() {
  const socket = dgram.createSocket('udp4');
  let calls = 0;

  socket.connect(12345, '127.0.0.1', (error) => {
    calls += 1;
    if (calls > 1) {
      // Only reachable when the successful emit is still inside the catch:
      // it resets the connected state and queues this callback with the error.
      console.log(`redelivered:${error && error.message}`);
      try {
        socket.remoteAddress();
      } catch (error) {
        console.log(`state:${error.code}`);
      }
      socket.close();
      return;
    }
    assert.strictEqual(error, undefined);
    console.log(`connected:${socket.remoteAddress().port}`);
    throw new Error('connect-listener-threw');
  });
}

function parent() {
  const { status, stdout, stderr } = spawnSync(
    process.execPath,
    [__filename, 'child'],
    { encoding: 'utf8' },
  );

  assert.deepStrictEqual(stdout.trim().split('\n'), ['connected:12345']);
  assert.match(stderr, /connect-listener-threw/);
  assert.notStrictEqual(status, 0);
}

if (process.argv[2] === 'child') child();
else parent();
