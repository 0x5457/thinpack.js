'use strict';

const { MessageChannel } = require('node:worker_threads');

const { port1, port2 } = new MessageChannel();
// Keep a ref'd read pending; closing the peer must wake it so the process exits.
port1.on('message', () => {});
port2.close();
