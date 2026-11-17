(() => {
  const { EventEmitter } = require("node:events");
  const net = require("node:net");
  const { UVException } = require("node:thinpack/internal/errors");
  const Worker = require("node:thinpack/internal/cluster/worker");
  const channel = require("node:thinpack/internal/cluster/channel");

  const cluster = new EventEmitter();
  cluster.isWorker = true;
  cluster.isMaster = false;
  cluster.isPrimary = false;
  cluster.worker = null;
  cluster.Worker = Worker;

  const kClusterListen = Symbol.for("thinpack.cluster.listen");
  const servers = new Set();
  const listenIndexes = new Map();
  let worker = null;
  let disconnecting = false;

  cluster._setupWorker = function _setupWorker() {
    const id = Number(process.env.NODE_UNIQUE_ID) | 0;
    channel.setup();
    delete process.env.NODE_UNIQUE_ID;
    Deno.core.ops.op_cluster_clear_env();

    worker = new Worker({ id, process, state: "online" });
    cluster.worker = worker;

    process.once("disconnect", () => {
      worker.emit("disconnect");
      if (!worker.exitedAfterDisconnect) process.exit(0);
    });

    process.on("internalMessage", (message) => {
      if (message.cmd !== "NODE_CLUSTER" || message.act !== "disconnect") return;
      worker.exitedAfterDisconnect = true;
      disconnectWorker(worker);
    });

    net.Server[kClusterListen] = onListen;
    channel.sendOnline();
  };

  function onListen(server, options, bind) {
    if (options.port === 0) {
      const addressKey = String(options.address);
      const index = listenIndexes.get(addressKey) ?? 0;
      listenIndexes.set(addressKey, index + 1);
      channel.sendInternal(
        { act: "queryServer", address: options.address, port: options.port, index },
        (reply) => {
          if (reply.errno !== undefined && reply.errno !== 0) {
            const error = new UVException({ errno: reply.errno, syscall: "listen" });
            server.emit("error", error);
            return;
          }
          bind(reply.port);
        },
      );
    } else {
      bind(options.port);
    }

    server.once("listening", () => {
      worker.state = "listening";
      const address = server.address();
      // Node reports the address family as 4/6 and leaves `fd` undefined for a listener
      // that was created by the worker rather than passed in by the primary.
      const info = typeof address === "string"
        ? { addressType: -1, address, port: -1, fd: undefined }
        : {
          addressType: address.family === "IPv6" ? 6 : 4,
          address: address.address,
          port: address.port,
          fd: undefined,
        };
      channel.sendInternal({ act: "listening", ...info });
      worker.emit("listening", info);
    });

    servers.add(server);
    server.once("close", () => servers.delete(server));
  }

  function disconnectWorker(target) {
    if (disconnecting) return;
    disconnecting = true;
    target.state = "disconnecting";

    let remaining = 1;
    const done = () => {
      remaining -= 1;
      if (remaining === 0 && process.connected) process.disconnect();
    };
    for (const server of servers) {
      remaining += 1;
      server.close(done);
    }
    servers.clear();
    done();
  }

  Worker.prototype.disconnect = function disconnect() {
    if (this.state === "disconnecting" || this.state === "destroying") return this;
    this.exitedAfterDisconnect = true;
    this.state = "disconnecting";
    channel.sendInternal({ act: "exitedAfterDisconnect" }, () => {
      disconnectWorker(this);
    });
    return this;
  };

  Worker.prototype.destroy = function destroy() {
    this.exitedAfterDisconnect = true;
    if (!this.isConnected()) {
      process.exit(0);
    }
    this.state = "destroying";
    channel.sendInternal({ act: "exitedAfterDisconnect" });
    process.once("disconnect", () => process.exit(0));
    process.disconnect();
  };

  module.exports = cluster;
})();
