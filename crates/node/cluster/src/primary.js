(() => {
  const { EventEmitter } = require("node:events");
  const { codes, UVException } = require("node:thinpack/internal/errors");
  const Worker = require("node:thinpack/internal/cluster/worker");
  const WorkerProcess = require("node:thinpack/internal/cluster/worker_process");

  const SCHED_NONE = 1;
  const SCHED_RR = 2;

  const cluster = new EventEmitter();
  cluster.isWorker = false;
  cluster.isMaster = true;
  cluster.isPrimary = true;
  cluster.Worker = Worker;
  cluster.workers = {};
  cluster.settings = {};
  cluster.SCHED_NONE = SCHED_NONE;
  cluster.SCHED_RR = SCHED_RR;

  // Workers bind the same address with SO_REUSEPORT and the kernel distributes incoming
  // connections, which is the observable behaviour of Node's SCHED_NONE. SCHED_RR's
  // primary-side accept loop needs handle passing, which Thinpack does not implement.
  let schedulingPolicy = process.env.NODE_CLUSTER_SCHED_POLICY;
  if (schedulingPolicy === "rr") schedulingPolicy = SCHED_RR;
  else schedulingPolicy = SCHED_NONE;
  cluster.schedulingPolicy = schedulingPolicy;

  let ids = 0;
  const sharedPorts = new Map();

  cluster.setupPrimary = function setupPrimary(options) {
    const settings = {
      args: process.argv.slice(2),
      exec: process.argv[1],
      execArgv: [],
      silent: false,
      ...cluster.settings,
      ...options,
    };
    if (settings.serialization !== undefined && settings.serialization !== "json") {
      throw new codes.ERR_INVALID_ARG_VALUE(
        "options.serialization",
        settings.serialization,
        'must be "json"',
      );
    }
    cluster.settings = settings;
    process.nextTick(() => cluster.emit("setup", settings));
  };
  cluster.setupMaster = cluster.setupPrimary;

  cluster.fork = function fork(env) {
    cluster.setupPrimary();
    const id = ++ids;
    const worker = createWorker(id, env);
    cluster.workers[id] = worker;

    worker.on("message", (message, handle) => {
      cluster.emit("message", worker, message, handle);
    });
    worker.process.on("internalMessage", (message, handle) => {
      onInternalMessage(worker, message, handle);
    });
    worker.process.once("exit", (code, signal) => {
      if (!worker.isConnected()) removeWorker(worker);
      worker.exitedAfterDisconnect = Boolean(worker.exitedAfterDisconnect);
      worker.state = "dead";
      worker.emit("exit", code, signal);
      cluster.emit("exit", worker, code, signal);
    });
    worker.process.once("disconnect", () => {
      if (worker.isDead()) removeWorker(worker);
      worker.exitedAfterDisconnect = Boolean(worker.exitedAfterDisconnect);
      worker.state = "disconnected";
      worker.emit("disconnect");
      cluster.emit("disconnect", worker);
    });

    process.nextTick(() => cluster.emit("fork", worker));
    return worker;
  };

  cluster.disconnect = function disconnect(callback) {
    const workers = Object.values(cluster.workers);
    if (workers.length === 0) {
      if (callback !== undefined) process.nextTick(callback);
      return;
    }

    let remaining = 0;
    const done = () => {
      remaining -= 1;
      if (remaining === 0 && callback !== undefined) callback();
    };
    for (const worker of workers) {
      if (worker.isConnected()) {
        remaining += 1;
        worker.once("disconnect", done);
      }
      worker.disconnect();
    }
    if (remaining === 0 && callback !== undefined) process.nextTick(callback);
  };

  Worker.prototype.disconnect = function disconnect() {
    this.exitedAfterDisconnect = true;
    this.process.sendInternal({ act: "disconnect" });
    removeWorker(this);
    return this;
  };

  Worker.prototype.destroy = function destroy(signo) {
    this.process.kill(signo || "SIGTERM");
  };

  function createWorker(id, env) {
    const settings = cluster.settings;
    const args = [];
    if (settings.exec !== undefined && settings.exec !== process.execPath) {
      args.push(String(settings.exec));
    }
    args.push(...(settings.args ?? []).map(String));

    const environment = [];
    for (const [key, value] of Object.entries({ ...process.env, ...env })) {
      if (value !== undefined) environment.push(`${key}=${value}`);
    }

    let payload;
    try {
      payload = Deno.core.ops.op_cluster_spawn(
        [process.execPath, ...args].join("\0"),
        environment.join("\0"),
        settings.cwd ? String(settings.cwd) : "",
        settings.silent === true,
        id,
      );
    } catch (error) {
      if (/^-?\d+$/.test(String(error.code))) {
        throw new UVException({
          errno: Number(error.code),
          syscall: "spawn",
          path: process.execPath,
        });
      }
      throw error;
    }

    const [entryId, pid, channelId] = payload.split("\0").map(Number);
    const child = new WorkerProcess(entryId, pid, channelId, settings.silent === true);
    child.spawnargs = [process.execPath, ...args];
    return new Worker({ id, process: child });
  }

  function removeWorker(worker) {
    delete cluster.workers[worker.id];
  }

  function onInternalMessage(worker, message) {
    if (message.cmd !== "NODE_CLUSTER") return;
    switch (message.act) {
      case "online":
        worker.state = "online";
        worker.emit("online");
        cluster.emit("online", worker);
        break;
      case "listening":
        worker.state = "listening";
        {
          const info = {
            addressType: message.addressType,
            address: message.address,
            port: message.port,
            fd: message.fd,
          };
          worker.emit("listening", info);
          cluster.emit("listening", worker, info);
        }
        break;
      case "queryServer":
        queryServer(worker, message);
        break;
      case "exitedAfterDisconnect":
        worker.exitedAfterDisconnect = true;
        break;
      default:
        break;
    }
  }

  function queryServer(worker, message) {
    if (worker.exitedAfterDisconnect) return;

    let port = message.port;
    if (port === 0) {
      const host = String(message.address);
      const key = `${host}:${message.index}`;
      port = sharedPorts.get(key);
      if (port === undefined) {
        try {
          port = Deno.core.ops.op_cluster_allocate_port(host);
        } catch (error) {
          const errno = Number(error.code);
          worker.process.sendInternal({
            act: "port",
            seq: message.seq,
            errno: Number.isInteger(errno) ? errno : -5,
          });
          return;
        }
        sharedPorts.set(key, port);
      }
    }

    worker.process.sendInternal({ act: "port", seq: message.seq, port });
  }

  module.exports = cluster;
})();
