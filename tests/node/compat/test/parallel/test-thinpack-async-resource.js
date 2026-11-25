'use strict';

const assert = require('node:assert/strict');
const {
  AsyncLocalStorage,
  AsyncResource,
  executionAsyncId,
  executionAsyncResource,
  triggerAsyncId,
} = require('node:async_hooks');

const storage = new AsyncLocalStorage();

const triggerId = executionAsyncId();
const resource = storage.run('captured', () => new AsyncResource('TestResource'));

assert.equal(resource.triggerAsyncId(), triggerId);
assert.equal(storage.getStore(), undefined);

const scope = () => ({
  asyncId: executionAsyncId(),
  triggerAsyncId: triggerAsyncId(),
  resource: executionAsyncResource(),
  store: storage.getStore(),
});
const expected = {
  asyncId: resource.asyncId(),
  triggerAsyncId: triggerId,
  resource,
  store: 'captured',
};

assert.deepEqual(resource.runInAsyncScope(scope), expected);
assert.equal(executionAsyncId(), triggerId);
assert.equal(storage.getStore(), undefined);

const bound = resource.bind(function add(left, right) {
  return { ...scope(), sum: left + right };
});
assert.equal(bound.length, 2);
assert.deepEqual(bound(1, 2), { ...expected, sum: 3 });

const staticBound = storage.run('static', () =>
  AsyncResource.bind(() => storage.getStore(), 'StaticBinding'));
assert.equal(staticBound(), 'static');
