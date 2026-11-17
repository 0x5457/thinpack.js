(() => {
  const primordials = Object.create(null);
  for (const key of Reflect.ownKeys(globalThis.__bootstrap.primordials)) {
    primordials[key] = globalThis.__bootstrap.primordials[key];
  }

  if (primordials.AsyncIteratorPrototype === undefined) {
    const iterator = (async function* () {})();
    primordials.AsyncIteratorPrototype = Object.getPrototypeOf(
      Object.getPrototypeOf(iterator),
    );
  }
  for (const name of ["SafeSet", "SafeMap", "SafeWeakSet", "SafeWeakMap"]) {
    const Original = primordials[name];
    primordials[name] = class extends Original {
      constructor(iterable) {
        super(iterable == null ? undefined : Array.from(iterable));
      }
    };
  }

  primordials.SymbolDispose ??= Symbol.dispose ?? Symbol.for("nodejs.dispose");
  primordials.SymbolAsyncDispose ??=
    Symbol.asyncDispose ?? Symbol.for("nodejs.asyncDispose");

  Object.defineProperty(globalThis, "primordials", {
    value: primordials,
    writable: true,
    configurable: true,
    enumerable: false,
  });
})();
