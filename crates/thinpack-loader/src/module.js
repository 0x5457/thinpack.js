(() => {
  const { ops } = Deno.core;
  const cache = Object.create(null);
  const split = (raw) => {
    const first = raw.indexOf("\0");
    const second = raw.indexOf("\0", first + 1);
    const third = raw.indexOf("\0", second + 1);
    return [
      raw.slice(0, first),
      raw.slice(first + 1, second),
      raw.slice(second + 1, third),
      raw.slice(third + 1),
    ];
  };

  // Module ids are native host paths when the app runs from disk, and POSIX
  // `/app/...` payload paths when it runs from a packed binary.
  const dirname = (filename) => {
    const slash = filename[0] === "/"
      ? filename.lastIndexOf("/")
      : filename.lastIndexOf("\\");
    return slash <= 0 ? "/" : filename.slice(0, slash);
  };

  const fail = (code, message) => {
    const error = new Error(message);
    error.code = code;
    return error;
  };

  const report = (stack) =>
    stack.length === 0 ? "" : "\nRequire stack:\n" + stack.map((entry) => "- " + entry).join("\n");

  const evaluate = (id, url, source, json, stack, isMain = false) => {
    const cached = cache[id];
    if (cached !== undefined) return cached.exports;

    const module = { id, filename: id, exports: {}, loaded: false };
    if (isMain) mainModule = module;
    cache[id] = module;

    if (json) {
      try {
        module.exports = JSON.parse(source);
      } catch (failure) {
        delete cache[id];
        throw failure;
      }
      module.loaded = true;
      return module.exports;
    }
    try {
      const [factory, error] = Deno.core.evalContext(
        "(function (exports, require, module, __filename, __dirname) {" + source + "\n})",
        url,
      );
      if (error) throw error.thrown;

      factory(
        module.exports,
        makeRequire(id, stack.concat(id)),
        module,
        id,
        dirname(id),
      );
    } catch (failure) {
      delete cache[id];
      throw failure;
    }
    module.loaded = true;
    return module.exports;
  };

  const load = (specifier, parent, stack, isMain = false) => {
    const [kind, id, url, body] = split(ops.op_require(String(specifier), parent));
    switch (kind) {
      case "b":
      case "f":
        return evaluate(id, url, body, false, stack, isMain);
      case "j":
        return evaluate(id, url, body, true, stack, isMain);
      case "m":
        return requireEsm(id, url);
      case "e":
        if (id === "MODULE_NOT_FOUND") {
          throw fail(id, "Cannot find module '" + specifier + "'" + report(stack));
        }
        throw fail(id, url);
      default:
        throw fail("ERR_INTERNAL_LOADER", "unexpected loader reply: " + kind);
    }
  };
  const requireEsm = (filename, url) => {
    const cached = cache[filename];
    if (cached !== undefined) return cached.exports;
    let namespace;
    try {
      namespace = ops.op_require_esm(url);
    } catch (error) {
      // The op tags top-level await with the standard `code\0detail` prefix
      // (the engine exposes no error code of its own for it).
      const code = String(error.message).split("\0")[0];
      if (code === "ERR_REQUIRE_ASYNC_MODULE") {
        throw fail(
          code,
          "require() cannot be used on an ES module with top-level await: " + filename,
        );
      }
      throw error;
    }
    const module = { id: filename, filename, exports: namespace, loaded: true };
    cache[filename] = module;
    return namespace;
  };
  const extensions = {
    ".js": () => {},
    ".json": () => {},
    ".node": () => {},
  };
  Object.defineProperty(globalThis, "__thinpack_module_extensions", {
    value: extensions,
  });

  let mainModule = null;

  const makeRequire = (filename, stack) => {
    const require = function require(specifier) {
      return load(specifier, filename, stack);
    };
    require.resolve = (specifier) => ops.op_require_resolve(specifier, filename);
    require.extensions = extensions;
    require.cache = cache;
    require.main = mainModule ?? undefined;
    return require;
  };

  const runMain = () => {
    const [kind, id, url, source] = split(ops.op_entry());
    if (kind === "e") {
      throw fail(id, url);
    }
    if (kind === "v") {
      evaluate(id, url, source, false, []);
      const require = makeRequire(id, [id]);
      Object.defineProperty(globalThis, "require", {
        value: require,
        configurable: true,
        writable: true,
      });
      return;
    }
    load(id, "", [], true);
  };

  Object.defineProperty(globalThis, "__thinpack_run_main", { value: runMain });
  Object.defineProperty(globalThis, "__thinpack_builtin_modules", {
    configurable: true,
    get() {
      const value = ops.op_builtin_modules().split("\0").filter(Boolean);
      Object.defineProperty(globalThis, "__thinpack_builtin_modules", { value });
      return value;
    },
  });
  Object.defineProperty(globalThis, "__thinpack_require_cache", {
    value: cache,
    configurable: true,
  });
  Object.defineProperty(globalThis, "__thinpack_create_require__", {
    value: (filename) => makeRequire(String(filename), []),
  });
  Object.defineProperty(globalThis, "__thinpack_require__", {
    value: (specifier) => load(specifier, "", []),
  });
  const finishHooks = [];
  Object.defineProperty(globalThis, "__thinpack_on_finish", {
    value: (hook) => finishHooks.push(hook),
  });
  Object.defineProperty(globalThis, "__thinpack_finish", {
    value: () => {
      for (const hook of finishHooks) hook();
    },
  });
  const importConditions = Object.freeze(["node", "import"]);
  const esmHookSets = [];
  const esmHookLoads = new Set();

  const hookFailure = (message, code) => {
    const error = new TypeError(message);
    if (code !== undefined) error.code = code;
    return error;
  };
  const sleep = (milliseconds) => {
    if (milliseconds > 0) {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
    }
  };

  const runHook = (name, invoke) => {
    const previousSetTimeout = globalThis.setTimeout;
    const previousClearTimeout = globalThis.clearTimeout;
    const previousSetImmediate = globalThis.setImmediate;
    const previousClearImmediate = globalThis.clearImmediate;
    const timers = [];
    let nextTimer = 1;
    const setHookTimeout = (callback, delay = 0, ...args) => {
      if (typeof callback !== "function") {
        return previousSetTimeout(callback, delay, ...args);
      }
      const timer = {
        id: nextTimer++,
        callback,
        args,
        due: Date.now() + Math.max(0, Number(delay) || 0),
      };
      timers.push(timer);
      return timer;
    };
    const clearHookTimeout = (timer) => {
      const index = timers.indexOf(timer);
      if (index !== -1) timers.splice(index, 1);
    };
    globalThis.setTimeout = setHookTimeout;
    globalThis.clearTimeout = clearHookTimeout;
    globalThis.setImmediate = (callback, ...args) => setHookTimeout(callback, 0, ...args);
    globalThis.clearImmediate = clearHookTimeout;
    const restore = () => {
      globalThis.setTimeout = previousSetTimeout;
      globalThis.clearTimeout = previousClearTimeout;
      globalThis.setImmediate = previousSetImmediate;
      globalThis.clearImmediate = previousClearImmediate;
    };
    let result;
    try {
      result = invoke();
    } catch (error) {
      restore();
      throw error;
    }
    if (result === null || typeof result !== "object" || typeof result.then !== "function") {
      restore();
      return result;
    }
    let settled = 0;
    let value;
    let failure;
    result.then(
      (resolved) => { settled = 1; value = resolved; },
      (rejected) => { settled = 2; failure = rejected; },
    );
    try {
      while (settled === 0) {
        Deno.core.runMicrotasks();
        if (settled !== 0) break;
        timers.sort((left, right) => left.due - right.due);
        const timer = timers.shift();
        if (timer === undefined) break;
        sleep(timer.due - Date.now());
        try {
          timer.callback(...timer.args);
        } finally {
          Deno.core.runMicrotasks();
        }
      }
    } finally {
      restore();
    }
    if (settled === 0) {
      const error = new Error(
        "the " + name + " hook is still pending after the timer checkpoint; " +
          "hooks that wait for I/O need a loader thread",
      );
      error.code = "ERR_NOT_IMPLEMENTED";
      throw error;
    }
    if (settled === 2) throw failure;
    return value;
  };

  const parseAttributes = (raw) => {
    const attributes = {};
    if (raw === "") return attributes;
    const parts = String(raw).split("\0");
    for (let index = 0; index + 1 < parts.length; index += 2) {
      attributes[parts[index]] = parts[index + 1];
    }
    return attributes;
  };

  const defaultResolve = (specifier, parentURL) =>
    ops.op_esm_resolve_default(
      String(specifier),
      parentURL === undefined || parentURL === null ? "" : String(parentURL),
    );

  const defaultLoad = (url) => {
    const raw = ops.op_esm_load_default(String(url));
    const separator = raw.indexOf("\0");
    return { format: raw.slice(0, separator), source: raw.slice(separator + 1) };
  };

  const hasLoadHook = () => esmHookSets.some((hooks) => hooks.load !== undefined);
  const runResolveHooks = (index, specifier, context) => {
    if (index < 0) return { url: defaultResolve(specifier, context.parentURL) };
    const hooks = esmHookSets[index];
    if (hooks.resolve === undefined) {
      return runResolveHooks(index - 1, specifier, context);
    }
    let nextResult;
    let nextCalled = false;
    const nextResolve = (nextSpecifier, nextContext) => {
      nextCalled = true;
      const parentURL =
        nextContext !== undefined && nextContext !== null && nextContext.parentURL !== undefined
          ? nextContext.parentURL
          : context.parentURL;
      nextResult = runResolveHooks(index - 1, String(nextSpecifier), { ...context, parentURL });
      return nextResult;
    };
    let result = runHook(
      "resolve",
      () => hooks.resolve(specifier, { ...context, data: hooks.data }, nextResolve),
    );
    if (result === undefined || result === null) {
      if (!nextCalled) {
        throw hookFailure("the resolve hook must return a value or call nextResolve");
      }
      result = nextResult;
    }
    if (result === null || typeof result !== "object") {
      throw hookFailure("the resolve hook must return an object with a url");
    }
    return result;
  };

  const runLoadHooks = (index, url, format, attributes) => {
    if (index < 0) return defaultLoad(url);
    const hooks = esmHookSets[index];
    if (hooks.load === undefined) return runLoadHooks(index - 1, url, format, attributes);
    let nextResult;
    const nextLoad = (nextUrl, nextContext) => {
      const nextFormat =
        nextContext !== undefined && nextContext !== null && nextContext.format !== undefined
          ? nextContext.format
          : format;
      nextResult = runLoadHooks(index - 1, String(nextUrl), nextFormat, attributes);
      return nextResult;
    };
    const result = runHook(
      "load",
      () => hooks.load(
        url,
        { format, importAttributes: attributes, conditions: importConditions, data: hooks.data },
        nextLoad,
      ),
    );
    if (result === null || typeof result !== "object") {
      throw hookFailure("the load hook must return an object");
    }
    if (result.source === undefined) {
      if (nextResult === undefined) nextResult = runLoadHooks(index - 1, url, format, attributes);
      return {
        format: typeof result.format === "string" ? result.format : nextResult.format,
        source: nextResult.source,
      };
    }
    if (typeof result.source !== "string") {
      throw hookFailure("the load hook source must be a string");
    }
    if (typeof result.format !== "string") {
      throw hookFailure("the load hook must provide a format together with the source");
    }
    return { format: result.format, source: result.source };
  };

  const resolveBridge = (specifier, referrer, rawAttributes) => {
    if (esmHookSets.length === 0) return "";
    const parentURL = referrer === "" || referrer === "." ? undefined : String(referrer);
    const attributes = parseAttributes(rawAttributes);
    const result = runResolveHooks(esmHookSets.length - 1, String(specifier), {
      conditions: importConditions,
      parentURL,
      importAttributes: attributes,
    });
    if (typeof result.url !== "string" || result.url === "") {
      throw hookFailure("the resolve hook must return an object with a url");
    }
    if (!hasLoadHook() || esmHookLoads.has(result.url)) return result.url;
    esmHookLoads.add(result.url);
    const loaded = runLoadHooks(esmHookSets.length - 1, result.url, result.format, attributes);
    return result.url + "\0" + loaded.format + "\0" + loaded.source;
  };

  const registerHooks = (hooks) => {
    if (hooks === null || typeof hooks !== "object") {
      throw hookFailure('The "hooks" argument must be an object', "ERR_INVALID_ARG_TYPE");
    }
    if (hooks.resolve !== undefined && typeof hooks.resolve !== "function") {
      throw hookFailure('The "hooks.resolve" property must be of type function', "ERR_INVALID_ARG_TYPE");
    }
    if (hooks.load !== undefined && typeof hooks.load !== "function") {
      throw hookFailure('The "hooks.load" property must be of type function', "ERR_INVALID_ARG_TYPE");
    }
    esmHookSets.push({ resolve: hooks.resolve, load: hooks.load, data: hooks.data });
  };
  const register = (specifier, parentURL, options) => {
    let parent;
    let opts;
    if (typeof parentURL === "string" || parentURL instanceof URL) {
      parent = String(parentURL);
      opts = options;
    } else if (parentURL !== undefined) {
      opts = parentURL;
    }
    if (opts !== undefined && opts !== null && opts.parentURL !== undefined) {
      parent = String(opts.parentURL);
    }
    const url = defaultResolve(String(specifier), parent);
    const namespace = ops.op_require_esm(url);
    const exported = namespace ?? {};
    const fallback = exported.default ?? {};
    registerHooks({
      resolve: exported.resolve ?? fallback.resolve,
      load: exported.load ?? fallback.load,
      data: opts === undefined || opts === null ? undefined : opts.data,
    });
  };

  Object.defineProperty(globalThis, "__thinpack_esm_hooks__", {
    value: { register, registerHooks },
  });
  Object.defineProperty(globalThis, "__thinpack_esm_resolve__", { value: resolveBridge });
  Object.defineProperty(globalThis, "__thinpack_require_source__", {
    value: (id, url, source) => {
      const cached = cache[id];
      if (cached !== undefined) return cached.exports;
      return evaluate(id, url, source, false, [id]);
    },
  });
})();
