module.exports = globalThis.process;

// Node's ESM facade exposes these as named exports. Assigning them onto the
// process object itself keeps the module identity (`module.exports === process`)
// while making the static names visible to the builtin facade generator.
module.exports.argv = process.argv;
module.exports.argv0 = process.argv0;
module.exports.execArgv = process.execArgv;
module.exports.execPath = process.execPath;
module.exports.exitCode = process.exitCode;
module.exports.env = process.env;
module.exports.platform = process.platform;
module.exports.pid = process.pid;
module.exports.arch = process.arch;
module.exports.version = process.version;
module.exports.versions = process.versions;
module.exports.release = process.release;
module.exports.config = process.config;
module.exports.title = process.title;
module.exports.umask = process.umask;
module.exports.chdir = process.chdir;
module.exports.cwd = process.cwd;
module.exports.exit = process.exit;
module.exports.kill = process.kill;
module.exports.nextTick = process.nextTick;
module.exports.stdout = process.stdout;
module.exports.stderr = process.stderr;
module.exports.stdin = process.stdin;
module.exports.hrtime = process.hrtime;
module.exports.uptime = process.uptime;
module.exports.memoryUsage = process.memoryUsage;
module.exports.cpuUsage = process.cpuUsage;
module.exports.resourceUsage = process.resourceUsage;
module.exports.features = process.features;
module.exports.emitWarning = process.emitWarning;
module.exports.on = process.on;
