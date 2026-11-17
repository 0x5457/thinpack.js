(() => {
  const core = Deno.core;
  let asyncHooks;
  const hooks = () =>
    asyncHooks ?? (asyncHooks = globalThis.__thinpack_require__(
      "node:thinpack/internal/async_hooks",
    ));

  const kScope = Symbol("thinpack.timerScope");

  const invalidCallback = () => {
    const error = new TypeError('The "callback" argument must be of type function');
    error.code = "ERR_INVALID_ARG_TYPE";
    return error;
  };
  const nodeTimer = (timer) => {
    timer.ref = () => {
      core.refTimer(timer);
      return timer;
    };
    timer.unref = () => {
      core.unrefTimer(timer);
      return timer;
    };
    timer.refresh = () => {
      core.refreshTimer(timer);
      return timer;
    };
    timer[Symbol.toPrimitive] = () => timer._timerId;
    return timer;
  };

  const create = (repeat) => (callback, delay, ...args) => {
    if (typeof callback !== "function") {
      throw invalidCallback();
    }
    const asyncHooks = hooks();
    const scope = asyncHooks.createResourceScope("Timeout");
    const timer = core.createTimer(
      (...timerArgs) => {
        try {
          asyncHooks.runInScope(scope, callback, undefined, timerArgs);
        } finally {
          if (!repeat) asyncHooks.destroyScope(scope);
        }
      },
      delay === undefined ? 1 : delay,
      args,
      repeat,
      true,
      false,
    );
    timer[kScope] = scope;
    nodeTimer(timer);
    asyncHooks.attachResource(scope, timer);
    return timer;
  };

  const setTimeout = create(false);
  const setInterval = create(true);
  const setImmediate = (callback, ...args) => setTimeout(callback, 0, ...args);

  const clearTimer = (timer) => {
    if (timer !== null && typeof timer === "object") {
      const scope = timer[kScope];
      if (scope !== undefined) hooks().destroyScope(scope);
      core.cancelTimer(timer);
    }
  };

  const timers = {
    setTimeout,
    setInterval,
    setImmediate,
    clearTimeout: clearTimer,
    clearInterval: clearTimer,
    clearImmediate: clearTimer,
  };

  Object.assign(globalThis, timers);
  globalThis.__thinpack_timers = timers;
})();
