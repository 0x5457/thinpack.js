// Copyright 2018-2026 the Deno authors. MIT license.
// Adapted from Deno v2.9.8 ext/node/polyfills/testing.ts
// (denoland/deno e7b39ebd55c85aaf6a29290c1f5a99e2e21e09dd), with the
// registration and export shape from Node.js v26.13.1 lib/test.js.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

'use strict';

const { AsyncLocalStorage } = require('node:async_hooks');
const { codes } = require('node:thinpack/internal/errors');

const contexts = new AsyncLocalStorage();

let root;
let rootIndex = 0;
let rootBeforeRan = false;
let rootAfterRan = false;
let drainScheduled = false;
let draining = false;
let testCount = 0;
let suiteCount = 0;
let passCount = 0;
let failCount = 0;
let skippedCount = 0;
let todoCount = 0;
let runFailed = false;

function makeSuite(name, parent, options) {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return {
    kind: 'suite',
    name,
    parent,
    options,
    children: [],
    before: [],
    after: [],
    beforeEach: [],
    afterEach: [],
    body: undefined,
    bodyError: undefined,
    promise,
    resolve,
    context: undefined,
  };
}

function makeTest(name, fn, options, parent) {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { kind: 'test', name, fn, options, parent, promise, resolve };
}

function normalize(name, options, fn, overrides = {}) {
  if (typeof name === 'function') {
    fn = name;
    name = fn.name;
    options = {};
  } else if (name !== null && typeof name === 'object') {
    fn = options;
    options = name;
    name = undefined;
  } else if (typeof options === 'function') {
    fn = options;
    options = {};
  }

  if (options === null || typeof options !== 'object') options = {};
  if (typeof fn !== 'function') fn = () => {};
  if (typeof name !== 'string' || name.length === 0) name = fn.name || '<anonymous>';
  return { name, fn, options: { ...options, ...overrides } };
}

function store() {
  return contexts.getStore() || {};
}

function queueRun() {
  if (drainScheduled || draining) return;
  drainScheduled = true;
  setTimeout(executeRun, 0);
}

function addTest(name, options, fn, overrides) {
  const prepared = normalize(name, options, fn, overrides);
  const current = store();
  if (current.test) return current.test.test(prepared.name, prepared.options, prepared.fn);

  const parent = current.suite || root;
  const entry = makeTest(prepared.name, prepared.fn, prepared.options, parent);
  parent.children.push(entry);
  queueRun();
  return parent === root ? entry.promise : Promise.resolve();
}

function addSuite(name, options, fn, overrides) {
  const prepared = normalize(name, options, fn, overrides);
  const current = store();

  if (current.test) {
    return current.test.test(prepared.name, prepared.options, async (context) => {
      const entry = makeSuite(prepared.name, context, prepared.options);
      entry.context = context;
      const nestedStore = { ...store(), suite: entry, test: undefined };
      await contexts.run(nestedStore, () => prepared.fn(context));
      const suites = [...(current.suites || []), entry];
      for (let i = 0; i < entry.children.length; i++) {
        await runEntry(entry.children[i], i + 1, suites, current.tests || []);
      }
    });
  }

  const parent = current.suite || root;
  const entry = makeSuite(prepared.name, parent, prepared.options);
  entry.context = new TestContext(prepared.name, parent.context || null);
  parent.children.push(entry);
  queueRun();

  const suites = [...(current.suites || (parent === root ? [root] : [])), entry];
  try {
    entry.body = contexts.run(
      { ...current, suite: entry, test: undefined, suites },
      () => prepared.fn(entry.context),
    );
  } catch (error) {
    entry.bodyError = error;
  }
  return parent === root ? entry.promise : Promise.resolve();
}

class TestContext {
  #name;
  #parent;
  #abortController = new AbortController();
  #before = [];
  #after = [];
  #beforeEach = [];
  #afterEach = [];
  #subtestTail = Promise.resolve();
  #subtestCount = 0;
  #subtestFailed = false;
  #beforeRan = false;
  #afterRan = false;
  #mock;
  #skip;
  #todo;

  constructor(name, parent = null) {
    this.#name = name;
    this.#parent = parent;
  }

  get name() { return this.#name; }

  get fullName() {
    if (!this.#parent || this.#parent.name === '<root>') return this.#name;
    return `${this.#parent.fullName} > ${this.#name}`;
  }

  get signal() { return this.#abortController.signal; }

  get mock() { return this.#mock ??= new MockTracker(); }

  diagnostic(message) {
    console.log(`# ${String(message)}`);
  }

  skip(message) {
    this.#skip ??= message ?? true;
  }

  todo(message) {
    this.#todo ??= message ?? true;
  }

  before(fn) { this.#before.push(requireFunction(fn, 'before')); }
  after(fn) { this.#after.push(requireFunction(fn, 'after')); }
  beforeEach(fn) { this.#beforeEach.push(requireFunction(fn, 'beforeEach')); }
  afterEach(fn) { this.#afterEach.push(requireFunction(fn, 'afterEach')); }

  test(name, options, fn) {
    const prepared = normalize(name, options, fn);
    const entry = makeTest(prepared.name, prepared.fn, prepared.options, this);
    const number = ++this.#subtestCount;
    const current = store();
    const suites = current.suites || [root];
    const tests = current.tests || [this];
    const promise = this.#subtestTail.then(async () => {
      await this._runBeforeOnce(current);
      const result = await runEntry(entry, number, suites, tests);
      if (!result.ok) this.#subtestFailed = true;
      return result;
    });
    this.#subtestTail = promise.then(() => undefined, () => undefined);
    return promise;
  }

  _beforeHooks() { return this.#before; }
  _afterHooks() { return this.#after; }
  _directives() { return { skip: this.#skip, todo: this.#todo }; }

  _resetMock() { this.#mock?.reset(); }

  async _runBeforeOnce(current) {
    if (this.#beforeRan) return;
    this.#beforeRan = true;
    const context = { ...current, test: this };
    for (const hook of this.#before) await contexts.run(context, () => call(hook, this));
  }

  async _runAfterOnce(current) {
    if (this.#afterRan) return;
    this.#afterRan = true;
    const context = { ...current, test: this };
    for (const hook of this.#after) await contexts.run(context, () => call(hook, this));
  }

  async _runBeforeEach(child, current) {
    const context = { ...current, test: this };
    for (const hook of this.#beforeEach) await contexts.run(context, () => call(hook, child));
  }

  async _runAfterEach(child, current) {
    const context = { ...current, test: this };
    for (const hook of [...this.#afterEach].reverse()) await contexts.run(context, () => call(hook, child));
  }

  async _finish(current) {
    await this.#subtestTail;
    await this._runAfterOnce(current);
    if (this.#subtestFailed) throw new Error('one or more subtests failed');
  }

  _abort() {
    if (!this.signal.aborted) this.#abortController.abort();
  }
}

root = makeSuite('<root>', null, {});
root.context = new TestContext('<root>');

function requireFunction(fn, name) {
  if (typeof fn !== 'function') throw new TypeError(`${name}() requires a function`);
  return fn;
}

function call(fn, context) {
  if (fn.length < 2) return Promise.resolve(fn(context));
  return new Promise((resolve, reject) => {
    let complete = false;
    const done = (error) => {
      if (complete) return;
      complete = true;
      if (error) reject(error);
      else resolve();
    };
    try {
      const result = fn(context, done);
      if (result && typeof result.then === 'function') {
        result.catch(() => {});
        reject(new TypeError('callback and promise results are mutually exclusive'));
      }
    } catch (error) {
      reject(error);
    }
  });
}

async function runFunction(fn, context, timeout, signal) {
  // Invoke synchronously: the runner calls this inside AsyncLocalStorage.run, and
  // engines without promise hooks (JSC) lose the store across the microtask.
  const body = call(fn, context);
  const races = [body];
  let timeoutId;
  let abortHandler;

  if (typeof timeout === 'number' && timeout > 0) {
    races.push(new Promise((_, reject) => {
      timeoutId = setTimeout(() => {
        context._abort();
        reject(new Error(`test timed out after ${timeout}ms`));
      }, timeout);
    }));
  }
  if (signal) {
    races.push(new Promise((_, reject) => {
      abortHandler = () => reject(signal.reason || new Error('The test was aborted'));
      if (signal.aborted) abortHandler();
      else signal.addEventListener('abort', abortHandler, { once: true });
    }));
  }

  try {
    return await Promise.race(races);
  } finally {
    if (timeoutId !== undefined) clearTimeout(timeoutId);
    if (abortHandler && signal) signal.removeEventListener('abort', abortHandler);
  }
}

async function runHookList(hooks, context, activeStore) {
  for (const hook of hooks) await contexts.run(activeStore, () => call(hook, context));
}

async function runEachHooks(suites, kind, context, activeStore, reverse = false) {
  const chain = reverse ? [...suites].reverse() : suites;
  for (const suite of chain) {
    await runHookList(suite[kind], context, {
      ...activeStore,
      suite,
      test: suite.context || activeStore.test,
    });
  }
}

function write(depth, text) {
  console.log(`${'  '.repeat(depth)}${text}`);
}

function directive(options) {
  if (options.skip) return ` # SKIP${options.skip === true ? '' : ` ${String(options.skip)}`}`;
  if (options.todo) return ` # TODO${options.todo === true ? '' : ` ${String(options.todo)}`}`;
  return '';
}

function writeError(depth, error) {
  write(depth, '  ---');
  write(depth, `  message: ${JSON.stringify(error?.message || String(error))}`);
  if (error?.stack) write(depth, `  stack: ${JSON.stringify(error.stack)}`);
  write(depth, '  ...');
}

function parentContext(parent) {
  if (parent instanceof TestContext) return parent;
  return parent?.context || null;
}

async function runEntry(entry, number, suites, tests, forceSkip = false) {
  const depth = Math.max(0, suites.length - 1);
  write(depth, `# Subtest: ${entry.name}`);

  let error;
  let skippedEntry = forceSkip || Boolean(entry.options.skip);
  const parentTodo = suites.find((suite) => suite.options.todo)?.options.todo;
  let todoReason = entry.options.todo || parentTodo;
  let skipReason = entry.options.skip;
  let wasTodo = !skippedEntry && Boolean(todoReason);
  let childFailed = false;

  if (skippedEntry) {
    if (entry.kind === 'test') {
      testCount++;
      skippedCount++;
    } else {
      suiteCount++;
    }
  } else if (entry.kind === 'suite') {
    suiteCount++;
    const context = entry.context || new TestContext(entry.name, parentContext(entry.parent));
    entry.context = context;
    const suiteChain = [...suites, entry];
    const activeStore = { suite: entry, test: context, suites: suiteChain, tests };
    try {
      if (entry.bodyError) throw entry.bodyError;
      if (entry.body && typeof entry.body.then === 'function') await entry.body;
      await runHookList(entry.before, context, activeStore);
      await runHookList(context._beforeHooks(), context, activeStore);
      const only = entry.children.some((child) => child.options.only);
      for (let i = 0; i < entry.children.length; i++) {
        const child = entry.children[i];
        const result = await runEntry(
          child,
          i + 1,
          suiteChain,
          tests,
          only && !child.options.only,
        );
        if (!result.ok) childFailed = true;
      }
    } catch (caught) {
      error = caught;
    } finally {
      try {
        await runHookList(context._afterHooks(), context, activeStore);
        await runHookList(entry.after, context, activeStore);
      } catch (caught) {
        error ||= caught;
      }
      context._resetMock();
    }
  } else {
    testCount++;
    const context = new TestContext(entry.name, parentContext(entry.parent));
    const activeStore = {
      suite: suites[suites.length - 1] || root,
      test: context,
      suites,
      tests: [...tests, context],
    };
    let beforeEachComplete = false;
    try {
      await runEachHooks(suites, 'beforeEach', context, activeStore);
      for (const parent of tests) await parent._runBeforeOnce(activeStore);
      for (const parent of tests) await parent._runBeforeEach(context, activeStore);
      beforeEachComplete = true;

      let bodyError;
      try {
        await contexts.run(activeStore, () => runFunction(entry.fn, context, entry.options.timeout, entry.options.signal));
      } catch (caught) {
        bodyError = caught;
      }

      try {
        await context._finish(activeStore);
      } catch (caught) {
        bodyError ||= caught;
      }

      const directives = context._directives();
      if (directives.skip !== undefined) {
        skippedEntry = true;
        wasTodo = false;
        skipReason = directives.skip;
      } else if (directives.todo !== undefined) {
        wasTodo = true;
        todoReason = directives.todo;
      }

      if (entry.options.expectFailure) {
        if (!bodyError) throw new Error('test was expected to fail but passed');
      } else if (bodyError) {
        throw bodyError;
      }
    } catch (caught) {
      error = caught;
    } finally {
      if (beforeEachComplete) {
        try {
          for (const parent of [...tests].reverse()) await parent._runAfterEach(context, activeStore);
          await runEachHooks(suites, 'afterEach', context, activeStore, true);
        } catch (caught) {
          error ||= caught;
        }
      }
      context._resetMock();
    }

    if (skippedEntry) skippedCount++;
    else if (wasTodo) todoCount++;
  }

  const ok = !error && !childFailed;
  const suffix = skippedEntry
    ? directive({ skip: skipReason || true })
    : wasTodo ? directive({ todo: todoReason || true }) : '';

  if (skippedEntry) {
    write(depth, `ok ${number} - ${entry.name}${suffix}`);
  } else if (wasTodo) {
    if (ok) write(depth, `ok ${number} - ${entry.name}${suffix}`);
    else {
      write(depth, `not ok ${number} - ${entry.name}${suffix}`);
      writeError(depth, error || new Error('one or more subtests failed'));
    }
  } else if (ok) {
    if (entry.kind === 'test') passCount++;
    write(depth, `ok ${number} - ${entry.name}`);
  } else {
    runFailed = true;
    if (entry.kind === 'test') failCount++;
    write(depth, `not ok ${number} - ${entry.name}`);
    writeError(depth, error || new Error('one or more subtests failed'));
  }

  const result = { ok: ok || skippedEntry || wasTodo, skipped: skippedEntry, todo: wasTodo };
  entry.resolve?.(result);
  return result;
}

// Registration is asynchronous: `test()` and the hook helpers schedule a drain
// on the next tick, so top-level registrations batch into one run. A
// registration that lands after a drain finished (from a top-level await
// continuation, for example) schedules another drain instead of being dropped,
// which would exit 0 with a failing test never executed. Root before/after
// hooks run once, around the first drain; a later drain runs only the entries
// that arrived after it.
async function executeRun() {
  drainScheduled = false;
  draining = true;
  const activeStore = { suite: root, test: root.context, suites: [root], tests: [] };
  let rootError;
  try {
    if (!rootBeforeRan) {
      rootBeforeRan = true;
      write(0, 'TAP version 13');
      try {
        await runHookList(root.before, root.context, activeStore);
      } catch (error) {
        rootError = error;
      }
    }
    // A failing root before hook cancels the tests registered so far, as in
    // Node; they are not started and the root failure is reported below.
    if (!rootError) {
      while (rootIndex < root.children.length) {
        const entry = root.children[rootIndex];
        rootIndex++;
        // `only` marks the selected siblings; the rest are reported as skipped and
        // never executed, the way Node's runner treats them.
        const only = root.children.some((child) => child.options.only);
        await runEntry(entry, rootIndex, [root], [], only && !entry.options.only);
      }
    }
    if (!rootAfterRan) {
      rootAfterRan = true;
      try {
        await runHookList(root.after, root.context, activeStore);
      } catch (error) {
        rootError ||= error;
      }
    }
  } finally {
    draining = false;
  }

  if (rootError) {
    runFailed = true;
    write(0, '# root hook failed');
    writeError(0, rootError);
  }
  write(0, `1..${root.children.length}`);
  write(0, `# tests ${testCount}`);
  write(0, `# suites ${suiteCount}`);
  write(0, `# pass ${passCount}`);
  write(0, `# fail ${failCount}`);
  write(0, `# skipped ${skippedCount}`);
  write(0, `# todo ${todoCount}`);
  if (runFailed) globalThis.process.exitCode = 1;
  if (rootIndex < root.children.length) queueRun();
}

function run() {
  throw new codes.ERR_NOT_IMPLEMENTED('node:test run()');
}

function registerHook(kind, fn) {
  requireFunction(fn, kind);
  const current = store();
  if (current.test) current.test[kind](fn);
  else (current.suite || root)[kind].push(fn);
  queueRun();
}

function before(fn) { registerHook('before', fn); }
function after(fn) { registerHook('after', fn); }
function beforeEach(fn) { registerHook('beforeEach', fn); }
function afterEach(fn) { registerHook('afterEach', fn); }
function test(name, options, fn, overrides) { return addTest(name, options, fn, overrides); }
function suite(name, options, fn, overrides) { return addSuite(name, options, fn, overrides); }

function getTestContext() {
  const current = store();
  return current.test || current.suite?.context;
}

function variant(registration, option, value) {
  return (name, options, fn) => registration(name, options, fn, { [option]: value });
}

test.skip = variant(test, 'skip', true);
test.todo = variant(test, 'todo', true);
test.only = variant(test, 'only', true);
suite.skip = variant(suite, 'skip', true);
suite.todo = variant(suite, 'todo', true);
suite.only = variant(suite, 'only', true);
test.test = test;
test.it = test;
test.describe = suite;
test.suite = suite;

class MockFunctionContext {
  #calls = [];
  #implementation;
  #restore;
  #times;
  #tracker;
  #once = new Map();

  constructor(tracker, implementation, restore, times) {
    this.#tracker = tracker;
    this.#implementation = implementation;
    this.#restore = restore;
    this.#times = times;
  }

  get calls() { return this.#calls.slice(); }
  callCount() { return this.#calls.length; }

  mockImplementation(implementation) {
    this.#implementation = requireFunction(implementation, 'mockImplementation');
  }

  mockImplementationOnce(implementation, onCall = this.#calls.length) {
    this.#once.set(onCall, requireFunction(implementation, 'mockImplementationOnce'));
  }

  mockReturnValue(value) { this.#implementation = () => value; }
  mockResolvedValue(value) { this.#implementation = () => Promise.resolve(value); }
  mockRejectedValue(value) { this.#implementation = () => Promise.reject(value); }
  mockThrownValue(value) { this.#implementation = () => { throw value; }; }
  resetCalls() { this.#calls = []; }

  restore() {
    if (this.#restore) {
      this.#restore();
      this.#restore = null;
    }
    this.#tracker._remove(this);
  }

  _next() {
    const index = this.#calls.length;
    const implementation = this.#once.get(index) || this.#implementation;
    this.#once.delete(index);
    return implementation;
  }

  _record(thisArg, args, result, error, target) {
    this.#calls.push({ arguments: args, error, result, this: thisArg, target });
    if (this.#times !== undefined && this.#calls.length >= this.#times) this.restore();
  }
}

// A tracker owns the mocks it created and restores all of them on `reset()`.
// Node hands every test its own tracker (`t.mock`), reset when that test
// finishes, and keeps the module-level `mock` export as a separate tracker that
// only resets when the test asks for it.
class MockTracker {
  #mocks = [];

  fn(original, implementation, options) {
    if (original && typeof original === 'object') {
      options = original;
      original = undefined;
      implementation = undefined;
    } else if (implementation && typeof implementation === 'object') {
      options = implementation;
      implementation = undefined;
    }
    return mockFunction(this, original, implementation || original, options || {});
  }

  method(object, key, implementation, options) {
    if (implementation && typeof implementation === 'object') {
      options = implementation;
      implementation = undefined;
    }
    const descriptor = findDescriptor(object, key);
    if (!descriptor || typeof descriptor.value !== 'function') {
      throw new TypeError(`Cannot mock property '${String(key)}' because it is not a function`);
    }

    const ownDescriptor = Object.getOwnPropertyDescriptor(object, key);
    const restore = () => {
      if (ownDescriptor) Object.defineProperty(object, key, ownDescriptor);
      else delete object[key];
    };
    const mocked = mockFunction(this, descriptor.value, implementation, options || {}, restore);
    Object.defineProperty(object, key, { ...descriptor, value: mocked });
    return mocked;
  }

  reset() {
    while (this.#mocks.length) this.#mocks[this.#mocks.length - 1].restore();
  }

  restoreAll() { this.reset(); }

  _add(context) { this.#mocks.push(context); }

  _remove(context) {
    const index = this.#mocks.indexOf(context);
    if (index !== -1) this.#mocks.splice(index, 1);
  }
}

function mockFunction(tracker, original, implementation, options = {}, restore = null) {
  const context = new MockFunctionContext(tracker, implementation || original, restore, options.times);
  tracker._add(context);
  const fn = function (...args) {
    const impl = context._next();
    let result;
    try {
      if (new.target && impl) result = Reflect.construct(impl, args, new.target === fn ? impl : new.target);
      else if (impl) result = Reflect.apply(impl, this, args);
    } catch (error) {
      context._record(this, args, undefined, error, new.target);
      throw error;
    }
    context._record(this, args, result, undefined, new.target);
    return result;
  };
  Object.defineProperty(fn, 'mock', { value: context });
  return fn;
}

function findDescriptor(object, key) {
  for (let current = object; current; current = Object.getPrototypeOf(current)) {
    const descriptor = Object.getOwnPropertyDescriptor(current, key);
    if (descriptor) return descriptor;
  }
}

const mock = new MockTracker();

module.exports = {
  TestContext,
  after,
  afterEach,
  before,
  beforeEach,
  describe: suite,
  getTestContext,
  it: test,
  mock,
  run,
  suite,
  test,
};
