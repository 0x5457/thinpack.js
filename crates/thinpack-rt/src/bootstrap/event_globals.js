// Adapted from Node.js v26.13.1 lib/internal/event_target.js
// (https://github.com/nodejs/node, MIT). The listener table stays behind
// symbols owned by this module; the internal adapter only re-exports what
// node:events needs to read it.
//
// Copyright Joyent, Inc. and other Node contributors.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the
// "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish,
// distribute, sublicense, and/or sell copies of the Software, and to permit
// persons to whom the Software is furnished to do so, subject to the
// following conditions:
//
// The above copyright notice and this permission notice shall be included
// in all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
// NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
// OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
// USE OR OTHER DEALINGS IN THE SOFTWARE.

(() => {
  const kEvents = Symbol("kEvents");
  const kHandlers = Symbol("kHandlers");
  const kTarget = Symbol("kTarget");
  const kIsBeingDispatched = Symbol("kIsBeingDispatched");
  const kTrustEvent = Symbol("kTrustEvent");
  const kStop = Symbol("kStop");
  const kStopPropagation = Symbol("kStopPropagation");
  const kType = Symbol("type");
  const kDetail = Symbol("detail");
  const kDefaultPrevented = Symbol("defaultPrevented");
  const kBubbles = Symbol("bubbles");
  const kCancelable = Symbol("cancelable");
  const kComposed = Symbol("composed");
  const kTimeStamp = Symbol("timeStamp");
  const kInPassiveListener = Symbol("kInPassiveListener");
  const kWeakHandler = Symbol("kWeak");
  const kResistStopPropagation = Symbol("kResistStopPropagation");
  const kIsEventTarget = Symbol.for("nodejs.event_target");
  const kMaxEventTargetListeners = Symbol.for("nodejs.event_target.max_listeners");
  const kMaxEventTargetListenersWarned =
    Symbol.for("nodejs.event_target.max_listeners_warned");

  const kFlagOnce = 1 << 0;
  const kFlagCapture = 1 << 1;
  const kFlagPassive = 1 << 2;
  const kFlagResistStopPropagation = 1 << 3;
  const kFlagRemoved = 1 << 4;

  const kEmptyObject = Object.freeze({ __proto__: null });

  const invalidArgType = (name, expected, actual) => {
    const type = name.includes(".") ? "property" : "argument";
    const error = new TypeError(
      `The "${name}" ${type} must be ${expected}. Received ${String(actual)}`,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    return error;
  };

  const missingArgs = (...names) => {
    const quoted = names.map((name) => `"${name}"`);
    const joined = quoted.length <= 1
      ? quoted.join("")
      : `${quoted.slice(0, -1).join(", ")} and ${quoted[quoted.length - 1]}`;
    const error = new TypeError(
      `The ${joined} argument${names.length > 1 ? "s" : ""} must be specified`,
    );
    error.code = "ERR_MISSING_ARGS";
    return error;
  };

  const invalidThis = (type) => {
    const error = new TypeError(`Value of "this" must be of type ${type}`);
    error.code = "ERR_INVALID_THIS";
    return error;
  };

  class Event {
    constructor(type, options = undefined) {
      if (arguments.length === 0) throw missingArgs("type");
      if (options !== undefined && options !== null) {
        if (typeof options !== "object" || Array.isArray(options)) {
          throw invalidArgType("options", "object", options);
        }
      }
      this[kType] = `${type}`;
      this[kTarget] = null;
      this[kIsBeingDispatched] = false;
      this[kStop] = false;
      this[kStopPropagation] = false;
      this[kInPassiveListener] = false;
      this[kTrustEvent] = Boolean(options?.[kTrustEvent]);
      this[kBubbles] = Boolean(options?.bubbles);
      this[kCancelable] = Boolean(options?.cancelable);
      this[kComposed] = Boolean(options?.composed);
      this[kDefaultPrevented] = false;
      this[kTimeStamp] = Date.now();
    }

    get target() {
      return this[kTarget];
    }

    get currentTarget() {
      return this[kIsBeingDispatched] ? this[kTarget] : null;
    }

    get srcElement() {
      return this[kTarget];
    }

    get type() {
      return this[kType];
    }

    get cancelable() {
      return this[kCancelable];
    }

    get defaultPrevented() {
      return this[kCancelable] && this[kDefaultPrevented];
    }

    get timeStamp() {
      return this[kTimeStamp];
    }

    get composedPath() {
      return this[kIsBeingDispatched] ? [this[kTarget]] : [];
    }

    get returnValue() {
      return !this[kCancelable] || !this[kDefaultPrevented];
    }

    set returnValue(value) {
      if (!value) this.preventDefault();
    }

    get bubbles() {
      return this[kBubbles];
    }

    get composed() {
      return this[kComposed];
    }

    get eventPhase() {
      return this[kIsBeingDispatched] ? Event.AT_TARGET : Event.NONE;
    }

    get cancelBubble() {
      return this[kStopPropagation];
    }

    set cancelBubble(value) {
      if (value) this[kStopPropagation] = true;
    }

    get isTrusted() {
      return this[kTrustEvent];
    }

    initEvent(type, bubbles = false, cancelable = false) {
      if (arguments.length === 0) throw missingArgs("type");
      if (this[kIsBeingDispatched]) return;
      this[kType] = `${type}`;
      this[kBubbles] = Boolean(bubbles);
      this[kCancelable] = Boolean(cancelable);
    }

    preventDefault() {
      if (!this[kCancelable] || this[kInPassiveListener]) return;
      this[kDefaultPrevented] = true;
    }

    stopPropagation() {
      this[kStopPropagation] = true;
    }

    stopImmediatePropagation() {
      this[kStopPropagation] = true;
      this[kStop] = true;
    }
  }

  Object.defineProperties(Event, {
    NONE: { value: 0, writable: false, configurable: false, enumerable: true },
    CAPTURING_PHASE: { value: 1, writable: false, configurable: false, enumerable: true },
    AT_TARGET: { value: 2, writable: false, configurable: false, enumerable: true },
    BUBBLING_PHASE: { value: 3, writable: false, configurable: false, enumerable: true },
  });

  const enumerable = (value) => ({
    __proto__: null,
    writable: true,
    enumerable: true,
    configurable: true,
    value,
  });

  function makeEnumerable(prototype, names, configurable = true) {
    const descriptors = Object.getOwnPropertyDescriptors(prototype);
    for (const name of names) {
      const descriptor = descriptors[name];
      descriptor.enumerable = true;
      descriptor.configurable = configurable;
      Object.defineProperty(prototype, name, descriptor);
    }
  }

  makeEnumerable(Event.prototype, [
    "target",
    "currentTarget",
    "srcElement",
    "type",
    "cancelable",
    "defaultPrevented",
    "timeStamp",
    "composedPath",
    "returnValue",
    "bubbles",
    "composed",
    "eventPhase",
    "cancelBubble",
    "initEvent",
    "preventDefault",
    "stopPropagation",
    "stopImmediatePropagation",
  ]);
  makeEnumerable(Event.prototype, ["isTrusted"], false);
  Object.defineProperty(Event.prototype, Symbol.toStringTag, {
    __proto__: null,
    writable: false,
    enumerable: false,
    configurable: true,
    value: "Event",
  });

  class CustomEvent extends Event {
    constructor(type, options = kEmptyObject) {
      if (arguments.length === 0) throw missingArgs("type");
      super(type, options);
      this[kDetail] = options?.detail ?? null;
    }

    get detail() {
      return this[kDetail];
    }
  }

  makeEnumerable(CustomEvent.prototype, ["detail"]);
  Object.defineProperty(CustomEvent.prototype, Symbol.toStringTag, {
    __proto__: null,
    writable: false,
    enumerable: false,
    configurable: true,
    value: "CustomEvent",
  });

  class Listener {
    constructor(previous, listener, once, capture, passive, resistStopPropagation) {
      this.next = undefined;
      if (previous !== undefined) previous.next = this;
      this.previous = previous;
      this.listener = listener;

      let flags = 0;
      if (once) flags |= kFlagOnce;
      if (capture) flags |= kFlagCapture;
      if (passive) flags |= kFlagPassive;
      if (resistStopPropagation) flags |= kFlagResistStopPropagation;
      this.flags = flags;
    }

    get once() {
      return Boolean(this.flags & kFlagOnce);
    }

    get capture() {
      return Boolean(this.flags & kFlagCapture);
    }

    get passive() {
      return Boolean(this.flags & kFlagPassive);
    }

    get resistStopPropagation() {
      return Boolean(this.flags & kFlagResistStopPropagation);
    }

    get removed() {
      return Boolean(this.flags & kFlagRemoved);
    }

    same(listener, capture) {
      return this.listener === listener && this.capture === capture;
    }

    remove() {
      if (this.previous !== undefined) this.previous.next = this.next;
      if (this.next !== undefined) this.next.previous = this.previous;
      this.flags |= kFlagRemoved;
    }
  }

  function initEventTarget(target) {
    target[kEvents] = undefined;
    target[kHandlers] = undefined;
    target[kMaxEventTargetListeners] = 10;
    target[kMaxEventTargetListenersWarned] = false;
  }

  function getEventsMap(target) {
    if (target[kEvents] === undefined) target[kEvents] = new Map();
    return target[kEvents];
  }

  function isEventTarget(value) {
    return Boolean(value?.constructor?.[kIsEventTarget]);
  }

  function dispatch(target, event) {
    const type = event[kType];
    const root = target[kEvents]?.get(type);
    if (root === undefined || root.next === undefined) return true;

    event[kTarget] = target;
    event[kIsBeingDispatched] = true;
    try {
      let handler = root.next;
      while (handler !== undefined) {
        const next = handler.next;
        if (handler.removed ||
            (event[kStop] === true && !handler.resistStopPropagation)) {
          handler = next;
          continue;
        }
        if (handler.once) {
          handler.remove();
          root.size--;
          if (root.size === 0) target[kEvents].delete(type);
        }

        const listener = handler.listener;
        if (handler.passive) event[kInPassiveListener] = true;
        try {
          if (typeof listener === "function") listener.call(target, event);
          else if (listener && typeof listener.handleEvent === "function") {
            listener.handleEvent(event);
          }
        } finally {
          if (handler.passive) event[kInPassiveListener] = false;
        }
        handler = next;
      }
    } finally {
      event[kIsBeingDispatched] = false;
    }
    return !event.defaultPrevented;
  }

  class EventTarget {
    constructor() {
      initEventTarget(this);
    }

    addEventListener(type, listener, options = kEmptyObject) {
      if (!isEventTarget(this)) throw invalidThis("EventTarget");
      if (arguments.length < 2) throw missingArgs("type", "listener");

      let once = false;
      let capture = false;
      let passive = false;
      let resistStopPropagation = false;

      if (options !== kEmptyObject && options !== null) {
        if (typeof options === "boolean") {
          capture = options;
        } else {
          if (typeof options !== "object" || Array.isArray(options)) {
            throw invalidArgType("options", "object", options);
          }
          once = Boolean(options.once);
          capture = Boolean(options.capture);
          passive = Boolean(options.passive);
          resistStopPropagation = Boolean(options[kResistStopPropagation]);

          const signal = options.signal;
          if (signal !== undefined) {
            if (signal === null || typeof signal !== "object" ||
                !("aborted" in signal)) {
              throw invalidArgType("options.signal", "AbortSignal", signal);
            }
            if (signal.aborted) return;
            signal.addEventListener("abort", () => {
              this.removeEventListener(type, listener, options);
            }, {
              __proto__: null,
              once: true,
              [kWeakHandler]: this,
              [kResistStopPropagation]: true,
            });
          }
        }
      }

      if (listener === null || listener === undefined) return;
      if (typeof listener !== "function" &&
          (typeof listener !== "object" || listener === null)) {
        throw invalidArgType("listener", "EventListener", listener);
      }

      type = `${type}`;
      const eventsMap = getEventsMap(this);
      let root = eventsMap.get(type);
      if (root === undefined) {
        root = {
          size: 1,
          next: undefined,
          resistStopPropagation,
        };
        new Listener(
          root,
          listener,
          once,
          capture,
          passive,
          resistStopPropagation,
        );
        eventsMap.set(type, root);
        return;
      }

      let previous = root;
      let handler = root.next;
      while (handler !== undefined && !handler.same(listener, capture)) {
        previous = handler;
        handler = handler.next;
      }
      if (handler !== undefined) return;

      new Listener(
        previous,
        listener,
        once,
        capture,
        passive,
        resistStopPropagation,
      );
      root.size++;
      root.resistStopPropagation ||= resistStopPropagation;
    }

    removeEventListener(type, listener, options = kEmptyObject) {
      if (!isEventTarget(this)) throw invalidThis("EventTarget");
      if (arguments.length < 2) throw missingArgs("type", "listener");
      if (listener === null || listener === undefined) return;
      if (typeof listener !== "function" &&
          (typeof listener !== "object" || listener === null)) {
        throw invalidArgType("listener", "EventListener", listener);
      }

      type = `${type}`;
      const capture = typeof options === "boolean"
        ? options
        : options?.capture === true;

      if (this[kEvents] === undefined) return;
      const root = this[kEvents].get(type);
      if (root === undefined || root.next === undefined) return;

      let handler = root.next;
      while (handler !== undefined) {
        if (handler.same(listener, capture)) {
          handler.remove();
          root.size--;
          if (root.size === 0) this[kEvents].delete(type);
          break;
        }
        handler = handler.next;
      }
    }

    dispatchEvent(event) {
      if (!isEventTarget(this)) throw invalidThis("EventTarget");
      if (arguments.length < 1) throw missingArgs("event");
      if (!(event instanceof Event)) {
        throw invalidArgType("event", "an instance of Event", event);
      }
      if (event[kIsBeingDispatched]) {
        const error = new Error(
          `The event "${event.type}" is already being dispatched`,
        );
        error.code = "ERR_EVENT_RECURSION";
        throw error;
      }
      return dispatch(this, event);
    }
  }

  Object.defineProperty(EventTarget, kIsEventTarget, {
    value: true,
    writable: true,
    configurable: true,
    enumerable: false,
  });

  Object.defineProperties(EventTarget.prototype, {
    addEventListener: enumerable(EventTarget.prototype.addEventListener),
    removeEventListener: enumerable(EventTarget.prototype.removeEventListener),
    dispatchEvent: enumerable(EventTarget.prototype.dispatchEvent),
    [Symbol.toStringTag]: {
      __proto__: null,
      writable: false,
      enumerable: false,
      configurable: true,
      value: "EventTarget",
    },
  });

  function makeEventHandler(handler) {
    function eventHandler(...args) {
      if (typeof eventHandler.handler !== "function") return;
      return Reflect.apply(eventHandler.handler, this, args);
    }
    eventHandler.handler = handler;
    return eventHandler;
  }

  function defineEventHandler(target, name, event = name) {
    const property = `on${name}`;

    function get() {
      return this[kHandlers]?.get(event)?.handler ?? null;
    }

    function set(value) {
      if (typeof value !== "function" &&
          (typeof value !== "object" || value === null)) {
        value = null;
      }
      if (this[kHandlers] === undefined) this[kHandlers] = new Map();

      let wrapped = this[kHandlers].get(event);
      if (wrapped === undefined) {
        wrapped = makeEventHandler(value);
        this[kHandlers].set(event, wrapped);
        this.addEventListener(event, wrapped);
        return;
      }

      const root = this[kEvents]?.get(event);
      if (typeof wrapped.handler === "function" && root !== undefined) {
        root.size--;
      }
      wrapped.handler = value;
      if (typeof wrapped.handler === "function") {
        if (root !== undefined) root.size++;
        else this.addEventListener(event, wrapped);
      }
    }

    Object.defineProperty(target, property, {
      __proto__: null,
      get,
      set,
      configurable: true,
      enumerable: true,
    });
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

  Object.defineProperty(globalThis, "__thinpack_event", {
    value: {
      Event,
      CustomEvent,
      EventTarget,
      defineEventHandler,
      isEventTarget,
      kEvents,
      kIsBeingDispatched,
      kTarget,
      kTrustEvent,
      kWeakHandler,
      kResistStopPropagation,
      kMaxEventTargetListeners,
      kMaxEventTargetListenersWarned,
    },
    writable: false, configurable: false, enumerable: false,
  });
})();
