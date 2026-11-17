(() => {
  const kTarget = Symbol("kTarget");
  const kIsBeingDispatched = Symbol("kIsBeingDispatched");
  const kTrustEvent = Symbol("kTrustEvent");

  class Event {
    constructor(type, init = {}) {
      this.type = String(type);
      this.bubbles = Boolean(init.bubbles);
      this.cancelable = Boolean(init.cancelable);
      this.composed = Boolean(init.composed);
      this[kTarget] = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.defaultPrevented = false;
      this[kIsBeingDispatched] = false;
      this[kTrustEvent] = Boolean(init[kTrustEvent]);
      this.timeStamp = Date.now();
      this._stop = false;
      this._stopImmediate = false;
    }

    get target() {
      return this[kTarget];
    }

    set target(value) {
      this[kTarget] = value;
    }

    get isTrusted() {
      return this[kTrustEvent];
    }

    preventDefault() {
      if (this.cancelable) this.defaultPrevented = true;
    }

    stopPropagation() {
      this._stop = true;
    }

    stopImmediatePropagation() {
      this._stop = true;
      this._stopImmediate = true;
    }

    composedPath() {
      return this.target ? [this.target] : [];
    }
  }

  Object.defineProperties(Event, {
    NONE: { value: 0, writable: false, configurable: false, enumerable: true },
    CAPTURING_PHASE: { value: 1, writable: false, configurable: false, enumerable: true },
    AT_TARGET: { value: 2, writable: false, configurable: false, enumerable: true },
    BUBBLING_PHASE: { value: 3, writable: false, configurable: false, enumerable: true },
  });

  class CustomEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.detail = init.detail ?? null;
    }
  }

  class EventTarget {
    constructor() {
      this._listeners = null;
      this[Symbol.for("nodejs.event_target.max_listeners")] = 10;
      this[Symbol.for("nodejs.event_target.max_listeners_warned")] = false;
    }

    addEventListener(type, listener, options) {
      if (listener === null || listener === undefined) return;
      if (!this._listeners) this._listeners = new Map();
      const key = String(type);
      const list = this._listeners.get(key) ?? [];
      if (!list.some((entry) => entry.listener === listener)) {
        list.push({
          listener,
          once: Boolean(options && options.once),
        });
        this._listeners.set(key, list);
      }
    }

    removeEventListener(type, listener) {
      if (!this._listeners) return;
      const key = String(type);
      const list = this._listeners.get(key);
      if (!list) return;
      const remaining = list.filter((entry) => entry.listener !== listener);
      if (remaining.length === 0) this._listeners.delete(key);
      else this._listeners.set(key, remaining);
    }

    dispatchEvent(event) {
      if (!this._listeners) return true;
      const list = this._listeners.get(event.type);
      event.target = this;
      event.currentTarget = this;
      event.eventPhase = 2;
      if (list) {
        for (const entry of list.slice()) {
          if (entry.once) this.removeEventListener(event.type, entry.listener);
          if (typeof entry.listener === "function") {
            entry.listener.call(this, event);
          } else if (entry.listener && typeof entry.listener.handleEvent === "function") {
            entry.listener.handleEvent(event);
          }
          if (event._stopImmediate) break;
        }
      }
      event.eventPhase = 0;
      return !event.defaultPrevented;
    }
  }

  Object.defineProperty(globalThis, "Event", {
    value: Event, writable: true, configurable: true, enumerable: false,
  });
  Object.defineProperty(globalThis, "CustomEvent", {
    value: CustomEvent, writable: true, configurable: true, enumerable: false,
  });
  Object.defineProperty(globalThis, "EventTarget", {
    value: EventTarget, writable: true, configurable: true, enumerable: false,
  });
  Object.defineProperty(globalThis, "WebSocket", {
    configurable: true,
    enumerable: false,
    get() {
      let WebSocket;
      try {
        ({ WebSocket } = globalThis.__thinpack_require__("node:thinpack/internal/websocket"));
      } catch (error) {
        const wrapped = new Error(
          "WebSocket is not available in this build; re-pack with --with http,https,crypto,buffer",
        );
        wrapped.code = error.code ?? "ERR_UNKNOWN_BUILTIN_MODULE";
        wrapped.cause = error;
        throw wrapped;
      }
      Object.defineProperty(globalThis, "WebSocket", {
        value: WebSocket, writable: true, configurable: true, enumerable: false,
      });
      return WebSocket;
    },
    set(value) {
      Object.defineProperty(globalThis, "WebSocket", {
        value, writable: true, configurable: true, enumerable: false,
      });
    },
  });

  Object.defineProperty(globalThis, "__thinpack_event", {
    value: { Event, CustomEvent, EventTarget, kTarget, kIsBeingDispatched, kTrustEvent },
    writable: false, configurable: false, enumerable: false,
  });
})();
