'use strict';

const assert = require('node:assert/strict');
const { AsyncLocalStorage } = require('node:async_hooks');

const storage = new AsyncLocalStorage();
const seen = [];

const record = (label) => () => {
  seen.push([label, storage.getStore()]);
};

async function main() {
  assert.equal(storage.getStore(), undefined);

  await storage.run({ request: 'outer' }, async () => {
    assert.equal(storage.getStore().request, 'outer');

    storage.run({ request: 'inner' }, () => {
      assert.equal(storage.getStore().request, 'inner');
    });
    assert.equal(storage.getStore().request, 'outer');

    Promise.resolve().then(record('promise'));
    queueMicrotask(record('microtask'));
    process.nextTick(record('nextTick'));
    setTimeout(record('timeout'), 1);
    setImmediate(record('immediate'));

    await null;
    assert.equal(storage.getStore().request, 'outer');

    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(storage.getStore().request, 'outer');
  });

  assert.equal(storage.getStore(), undefined);
  assert.deepEqual(seen.map(([label]) => label).sort(), [
    'immediate',
    'microtask',
    'nextTick',
    'promise',
    'timeout',
  ]);
  for (const [label, store] of seen) assert.equal(store.request, 'outer', label);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
