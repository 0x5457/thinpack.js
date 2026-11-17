(() => {
  const abortError = () =>
    new DOMException("This operation was aborted", "AbortError");

  class AbortSignal {
    constructor() {
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
      this._listeners = null;
      this[Symbol.for("nodejs.event_target.max_listeners")] = 0;
    }

    addEventListener(type, listener, options) {
      if (type !== "abort" || typeof listener !== "function") return;
      if (!this._listeners) this._listeners = [];
      this._listeners.push({
        listener,
        once: Boolean(options && options.once),
      });
    }

    removeEventListener(type, listener) {
      if (type !== "abort" || !this._listeners) return;
      this._listeners = this._listeners.filter(
        (entry) => entry.listener !== listener,
      );
    }

    throwIfAborted() {
      if (this.aborted) throw this.reason;
    }

    _abort(reason) {
      if (this.aborted) return;
      this.aborted = true;
      this.reason = reason === undefined ? abortError() : reason;
      const listeners = this._listeners || [];
      this._listeners = null;
      const event = { type: "abort", target: this };
      if (typeof this.onabort === "function") this.onabort(event);
      for (const entry of listeners) {
        entry.listener.call(this, event);
        }
    }

    static abort(reason) {
      const signal = new AbortSignal();
      signal._abort(reason);
      return signal;
    }

    static any(signals) {
      const signal = new AbortSignal();
      for (const source of signals) {
        if (source.aborted) {
          signal._abort(source.reason);
          break;
        }
        source.addEventListener(
          "abort",
          () => signal._abort(source.reason),
          { once: true },
        );
      }
      return signal;
    }

    static timeout(milliseconds) {
      const signal = new AbortSignal();
      const timer = setTimeout(
        () => signal._abort(abortError()),
        milliseconds,
      );
      if (typeof timer.unref === "function") timer.unref();
      return signal;
    }
  }

  class AbortController {
    constructor() {
      this.signal = new AbortSignal();
    }

    abort(reason) {
      this.signal._abort(reason);
    }
  }

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
