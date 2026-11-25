'use strict';

const assert = require('node:assert/strict');
const {
  addAbortListener,
  getEventListeners,
  listenerCount,
} = require('node:events');

{
  const target = new EventTarget();
  const listener = () => {};
  target.addEventListener('tick', listener);
  target.addEventListener('tick', listener);

  assert.equal(getEventListeners(target, 'tick')[0], listener);
  assert.equal(getEventListeners(target, 'tick').length, 1);
  assert.equal(listenerCount(target, 'tick'), 1);
  assert.deepEqual(Object.keys(target), []);
  assert.deepEqual(Object.getOwnPropertyNames(target), []);
  assert.equal(target._listeners, undefined);
}

{
  const target = new EventTarget();
  let calls = 0;
  const event = new Event('once');
  target.addEventListener('once', () => calls++, { once: true });
  target.dispatchEvent(event);
  target.dispatchEvent(new Event('once'));
  assert.equal(calls, 1);
  assert.deepEqual(getEventListeners(target, 'once'), []);
  assert.equal(event.currentTarget, null);
  assert.equal(event.eventPhase, Event.NONE);
}

(async () => {
  const controller = new AbortController();
  const { signal } = controller;
  assert.ok(signal instanceof EventTarget);

  let received;
  let receivedDuringDispatch;
  const listener = () => {};
  signal.addEventListener('abort', (event) => {
    received = event;
    receivedDuringDispatch = [event.currentTarget, event.eventPhase];
  });
  signal.addEventListener('abort', listener);
  assert.equal(getEventListeners(signal, 'abort')[1], listener);
  assert.equal(listenerCount(signal, 'abort'), 2);

  controller.abort('stop');
  assert.equal(signal.aborted, true);
  assert.equal(signal.reason, 'stop');
  assert.equal(received instanceof Event, true);
  assert.equal(received.type, 'abort');
  assert.equal(received.target, signal);
  assert.deepEqual(receivedDuringDispatch, [signal, Event.AT_TARGET]);
  assert.equal(received.currentTarget, null);
  assert.equal(received.eventPhase, Event.NONE);
  assert.equal(received.isTrusted, true);
  assert.equal(getEventListeners(signal, 'abort').length, 2);

  let onabortCalls = 0;
  const withHandler = new AbortController();
  withHandler.signal.onabort = () => onabortCalls++;
  withHandler.abort();
  assert.equal(onabortCalls, 1);

  const stopped = new AbortController();
  const order = [];
  stopped.signal.addEventListener('abort', (event) => {
    order.push('stop');
    event.stopImmediatePropagation();
  });
  const disposable = addAbortListener(stopped.signal, () => order.push('after'));
  stopped.abort();
  assert.deepEqual(order, ['stop', 'after']);
  disposable[Symbol.dispose]();

  const abortedSignal = AbortSignal.abort();
  const replayed = Promise.withResolvers();
  addAbortListener(abortedSignal, (event) => {
    replayed.resolve([event.target, event.currentTarget, event.isTrusted]);
  });
  assert.deepEqual(await replayed.promise, [abortedSignal, abortedSignal, true]);

  const first = new AbortController();
  const second = new AbortController();
  const combined = AbortSignal.any([first.signal, second.signal]);
  second.abort('combined');
  assert.equal(combined.aborted, true);
  assert.equal(combined.reason, 'combined');
  assert.equal(getEventListeners(first.signal, 'abort').length, 0);
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
