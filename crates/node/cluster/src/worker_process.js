(() => {
  const { Child } = require("node:thinpack/internal/child_process");
  const channel = require("node:thinpack/internal/child_process/channel");

  // The primary's view of one worker process: the shared `Child` half (pid, piped stdio,
  // exit) plus the IPC channel that makes it a cluster worker.
  class WorkerProcess extends Child {
    constructor(id, pid, channelId, silent) {
      super();
      this.spawnfile = process.execPath;
      this.spawnargs = [process.execPath];
      this._attach(id, pid, { stdin: silent, stdout: silent, stderr: silent });
      // The channel handshake runs after the worker connects; on unix it resolves at once.
      // Reads and writes wait on it, while `cluster.fork` keeps its synchronous return
      // because construction never blocks.
      channel.attach(this, channelId, {
        connect: Deno.core.ops.op_child_channel_accept(channelId, id),
      });
    }
  }

  module.exports = WorkerProcess;
})();
