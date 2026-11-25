// The other half of `module.register`: this file runs in the loader isolate, where a hook
// may await the event loop like Node's loader thread. `thinpack_loader::hook` loads the
// hooks module, calls `add`, and turns each request into one of these calls.
(() => {
  const { ops } = Deno.core;
  const layers = [];
  const defaultConditions = Object.freeze(["node", "import"]);

  const hookFailure = (message, code) => {
    const error = new TypeError(message);
    if (code !== undefined) error.code = code;
    return error;
  };

  const text = (value) => {
    try {
      return value === undefined || value === null ? "" : String(value);
    } catch {
      return "";
    }
  };

  const describe = (failure) => {
    let object;
    try {
      object =
        failure !== null && (typeof failure === "object" || typeof failure === "function")
          ? failure
          : undefined;
    } catch {
      object = undefined;
    }
    return [
      "err",
      object !== undefined && object.name !== undefined ? text(object.name) : "Error",
      object !== undefined && object.message !== undefined ? text(object.message) : text(failure),
      object !== undefined && object.code !== undefined ? text(object.code) : "",
      object !== undefined && object.stack !== undefined ? text(object.stack) : "",
    ].join("\0");
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

  const splitConditions = (raw) =>
    raw === "" ? defaultConditions : Object.freeze(String(raw).split("\0"));

  const defaultResolve = (specifier, parentURL) =>
    ops.op_esm_resolve_default(String(specifier), parentURL ?? "");

  const defaultLoad = (url) => {
    const raw = ops.op_esm_load_default(String(url));
    const separator = raw.indexOf("\0");
    return { format: raw.slice(0, separator), source: raw.slice(separator + 1) };
  };

  const runResolve = async (index, specifier, context) => {
    if (index < 0) return { url: defaultResolve(specifier, context.parentURL) };
    const layer = layers[index];
    if (layer.resolve === undefined) return runResolve(index - 1, specifier, context);
    let nextResult;
    let nextCalled = false;
    const nextResolve = async (nextSpecifier, nextContext) => {
      nextCalled = true;
      const parentURL =
        nextContext !== undefined && nextContext !== null && nextContext.parentURL !== undefined
          ? nextContext.parentURL
          : context.parentURL;
      nextResult = await runResolve(index - 1, String(nextSpecifier), { ...context, parentURL });
      return nextResult;
    };
    let result = await layer.resolve(
      specifier,
      { ...context, data: layer.data },
      nextResolve,
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

  const runLoad = async (index, url, format, context) => {
    if (index < 0) return defaultLoad(url);
    const layer = layers[index];
    if (layer.load === undefined) return runLoad(index - 1, url, format, context);
    let nextResult;
    const nextLoad = async (nextUrl, nextContext) => {
      const nextFormat =
        nextContext !== undefined && nextContext !== null && nextContext.format !== undefined
          ? nextContext.format
          : format;
      nextResult = await runLoad(index - 1, String(nextUrl), nextFormat, context);
      return nextResult;
    };
    const result = await layer.load(
      url,
      { ...context, format, data: layer.data },
      nextLoad,
    );
    if (result === null || typeof result !== "object") {
      throw hookFailure("the load hook must return an object");
    }
    if (result.source === undefined) {
      if (nextResult === undefined) nextResult = await runLoad(index - 1, url, format, context);
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

  const add = async (namespace, bytes) => {
    try {
      const data = Deno.core.deserialize(bytes);
      const exported = namespace ?? {};
      const fallback = exported.default ?? {};
      const layer = {
        resolve: exported.resolve ?? fallback.resolve,
        load: exported.load ?? fallback.load,
        initialize: exported.initialize ?? fallback.initialize,
        data,
      };
      for (const name of ["resolve", "load", "initialize"]) {
        if (layer[name] !== undefined && typeof layer[name] !== "function") {
          throw hookFailure(`The "${name}" export must be of type function`);
        }
      }
      if (layer.initialize !== undefined) await layer.initialize(data);
      layers.push(layer);
      return "ok";
    } catch (failure) {
      return describe(failure);
    }
  };

  const resolve = async (specifier, parentURL, rawAttributes, rawConditions) => {
    try {
      const context = {
        conditions: splitConditions(rawConditions),
        parentURL: parentURL === "" ? undefined : parentURL,
        importAttributes: parseAttributes(rawAttributes),
      };
      const result = await runResolve(layers.length - 1, String(specifier), context);
      if (typeof result.url !== "string" || result.url === "") {
        throw hookFailure("the resolve hook must return an object with a url");
      }
      return "ok\0" + result.url;
    } catch (failure) {
      return describe(failure);
    }
  };

  const load = async (url, format, rawAttributes, rawConditions) => {
    try {
      const context = {
        format: format === "" ? undefined : format,
        conditions: splitConditions(rawConditions),
        importAttributes: parseAttributes(rawAttributes),
      };
      const result = await runLoad(layers.length - 1, String(url), context.format, context);
      return "ok\0" + result.format + "\0" + result.source;
    } catch (failure) {
      return describe(failure);
    }
  };

  Object.defineProperty(globalThis, "__thinpack_hook_runtime__", {
    value: { add, resolve, load },
  });
})();
