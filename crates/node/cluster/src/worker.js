(() => {
  const { EventEmitter } = require("node:events");

  // Shared by the primary and the worker, the same shape as Node's internal/cluster/worker.
  class Worker extends EventEmitter {
    constructor(options = {}) {
      super();
      this.exitedAfterDisconnect = undefined;
      this.state = options.state || "none";
      this.id = options.id | 0;

      if (options.process) {
        this.process = options.process;
        this.process.on("error", (error) => this.emit("error", error));
        this.process.on("message", (message, handle) => this.emit("message", message, handle));
      }
    }

    kill(...args) {
      this.destroy(...args);
    }

    send(...args) {
      return this.process.send(...args);
    }

    isDead() {
      return this.process.exitCode != null || this.process.signalCode != null;
    }

    isConnected() {
      return this.process.connected;
    }
  }

  // Node exposes Worker as a function that is also callable without `new`; keep the class
  // internals and add the callable shape.
  module.exports = new Proxy(Worker, {
    apply: (target, thisArg, args) => new target(...args),
  });
})();
