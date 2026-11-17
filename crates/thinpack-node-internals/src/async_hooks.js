(() => {
  const core = Deno.core;
  const ROOT_ASYNC_ID = 1;
  const ROOT_TRIGGER_ASYNC_ID = 0;
  const ROOT_ASYNC_RESOURCE = {};

  const HOOK_NAMES = ["init", "before", "after", "destroy", "promiseResolve"];

  const initHooks = [];
  const beforeHooks = [];
  const afterHooks = [];
  const destroyHooks = [];
  const promiseResolveHooks = [];

  const hookLists = {
    init: initHooks,
    before: beforeHooks,
    after: afterHooks,
    destroy: destroyHooks,
    promiseResolve: promiseResolveHooks,
  };

  let lastAsyncId = ROOT_ASYNC_ID;
  let lastStorageId = 0;
  let executionId = ROOT_ASYNC_ID;
  let executionTriggerId = ROOT_TRIGGER_ASYNC_ID;
  let executionResource = ROOT_ASYNC_RESOURCE;
  const executionStack = [];

  const promiseScopes = new WeakMap();
  let promiseHooksInstalled = false;
  const promiseFinalizer = typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry((asyncId) => {
      for (let i = 0; i < destroyHooks.length; i++) {
        destroyHooks[i].callback(asyncId);
      }
    })
    : undefined;

  const asyncCallbackError = (name) => {
    const error = new TypeError(
      'The "hook.' + name + '" argument must be of type function',
    );
    error.code = "ERR_ASYNC_CALLBACK";
    return error;
  };

  function enabledHooksExist() {
    return initHooks.length !== 0 || beforeHooks.length !== 0 ||
      afterHooks.length !== 0 || destroyHooks.length !== 0 ||
      promiseResolveHooks.length !== 0;
  }

  function newAsyncId() {
    return ++lastAsyncId;
  }

  function getDefaultTriggerAsyncId() {
    return executionId;
  }

  function executionAsyncId() {
    return executionId;
  }

  function triggerAsyncId() {
    return executionTriggerId;
  }

  function executionAsyncResource() {
    return executionResource;
  }

  function createHook(functions) {
    if (functions === null || typeof functions !== "object") {
      const error = new TypeError('The "hooks" argument must be of type object');
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    const entries = [];
    for (let i = 0; i < HOOK_NAMES.length; i++) {
      const name = HOOK_NAMES[i];
      const callback = functions[name];
      if (callback === undefined) continue;
      if (typeof callback !== "function") throw asyncCallbackError(name);
      entries.push({ name, callback });
    }

    let enabled = false;
    const hook = {
      enable() {
        if (enabled) return hook;
        enabled = true;
        for (const entry of entries) hookLists[entry.name].push(entry);
        ensurePromiseHooks();
        return hook;
      },
      disable() {
        if (!enabled) return hook;
        enabled = false;
        for (const entry of entries) {
          const list = hookLists[entry.name];
          const index = list.indexOf(entry);
          if (index !== -1) list.splice(index, 1);
        }
        return hook;
      },
    };
    return hook;
  }

  function ensurePromiseHooks() {
    if (promiseHooksInstalled) return;
    promiseHooksInstalled = true;
    core.setPromiseHooks(promiseInit, promiseBefore, promiseAfter, promiseResolve);
  }
  function promiseInit(promise, parent) {
    const parentScope = promiseScopes.get(parent);
    const scope = {
      type: "PROMISE",
      asyncId: newAsyncId(),
      triggerAsyncId: parentScope === undefined ? executionId : parentScope.asyncId,
      resource: promise,
    };
    promiseScopes.set(promise, scope);
    promiseFinalizer?.register(promise, scope.asyncId);
    if (initHooks.length !== 0) {
      for (let i = 0; i < initHooks.length; i++) {
        initHooks[i].callback(scope.asyncId, "PROMISE", scope.triggerAsyncId, promise);
      }
    }
  }

  function promiseBefore(promise) {
    executionStack.push(executionId, executionTriggerId, executionResource);
    const scope = promiseScopes.get(promise);
    if (scope === undefined) return;
    executionId = scope.asyncId;
    executionTriggerId = scope.triggerAsyncId;
    executionResource = promise;
    for (let i = 0; i < beforeHooks.length; i++) {
      beforeHooks[i].callback(scope.asyncId);
    }
  }

  function promiseAfter(promise) {
    const scope = promiseScopes.get(promise);
    if (scope !== undefined) {
      for (let i = 0; i < afterHooks.length; i++) {
        afterHooks[i].callback(scope.asyncId);
      }
    }
    executionResource = executionStack.pop();
    executionTriggerId = executionStack.pop();
    executionId = executionStack.pop();
  }

  function promiseResolve(promise) {
    if (promiseResolveHooks.length === 0) return;
    const scope = promiseScopes.get(promise);
    if (scope === undefined) return;
    for (let i = 0; i < promiseResolveHooks.length; i++) {
      promiseResolveHooks[i].callback(scope.asyncId);
    }
  }
  function createResourceScope(type) {
    return {
      type,
      asyncId: newAsyncId(),
      triggerAsyncId: executionId,
      context: core.getAsyncContext(),
      resource: undefined,
      destroyed: false,
    };
  }

  function attachResource(scope, resource) {
    scope.resource = resource;
    for (let i = 0; i < initHooks.length; i++) {
      initHooks[i].callback(scope.asyncId, scope.type, scope.triggerAsyncId, resource);
    }
  }

  function runInScope(scope, callback, thisArg, args) {
    const previousContext = core.getAsyncContext();
    if (previousContext !== scope.context) core.setAsyncContext(scope.context);
    executionStack.push(executionId, executionTriggerId, executionResource);
    executionId = scope.asyncId;
    executionTriggerId = scope.triggerAsyncId;
    executionResource = scope.resource;
    for (let i = 0; i < beforeHooks.length; i++) {
      beforeHooks[i].callback(scope.asyncId);
    }
    try {
      return Reflect.apply(callback, thisArg, args);
    } finally {
      for (let i = 0; i < afterHooks.length; i++) {
        afterHooks[i].callback(scope.asyncId);
      }
      executionResource = executionStack.pop();
      executionTriggerId = executionStack.pop();
      executionId = executionStack.pop();
      if (previousContext !== scope.context) core.setAsyncContext(previousContext);
    }
  }

  function destroyScope(scope) {
    if (scope.destroyed) return;
    scope.destroyed = true;
    for (let i = 0; i < destroyHooks.length; i++) {
      destroyHooks[i].callback(scope.asyncId);
    }
  }

  class AsyncResource {
    #scope;

    constructor(type, options = {}) {
      if (typeof type !== "string") {
        const error = new TypeError(
          'The "type" argument must be of type string',
        );
        error.code = "ERR_INVALID_ARG_TYPE";
        throw error;
      }
      const scope = createResourceScope(type);
      if (options.triggerAsyncId !== undefined) {
        scope.triggerAsyncId = options.triggerAsyncId;
      }
      this.#scope = scope;
      attachResource(scope, this);
    }

    asyncId() {
      return this.#scope.asyncId;
    }

    triggerAsyncId() {
      return this.#scope.triggerAsyncId;
    }

    runInAsyncScope(callback, thisArg, ...args) {
      return runInScope(this.#scope, callback, thisArg, args);
    }

    emitDestroy() {
      destroyScope(this.#scope);
      return this;
    }

    bind(callback, thisArg) {
      const resource = this;
      const bound = thisArg === undefined
        ? function (...args) {
          return resource.runInAsyncScope(callback, this, ...args);
        }
        : function (...args) {
          return resource.runInAsyncScope(callback, thisArg, ...args);
        };
      Object.defineProperty(bound, "length", {
        value: callback.length,
        configurable: true,
      });
      return bound;
    }

    static bind(callback, type, thisArg) {
      const resourceType = type || callback.name || "bound-anonymous-fn";
      return new AsyncResource(resourceType).bind(callback, thisArg);
    }
  }
  const enterStore = (context, id, store) => ({
    __proto__: null,
    ...context,
    [id]: store,
  });

  class AsyncLocalStorage {
    #id = lastStorageId++;
    #defaultValue;

    constructor(options = {}) {
      this.#defaultValue = options.defaultValue;
    }

    run(store, callback, ...args) {
      if (Object.is(this.getStore(), store)) {
        return Reflect.apply(callback, undefined, args);
      }
      const previousContext = core.getAsyncContext();
      core.setAsyncContext(enterStore(previousContext, this.#id, store));
      try {
        return Reflect.apply(callback, undefined, args);
      } finally {
        core.setAsyncContext(previousContext);
      }
    }

    exit(callback, ...args) {
      return this.run(undefined, callback, ...args);
    }

    enterWith(store) {
      core.setAsyncContext(enterStore(core.getAsyncContext(), this.#id, store));
    }

    getStore() {
      const context = core.getAsyncContext();
      if (context === undefined || context === null || !(this.#id in context)) {
        return this.#defaultValue;
      }
      return context[this.#id];
    }

    disable() {
      const context = core.getAsyncContext();
      if (context === undefined || context === null || !(this.#id in context)) {
        return;
      }
      const mapping = { __proto__: null, ...context };
      delete mapping[this.#id];
      core.setAsyncContext(mapping);
    }
  }

  module.exports = {
    AsyncLocalStorage,
    AsyncResource,
    attachResource,
    createHook,
    createResourceScope,
    destroyScope,
    enabledHooksExist,
    executionAsyncId,
    executionAsyncResource,
    getDefaultTriggerAsyncId,
    newAsyncId,
    runInScope,
    triggerAsyncId,
  };
})();
