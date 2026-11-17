"use strict";

const assert = require("node:assert");
const {
  AsyncLocalStorage,
  AsyncResource,
  createHook,
  executionAsyncId,
  executionAsyncResource,
  triggerAsyncId,
} = require("node:async_hooks");

const waitFor = (milliseconds) =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));
const trackResources = () => {
  const types = new Map();
  const labels = new Map();
  const events = [];
  const label = (asyncId) => {
    if (!labels.has(asyncId)) labels.set(asyncId, `#${labels.size + 1}`);
    return labels.get(asyncId);
  };
  const record = (event, asyncId) => {
    const type = types.get(asyncId);
    if (type === undefined || type === "PROMISE") return;
    events.push(`${event} ${label(asyncId)}`);
  };
  const hook = createHook({
    init(asyncId, type) {
      types.set(asyncId, type);
      if (type !== "PROMISE") events.push(`init ${type} ${label(asyncId)}`);
    },
    before: (asyncId) => record("before", asyncId),
    after: (asyncId) => record("after", asyncId),
    destroy: (asyncId) => record("destroy", asyncId),
  });
  return { events, hook, label };
};

const tests = [];
const test = (name, run) => tests.push({ name, run });

test("root execution context", () => {
  assert.strictEqual(executionAsyncId(), 1);
  assert.strictEqual(triggerAsyncId(), 0);
  assert.strictEqual(typeof executionAsyncResource(), "object");
  assert.notStrictEqual(executionAsyncResource(), null);
});

test("AsyncLocalStorage follows promises, ticks and timers", async () => {
  const storage = new AsyncLocalStorage();
  const seen = {};
  assert.strictEqual(storage.getStore(), undefined);

  await storage.run({ id: "outer" }, async () => {
    seen.sync = storage.getStore().id;
    Promise.resolve().then(() => {
      seen.then = storage.getStore().id;
    });
    queueMicrotask(() => {
      seen.microtask = storage.getStore().id;
    });
    process.nextTick(() => {
      seen.nextTick = storage.getStore().id;
    });
    setTimeout(() => {
      seen.timeout = storage.getStore().id;
    }, 1);
    setImmediate(() => {
      seen.immediate = storage.getStore().id;
    });
    const interval = setInterval(() => {
      seen.interval = storage.getStore().id;
      clearInterval(interval);
    }, 1);
    await null;
    seen.await = storage.getStore().id;
    await waitFor(4);
  });

  seen.outside = storage.getStore();
  assert.deepStrictEqual(seen, {
    sync: "outer",
    then: "outer",
    microtask: "outer",
    nextTick: "outer",
    timeout: "outer",
    immediate: "outer",
    interval: "outer",
    await: "outer",
    outside: undefined,
  });
});

test("AsyncLocalStorage enterWith, exit and disable", () => {
  const storage = new AsyncLocalStorage();
  const withDefault = new AsyncLocalStorage({ defaultValue: "fallback" });
  assert.strictEqual(withDefault.getStore(), "fallback");

  storage.enterWith("entered");
  assert.strictEqual(storage.getStore(), "entered");
  assert.strictEqual(storage.exit(() => storage.getStore()), undefined);
  assert.strictEqual(storage.getStore(), "entered");
  assert.deepStrictEqual(storage.run("value", (...args) => args, "a", "b"), [
    "a",
    "b",
  ]);
  assert.strictEqual(storage.getStore(), "entered");
  storage.disable();
  assert.strictEqual(storage.getStore(), undefined);
  assert.strictEqual(withDefault.getStore(), "fallback");
});

test("nested AsyncLocalStorage instances", () => {
  const outer = new AsyncLocalStorage();
  const inner = new AsyncLocalStorage();
  outer.run("outer", () => {
    inner.run("inner", () => {
      assert.strictEqual(outer.getStore(), "outer");
      assert.strictEqual(inner.getStore(), "inner");
    });
    assert.strictEqual(inner.getStore(), undefined);
    assert.strictEqual(outer.getStore(), "outer");
  });
  assert.strictEqual(outer.getStore(), undefined);
  assert.strictEqual(inner.getStore(), undefined);
});

test("AsyncResource scopes ids and dispatches hooks", () => {
  const { events, hook } = trackResources();
  hook.enable();

  const trigger = executionAsyncId();
  const resource = new AsyncResource("TestResource");
  const asyncId = resource.asyncId();
  assert.strictEqual(resource.triggerAsyncId(), trigger);

  const scope = resource.runInAsyncScope(() => ({
    asyncId: executionAsyncId(),
    triggerAsyncId: triggerAsyncId(),
    resource: executionAsyncResource(),
  }));
  assert.deepStrictEqual(scope, {
    asyncId,
    triggerAsyncId: trigger,
    resource,
  });

  const bound = resource.bind(function (left, right) {
    assert.strictEqual(executionAsyncId(), asyncId);
    return left + right;
  });
  assert.strictEqual(bound.length, 2);
  assert.strictEqual(bound(1, 2), 3);

  const staticBound = AsyncResource.bind(function staticBoundTarget() {
    const own = executionAsyncResource();
    assert.notStrictEqual(own, resource);
    assert.strictEqual(executionAsyncId(), own.asyncId());
    return triggerAsyncId();
  }, "StaticBind");
  assert.strictEqual(staticBound(), trigger);

  resource.emitDestroy();
  hook.disable();

  assert.deepStrictEqual(events, [
    "init TestResource #1",
    "before #1",
    "after #1",
    "before #1",
    "after #1",
    "init StaticBind #2",
    "before #2",
    "after #2",
    "destroy #1",
  ]);
});

test("AsyncResource carries the AsyncLocalStorage store it was created in", async () => {
  const storage = new AsyncLocalStorage();
  const resource = storage.run("captured", () => new AsyncResource("Work"));
  const seen = [];
  await new Promise((resolve) => {
    setTimeout(() => {
      resource.runInAsyncScope(() => {
        seen.push(storage.getStore());
      });
      resolve();
    }, 1);
  });
  assert.deepStrictEqual(seen, ["captured"]);
  assert.strictEqual(storage.getStore(), undefined);
});

test("timer resources report their own execution context", async () => {
  const { events, hook, label } = trackResources();
  hook.enable();

  await new Promise((resolve) => {
    const timer = setTimeout(() => {
      assert.strictEqual(executionAsyncResource(), timer);
      assert.strictEqual(label(executionAsyncId()), "#1");
      events.push("timeout");
      resolve();
    }, 1);
  });

  hook.disable();
  assert.deepStrictEqual(events, [
    "init Timeout #1",
    "before #1",
    "timeout",
    "after #1",
    "destroy #1",
  ]);
});

test("nextTick resources run in their own scope", async () => {
  const { events, hook, label } = trackResources();
  hook.enable();

  await new Promise((resolve) => {
    process.nextTick(() => {
      assert.strictEqual(typeof executionAsyncResource(), "object");
      assert.strictEqual(label(executionAsyncId()), "#1");
      events.push("tick");
      resolve();
    });
  });

  hook.disable();
  assert.deepStrictEqual(events, [
    "init TickObject #1",
    "before #1",
    "tick",
    "after #1",
    "destroy #1",
  ]);
});

test("interval resources tick under one Timeout scope", async () => {
  const { events, hook, label } = trackResources();
  hook.enable();

  await new Promise((resolve) => {
    let ticks = 0;
    const interval = setInterval(() => {
      events.push(`tick ${label(executionAsyncId())}`);
      if (++ticks === 2) {
        clearInterval(interval);
        resolve();
      }
    }, 1);
  });

  hook.disable();
  assert.deepStrictEqual(events, [
    "init Timeout #1",
    "before #1",
    "tick #1",
    "after #1",
    "before #1",
    "tick #1",
    "destroy #1",
    "after #1",
  ]);
});

test("promise hooks init, before, after and promiseResolve", () =>
  new Promise((done) => {
    const parent = executionAsyncId();
    const names = new Map();
    const events = [];
    const name = (asyncId) => {
      if (asyncId === parent) return "current";
      if (!names.has(asyncId)) names.set(asyncId, `p${names.size + 1}`);
      return names.get(asyncId);
    };
    const hook = createHook({
      init(asyncId, type, triggerAsyncId) {
        if (type !== "PROMISE") return;
        events.push(`init ${name(asyncId)} <- ${name(triggerAsyncId)}`);
      },
      before(asyncId) {
        events.push(`before ${name(asyncId)}`);
      },
      after(asyncId) {
        events.push(`after ${name(asyncId)}`);
      },
      promiseResolve(asyncId) {
        events.push(`resolve ${name(asyncId)}`);
      },
    });
    hook.enable();

    Promise.resolve(1).then(() => {
      events.push("then");
    });

    setTimeout(() => {
      hook.disable();
      try {
        assert.deepStrictEqual(events, [
          "init p1 <- current",
          "resolve p1",
          "init p2 <- p1",
          "before p2",
          "then",
          "resolve p2",
          "after p2",
        ]);
        done();
      } catch (error) {
        done(error);
      }
    }, 2);
  }));

test("disabled hooks see no resources", () => {
  const types = [];
  const hook = createHook({
    init(asyncId, type) {
      types.push(type);
    },
  });
  hook.enable();
  hook.disable();
  new AsyncResource("AfterDisable");
  assert.deepStrictEqual(types, []);
});

test("enabling twice does not duplicate hook calls", () => {
  const types = [];
  const hook = createHook({
    init(asyncId, type) {
      if (type !== "PROMISE") types.push(type);
    },
  });
  hook.enable();
  hook.enable();
  new AsyncResource("Once");
  hook.disable();
  hook.disable();
  assert.deepStrictEqual(types, ["Once"]);
});

test("hook callbacks must be functions", () => {
  assert.throws(() => createHook({ init: 1 }), { code: "ERR_ASYNC_CALLBACK" });
  assert.throws(() => createHook({ before: "no" }), { code: "ERR_ASYNC_CALLBACK" });
  assert.throws(() => createHook({ promiseResolve: {} }), {
    code: "ERR_ASYNC_CALLBACK",
  });
});

test("createHook requires an object", () => {
  assert.throws(() => createHook(), { code: "ERR_INVALID_ARG_TYPE" });
  assert.throws(() => createHook(null), { code: "ERR_INVALID_ARG_TYPE" });
});

test("AsyncResource requires a type", () => {
  assert.throws(() => new AsyncResource(), { code: "ERR_INVALID_ARG_TYPE" });
});

const main = async () => {
  for (const { name, run } of tests) {
    await run();
    console.log(`ok ${name}`);
  }
  console.log(`\nok ${tests.length} async_hooks checks`);
};

main().then(
  () => {},
  (error) => {
    console.error(error);
    process.exitCode = 1;
  },
);
