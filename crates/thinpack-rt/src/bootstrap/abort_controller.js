// AbortSignal follows Node.js v26.13.1 lib/internal/abort_controller.js (MIT,
// https://github.com/nodejs/node): one EventTarget implementation and one
// dispatch path are shared with the global EventTarget.

(() => {
  const {
    Event,
    EventTarget,
    defineEventHandler,
    kTrustEvent,
    kWeakHandler,
    kResistStopPropagation,
    kMaxEventTargetListeners,
  } = globalThis.__thinpack_event;

  const kAborted = Symbol("kAborted");
  const kReason = Symbol("kReason");
  const kSignal = Symbol("kSignal");
  const kDontThrow = Symbol("kDontThrow");

  const abortError = () =>
    new DOMException("This operation was aborted", "AbortError");

  const timeoutError = () =>
    new DOMException("The operation was aborted due to timeout", "TimeoutError");

  const invalidArgType = (name, expected, actual) => {
    const error = new TypeError(
      `The "${name}" argument must be ${expected}. Received ${String(actual)}`,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    return error;
  };

  function makeEnumerable(prototype, names) {
    const descriptors = Object.getOwnPropertyDescriptors(prototype);
    for (const name of names) {
      const descriptor = descriptors[name];
      descriptor.enumerable = true;
      Object.defineProperty(prototype, name, descriptor);
    }
  }

  function abortSignal(signal, reason) {
    if (signal[kAborted]) return;
    signal[kAborted] = true;
    signal[kReason] = reason;
    signal.dispatchEvent(new Event("abort", { [kTrustEvent]: true }));
  }

  class AbortSignal extends EventTarget {
    constructor(token) {
      if (token !== kDontThrow) {
        const error = new TypeError("Illegal constructor");
        error.code = "ERR_ILLEGAL_CONSTRUCTOR";
        throw error;
      }
      super();
      this[kMaxEventTargetListeners] = 0;
      this[kAborted] = false;
      this[kReason] = undefined;
    }

    get aborted() {
      return this[kAborted];
    }

    get reason() {
      return this[kReason];
    }

    throwIfAborted() {
      if (this[kAborted]) throw this[kReason];
    }

    static abort(reason = abortError()) {
      const signal = new AbortSignal(kDontThrow);
      signal[kAborted] = true;
      signal[kReason] = reason;
      return signal;
    }

    static any(signals) {
      const sources = Array.from(signals);
      for (const source of sources) {
        if (!(source instanceof AbortSignal)) {
          throw invalidArgType("signals", "sequence of AbortSignal", source);
        }
      }

      const signal = new AbortSignal(kDontThrow);
      const removers = [];

      const abort = (reason) => {
        if (signal[kAborted]) return;
        abortSignal(signal, reason);
        for (const remove of removers) remove();
        removers.length = 0;
      };

      for (const source of sources) {
        if (source[kAborted]) {
          abort(source[kReason]);
          break;
        }
        const onAbort = () => abort(source[kReason]);
        source.addEventListener("abort", onAbort, {
          __proto__: null,
          once: true,
          [kWeakHandler]: signal,
          [kResistStopPropagation]: true,
        });
        removers.push(() => source.removeEventListener("abort", onAbort));
      }
      return signal;
    }

    static timeout(milliseconds) {
      const signal = new AbortSignal(kDontThrow);
      const timer = setTimeout(
        () => abortSignal(signal, timeoutError()),
        milliseconds,
      );
      if (typeof timer.unref === "function") timer.unref();
      return signal;
    }
  }

  makeEnumerable(AbortSignal.prototype, ["aborted", "reason", "throwIfAborted"]);
  Object.defineProperty(AbortSignal.prototype, Symbol.toStringTag, {
    __proto__: null,
    writable: false,
    enumerable: false,
    configurable: true,
    value: "AbortSignal",
  });
  defineEventHandler(AbortSignal.prototype, "abort");

  class AbortController {
    get signal() {
      this[kSignal] ??= new AbortSignal(kDontThrow);
      return this[kSignal];
    }

    abort(reason = abortError()) {
      this[kSignal] ??= new AbortSignal(kDontThrow);
      abortSignal(this[kSignal], reason);
    }
  }

  makeEnumerable(AbortController.prototype, ["signal", "abort"]);
  Object.defineProperty(AbortController.prototype, Symbol.toStringTag, {
    __proto__: null,
    writable: false,
    enumerable: false,
    configurable: true,
    value: "AbortController",
  });

  Object.defineProperty(globalThis, "AbortController", {
    value: AbortController,
    writable: true,
    configurable: true,
    enumerable: false,
  });
  Object.defineProperty(globalThis, "AbortSignal", {
    value: AbortSignal,
    writable: true,
    configurable: true,
    enumerable: false,
  });
  Object.defineProperty(globalThis, "__thinpack_abort", {
    value: { AbortController, AbortSignal },
    writable: false,
    configurable: false,
    enumerable: false,
  });
})();
