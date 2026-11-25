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

  // A CommonJS file that contains ESM syntax fails in the compiler, and the
  // engine's message alone names neither the file nor the format it was read
  // as. Keep the engine message and add both, plus the two ways to ask for ESM.
  const compileFailure = (failure, id) => {
    if (!(failure instanceof SyntaxError)) return failure;
    const frames = String(failure.stack).split("\n").slice(1);
    failure.message = `${failure.message} (${id}, loaded as CommonJS)`;
    failure.stack = [
      `${failure.name}: ${failure.message}`,
      `${id} is loaded as CommonJS; if it is an ES module, rename it to .mjs ` +
        `or add "type": "module" to the nearest package.json.`,
      ...frames,
    ].join("\n");
    return failure;
  };

  // `require.extensions`/`Module._extensions`. The defaults are sentinels: a
  // registered handler replaces the compile/parse path for that extension.
  const defaultJs = () => {};
  const defaultJson = () => {};
  const defaultNode = () => {};
  const extensions = {
    ".js": defaultJs,
    ".json": defaultJson,
    ".node": defaultNode,
  };
  Object.defineProperty(globalThis, "__thinpack_module_extensions", {
    value: extensions,
  });

  const extensionHandler = (filename) => {
    const base = filename.slice(filename.lastIndexOf("/") + 1);
    for (let index = base.indexOf("."); index !== -1; index = base.indexOf(".", index + 1)) {
      const handler = extensions[base.slice(index)];
      if (handler !== undefined) return handler;
    }
    return defaultJs;
  };

  const evaluate = (id, url, source, json, stack, isMain = false, wrapped = false, parent = null, file = false) => {
    const cached = cache[id];
    if (cached !== undefined) return cached.exports;

    const module = {
      id,
      filename: id,
      exports: {},
      loaded: false,
      // Node keeps the first requiring module; the entry module has no parent.
      parent: parent === null ? null : cache[parent] ?? null,
    };
    if (isMain) mainModule = module;
    cache[id] = module;

    const handler = file ? extensionHandler(id) : defaultJs;
    if (handler !== defaultJs && handler !== defaultJson && handler !== defaultNode) {
      try {
        handler(module, id);
      } catch (failure) {
        delete cache[id];
        throw failure;
      }
      module.loaded = true;
      return module.exports;
    }

    if (json || handler === defaultJson) {
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
      const code = wrapped
        ? source
        : "(function (exports, require, module, __filename, __dirname) {" + source + "\n})";
      const [factory, error] = Deno.core.evalContext(code, url);
      if (error) {
        throw error.isCompileError ? compileFailure(error.thrown, id) : error.thrown;
      }

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
        return evaluate(id, url, body, false, stack, isMain);
      case "f":
        return evaluate(id, url, body, false, stack, isMain, true, parent, true);
      case "j":
        return evaluate(id, url, body, true, stack, isMain, false, parent, true);
      case "m":
        return requireEsm(id, url, parent);
      case "e":
        if (id === "MODULE_NOT_FOUND") {
          throw fail(id, "Cannot find module '" + specifier + "'" + report(stack));
        }
        throw fail(id, url);
      default:
        throw fail("ERR_INTERNAL_LOADER", "unexpected loader reply: " + kind);
    }
  };
  const requireEsm = (filename, url, parent) => {
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
    const module = {
      id: filename,
      filename,
      exports: namespace,
      loaded: true,
      parent: parent ? cache[parent] ?? null : null,
    };
    cache[filename] = module;
    return namespace;
  };
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
  // The ESM facade needs the builtin's real named exports, the way Node reads
  // `Object.keys(module.exports)`; a static scan cannot follow the alias assignments
  // builtins use (`module.exports = Module; Module.createRequire = ...`). Getters are
  // left out because reading one can initialize an optional capability the app did not
  // select (`events` without `async_hooks`). A builtin names the getter-backed exports
  // Node has with `__thinpack_declare_builtin_exports__`.
  const declaredBuiltinExports = Object.create(null);
  Object.defineProperty(globalThis, "__thinpack_declare_builtin_exports__", {
    value: (specifier, names) => {
      declaredBuiltinExports[specifier] = names;
    },
  });
  Object.defineProperty(globalThis, "__thinpack_builtin_exports__", {
    value: (specifier) => {
      const exports = load(specifier, "", []);
      const names = Object.keys(exports).filter(
        (name) => Object.getOwnPropertyDescriptor(exports, name)?.get === undefined,
      );
      return names.concat(declaredBuiltinExports[specifier] ?? []).join("\0");
    },
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
  const asyncActive = () => ops.op_esm_async_active();
  const errorClasses = {
    Error,
    EvalError,
    RangeError,
    ReferenceError,
    SyntaxError,
    TypeError,
    URIError,
  };

  // `module.register` failures cross the isolate bridge as text; rebuild the thrown
  // error with the class, message, code and stack the hook isolate recorded.
  const throwHookError = (reply) => {
    let rest = reply.startsWith("err\0") ? reply.slice(4) : reply;
    const fields = [];
    for (let index = 0; index < 3; index++) {
      const end = rest.indexOf("\0");
      if (end === -1) {
        fields.push(rest);
        rest = "";
      } else {
        fields.push(rest.slice(0, end));
        rest = rest.slice(end + 1);
      }
    }
    fields.push(rest);
    const [className, message, code, stack] = fields;
    const ErrorClass = errorClasses[className];
    const error = ErrorClass === undefined ? new Error(message) : new ErrorClass(message);
    if (error.name !== className) {
      Object.defineProperty(error, "name", { value: className, configurable: true });
    }
    if (code !== "") error.code = code;
    if (stack !== "") error.stack = stack;
    throw error;
  };

  const okReply = (reply) => {
    if (reply.startsWith("err\0")) throwHookError(reply);
    return reply.startsWith("ok\0") ? reply.slice(3) : reply;
  };

  const encodeAttributes = (attributes) => {
    const parts = [];
    for (const key of Object.keys(attributes)) parts.push(key, attributes[key]);
    return parts.join("\0");
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

  const formatOf = (url) => {
    const format = ops.op_esm_format(String(url));
    return format === "" ? undefined : format;
  };

  const defaultLoad = (url) => {
    const raw = ops.op_esm_load_default(String(url));
    const separator = raw.indexOf("\0");
    return { format: raw.slice(0, separator), source: raw.slice(separator + 1) };
  };

  const hasLoadHook = () => esmHookSets.some((hooks) => hooks.load !== undefined);
  const runResolveHooks = (index, specifier, context) => {
    if (index < 0) {
      if (asyncActive()) {
        return {
          url: okReply(ops.op_esm_async_resolve(
            String(specifier),
            context.parentURL ?? "",
            encodeAttributes(context.importAttributes),
            context.conditions.join("\0"),
          )),
        };
      }
      return { url: defaultResolve(specifier, context.parentURL) };
    }
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
    let result = hooks.resolve(specifier, { ...context, data: hooks.data }, nextResolve);
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
    if (index < 0) {
      if (asyncActive()) {
        const raw = okReply(ops.op_esm_async_load(
          String(url),
          format ?? "",
          encodeAttributes(attributes),
          importConditions.join("\0"),
        ));
        const separator = raw.indexOf("\0");
        return { format: raw.slice(0, separator), source: raw.slice(separator + 1) };
      }
      return defaultLoad(url);
    }
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
    const result = hooks.load(
      url,
      { format, importAttributes: attributes, conditions: importConditions, data: hooks.data },
      nextLoad,
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
    const parentURL = referrer === "" || referrer === "." ? undefined : String(referrer);
    if (esmHookSets.length === 0) {
      if (!asyncActive()) return "";
      return okReply(ops.op_esm_async_resolve(
        String(specifier),
        parentURL ?? "",
        rawAttributes,
        importConditions.join("\0"),
      ));
    }
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
    const format = result.format ?? formatOf(result.url);
    const loaded = runLoadHooks(esmHookSets.length - 1, result.url, format, attributes);
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
    const data = Deno.core.serialize(
      opts === undefined || opts === null ? undefined : opts.data,
      { transferredArrayBuffers: opts?.transferList },
    );
    const reply = ops.op_esm_register(url, data);
    if (reply !== "ok") throwHookError(reply);
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
