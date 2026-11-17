(() => {
  Object.defineProperty(globalThis, "CryptoKey", {
    configurable: true,
    enumerable: false,
    get() {
      return globalThis.__thinpack_require__("node:crypto").CryptoKey;
    },
  });
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    enumerable: false,
    get() {
      return globalThis.__thinpack_require__("node:crypto").webcrypto;
    },
  });
})();
