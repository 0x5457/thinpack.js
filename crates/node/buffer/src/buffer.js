(() => {
  const install = (value) => {
    Object.defineProperty(globalThis, "Buffer", {
      configurable: true,
      enumerable: false,
      value,
      writable: true,
    });
  };

  Object.defineProperty(globalThis, "Buffer", {
    configurable: true,
    enumerable: false,
    get() {
      const { Buffer } = globalThis.__thinpack_require__("node:buffer");
      install(Buffer);
      return Buffer;
    },
    set: install,
  });
})();
