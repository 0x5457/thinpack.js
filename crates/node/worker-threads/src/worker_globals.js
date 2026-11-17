(() => {
  for (const name of ["MessageChannel", "MessagePort", "Worker"]) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      enumerable: false,
      get() {
        return globalThis.__thinpack_require__("node:worker_threads")[name];
      },
    });
  }
})();
