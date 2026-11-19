(() => {
  const names = ["fetch", "Headers", "Request", "Response", "FormData"];

  const unavailable = (error) => {
    const wrapped = new Error(
      "fetch and its types are not available in this build; re-pack with `--with` " +
        "for each of http, net, tls, stream, zlib, crypto, buffer, assert, async_hooks, " +
        "diagnostics_channel, perf_hooks, querystring, url, util and worker_threads",
    );
    wrapped.code = error.code ?? "ERR_UNKNOWN_BUILTIN_MODULE";
    wrapped.cause = error;
    return wrapped;
  };

  const load = () => {
    try {
      return globalThis.__thinpack_require__("node:thinpack/internal/undici");
    } catch (error) {
      throw unavailable(error);
    }
  };

  for (const name of names) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      enumerable: false,
      get() {
        const exported = load()[name];
        Object.defineProperty(globalThis, name, {
          value: exported, writable: true, configurable: true, enumerable: false,
        });
        return exported;
      },
      set(value) {
        Object.defineProperty(globalThis, name, {
          value, writable: true, configurable: true, enumerable: false,
        });
      },
    });
  }

  // Node dispatches a cancelable `ErrorEvent` on `globalThis` first and reports the
  // exception unless a listener handles it; there is no `ErrorEvent` global here, so
  // throwing from a microtask lands in the uncaught-exception path instead.
  Object.defineProperty(globalThis, "reportError", {
    value: (error) => {
      queueMicrotask(() => {
        throw error;
      });
    },
    writable: true,
    configurable: true,
    enumerable: false,
  });
})();
