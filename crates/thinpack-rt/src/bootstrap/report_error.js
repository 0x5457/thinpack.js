(() => {
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
