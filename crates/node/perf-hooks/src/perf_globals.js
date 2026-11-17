(() => {
  Object.defineProperty(globalThis, "performance", {
    configurable: true,
    enumerable: true,
    get() {
      const value = globalThis.__thinpack_require__("node:perf_hooks").performance;
      Object.defineProperty(globalThis, "performance", {
        value,
        writable: true,
        configurable: true,
        enumerable: true,
      });
      return value;
    },
  });
})();
