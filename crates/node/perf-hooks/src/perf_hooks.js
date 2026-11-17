(() => {
  const timeOrigin = Date.now();
  let lastMark = 0;
  const observers = new Set();

  const now = () => Deno.core.ops.op_perf_now();

  const entries = [];

  const deliver = (entry) => {
    for (const observer of observers) observer._deliver(entry);
  };

  const performance = {
    timeOrigin,
    now,
    mark(name) {
      const entry = { name: String(name), entryType: "mark", startTime: now(), duration: 0 };
      entries.push(entry);
      lastMark = entry.startTime;
      deliver(entry);
      return entry;
    },
    measure(name) {
      const startTime = lastMark;
      lastMark = 0;
      const entry = {
        name: String(name),
        entryType: "measure",
        startTime,
        duration: now() - startTime,
      };
      entries.push(entry);
      deliver(entry);
      return entry;
    },
    getEntries: () => entries.slice(),
    getEntriesByName: (name) => entries.filter((entry) => entry.name === name),
    getEntriesByType: (type) => entries.filter((entry) => entry.entryType === type),
    clearMarks(name) {
      for (let index = entries.length - 1; index >= 0; index -= 1) {
        if (entries[index].entryType === "mark" && (name === undefined || entries[index].name === name)) {
          entries.splice(index, 1);
        }
      }
    },
    clearMeasures(name) {
      for (let index = entries.length - 1; index >= 0; index -= 1) {
        if (entries[index].entryType === "measure" && (name === undefined || entries[index].name === name)) {
          entries.splice(index, 1);
        }
      }
    },
    eventLoopUtilization: () => ({ idle: 0, active: now(), utilization: 0 }),
    markResourceTiming() {},
  };

  class PerformanceObserver {
    #callback;
    #types = null;
    #records = [];

    constructor(callback) {
      this.#callback = callback;
      observers.add(this);
    }

    observe(options = {}) {
      const types = options.entryTypes ?? (options.type === undefined ? [] : [options.type]);
      this.#types = new Set(types);
    }

    disconnect() {
      observers.delete(this);
    }

    takeRecords() {
      const records = this.#records;
      this.#records = [];
      return records;
    }

    _deliver(entry) {
      if (this.#types !== null && !this.#types.has(entry.entryType)) return;
      queueMicrotask(() => {
        if (!observers.has(this)) return;
        const list = {
          getEntries: () => [entry],
          getEntriesByName: (name) => (entry.name === name ? [entry] : []),
          getEntriesByType: (type) => (entry.entryType === type ? [entry] : []),
        };
        this.#callback(list, this);
      });
    }
  }

  module.exports = {
    performance,
    PerformanceObserver,
    PerformanceEntry: class PerformanceEntry {},
    PerformanceMark: class PerformanceMark {},
    PerformanceMeasure: class PerformanceMeasure {},
    constants: { NODE_PERFORMANCE_GC_MAJOR: 4 },
    monitorEventLoopDelay() {
      const { codes } = require("node:thinpack/internal/errors");
      throw new codes.ERR_NOT_IMPLEMENTED("perf_hooks.monitorEventLoopDelay");
    },
  };
})();
