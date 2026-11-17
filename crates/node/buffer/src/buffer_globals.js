(() => {
  for (const name of ["Blob", "File"]) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      enumerable: false,
      get() {
        return globalThis.__thinpack_require__("node:buffer")[name];
      },
    });
  }
})();
