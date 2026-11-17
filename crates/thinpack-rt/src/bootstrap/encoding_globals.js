(() => {
  for (const name of ["TextEncoder", "TextDecoder"]) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      enumerable: false,
      get() {
        return globalThis.__thinpack_require__("node:thinpack/internal/encoding")[name];
      },
    });
  }
})();
