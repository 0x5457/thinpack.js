(() => {
  const { codes } = require("node:thinpack/internal/errors");
  const homedir = () => {
    const environment = process.platform === "win32"
      ? process.env.USERPROFILE
      : process.env.HOME;
    return environment || Deno.core.ops.op_os_homedir();
  };
  const constants = require("node:thinpack/internal/constants").os;

  const system = Deno.core.ops.op_os_system_info().split("\0");
  const cpuCount = Number(system[0]) || 1;
  const totalmem = Number(system[1]) || 0;
  const freemem = Number(system[2]) || 0;
  const release = system[3] || "unknown";
  const version = system[4] || "unknown";
  const hostname = system[5] || "localhost";
  const uptime = Number(system[6]) || 0;
  const loadavg = [system[7], system[8], system[9]].map(Number);

  const os = {
    arch: () => process.arch,
    platform: () => process.platform,
    type: () => ({
      win32: "Windows_NT",
      linux: "Linux",
      darwin: "Darwin",
      freebsd: "FreeBSD",
      openbsd: "OpenBSD",
      sunos: "SunOS",
    })[process.platform] ?? process.platform,
    release: () => release,
    version: () => version,
    hostname: () => hostname,
    homedir: () => homedir(),
    tmpdir: () => {
      let path = process.env.TMPDIR || process.env.TMP || process.env.TEMP;
      if (path && path.length > 1) {
        while (path.endsWith("/")) path = path.slice(0, -1);
      }
      return path || Deno.core.ops.op_os_tmpdir();
    },
    endianness: () =>
      new Uint8Array(new Uint16Array([1]).buffer)[0] === 1 ? "LE" : "BE",
    cpus: () => {
      const fields = Deno.core.ops.op_os_cpus().split("\0");
      const cpus = [];
      for (let index = 0; index + 6 < fields.length; index += 7) {
        cpus.push({
          model: fields[index],
          speed: Number(fields[index + 1]),
          times: {
            user: Number(fields[index + 2]),
            nice: Number(fields[index + 3]),
            sys: Number(fields[index + 4]),
            idle: Number(fields[index + 5]),
            irq: Number(fields[index + 6]),
          },
        });
      }
      return cpus;
    },
    totalmem: () => totalmem,
    freemem: () => freemem,
    uptime: () => uptime,
    loadavg: () => loadavg,
    userInfo: () => {
      // Windows has no uid/gid; Node reports -1 there (and no shell).
      const isWindows = process.platform === "win32";
      return {
        uid: isWindows ? -1 : process.getuid(),
        gid: isWindows ? -1 : process.getgid(),
        username: process.env.USER || "unknown",
        homedir: homedir(),
        shell: isWindows ? null : (process.env.SHELL || "/bin/sh"),
      };
    },
    networkInterfaces: () => {
      const result = {};
      const raw = Deno.core.ops.op_os_network_interfaces();
      for (const entry of raw.split("\n")) {
        if (entry === "") continue;
        const [name, address, netmask, family, mac, internal, cidr, scopeid] =
          entry.split("\0");
        const info = {
          address,
          netmask,
          family,
          mac,
          internal: internal === "1",
          cidr,
        };
        if (family === "IPv6") info.scopeid = Number(scopeid);
        if (result[name] === undefined) result[name] = [];
        result[name].push(info);
      }
      return result;
    },
    machine: () => ({ x64: "x86_64", arm64: "aarch64", ia32: "i386" })[process.arch] ?? process.arch,
    availableParallelism: () => cpuCount,
    constants,
    // `os.constants.priority` is not implemented; the priority accessors are
    // loud instead of reporting fabricated values.
    getPriority: () => {
      throw new codes.ERR_NOT_IMPLEMENTED("os.getPriority");
    },
    setPriority: () => {
      throw new codes.ERR_NOT_IMPLEMENTED("os.setPriority");
    },
  };

  Object.defineProperty(os, "EOL", {
    value: process.platform === "win32" ? "\r\n" : "\n",
    writable: false,
    configurable: true,
    enumerable: true,
  });
  module.exports = os;
})();
