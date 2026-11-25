(() => {
  const names = ["fetch", "Headers", "Request", "Response", "FormData", "WebSocket"];

  const load = () => globalThis.__thinpack_require__("node:thinpack/internal/undici");

  for (const name of names) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      enumerable: false,
      get() {
        const value = load()[name];
        Object.defineProperty(globalThis, name, {
          value, writable: true, configurable: true, enumerable: false,
        });
        return value;
      },
      set(value) {
        Object.defineProperty(globalThis, name, {
          value, writable: true, configurable: true, enumerable: false,
        });
      },
    });
  }
})();
