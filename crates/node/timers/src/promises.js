(() => {
  const timers = globalThis.__thinpack_timers;

  const { AbortError } = require("node:thinpack/internal/errors");

  const validateSignal = (options) => {
    const signal = options && options.signal;
    if (
      signal !== undefined &&
      (signal === null || typeof signal !== "object" || !("aborted" in signal))
    ) {
      throw new TypeError("The \"options.signal\" argument must be an AbortSignal");
    }
    return signal;
  };

  const rejection = (signal) => Promise.reject(new AbortError(undefined, { cause: signal.reason }));

  const setTimeout = (delay, value, options) => {
    const signal = validateSignal(options);
    if (signal === undefined || !signal.aborted) {
      return new Promise((resolve, reject) => {
        const timer = timers.setTimeout(() => {
          signal?.removeEventListener("abort", onAbort);
          resolve(value);
        }, delay);
        const onAbort = () => {
          timers.clearTimeout(timer);
          reject(new AbortError(undefined, { cause: signal.reason }));
        };
        signal?.addEventListener("abort", onAbort, { once: true });
      });
    }
    return rejection(signal);
  };

  const setImmediate = (value, options) => {
    const signal = validateSignal(options);
    if (signal === undefined || !signal.aborted) {
      return new Promise((resolve, reject) => {
        const timer = timers.setImmediate(() => {
          signal?.removeEventListener("abort", onAbort);
          resolve(value);
        });
        const onAbort = () => {
          timers.clearImmediate(timer);
          reject(new AbortError(undefined, { cause: signal.reason }));
        };
        signal?.addEventListener("abort", onAbort, { once: true });
      });
    }
    return rejection(signal);
  };

  const setInterval = (delay, value, options) => {
    const signal = validateSignal(options);
    if (signal !== undefined && signal.aborted) {
      return rejection(signal);
    }

    const queue = [];
    let waiter;
    let waiterReject;
    const timer = timers.setInterval(() => {
      if (waiter !== undefined) {
        const current = waiter;
        waiter = undefined;
        current({ value, done: false });
      } else {
        queue.push(value);
      }
    }, delay);

    const iterator = {
      [Symbol.asyncIterator]() {
        return this;
      },
      next() {
        if (queue.length > 0) {
          return Promise.resolve({ value: queue.shift(), done: false });
        }
        if (signal !== undefined && signal.aborted) {
          return Promise.reject(new AbortError(undefined, { cause: signal.reason }));
        }
        return new Promise((resolve, reject) => {
          waiter = resolve;
          waiterReject = reject;
        });
      },
      return() {
        timers.clearInterval(timer);
        return Promise.resolve({ value: undefined, done: true });
      },
      ref() {
        timer.ref();
        return this;
      },
      unref() {
        timer.unref();
        return this;
      },
    };

    if (signal !== undefined) {
      signal.addEventListener(
        "abort",
        () => {
          timers.clearInterval(timer);
          if (waiterReject !== undefined) {
            const reject = waiterReject;
            waiterReject = undefined;
            waiter = undefined;
            reject(new AbortError(undefined, { cause: signal.reason }));
          }
        },
        { once: true },
      );
    }
    return iterator;
  };

  module.exports = { setTimeout, setImmediate, setInterval };
})();
