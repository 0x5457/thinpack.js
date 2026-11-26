//! The default `.js` loader a wrapper keeps and delegates to. Both the unpacked
//! runtime and a packed executable run the same fixture, so only the payload read
//! behind `Module._extensions` differs.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use super::super::{Cli, Outcome};
use super::{TempDir, build_packed_payload, run_command};

pub(super) const NAME: &str = "packed::require-extensions-delegate";
const DIRECTORY: &str = "require-extensions-delegate";
const ENTRY: &str = "/app/main.js";
const EXPECTED: &str = "widget ok";

const FILES: &[(&str, &str)] = &[
    (
        ENTRY,
        r"
const Module = require('node:module');

// The contract a wrapper such as pirates exposes: keep the loader it replaced,
// swap `_compile` so the hook sees the source the loader read, and delegate.
const original = Module._extensions['.js'];
if (typeof original !== 'function') throw new Error('no default .js loader');
let compiled = 0;
let transformed = 0;
Module._extensions['.js'] = function wrapper(module, filename) {
  const compile = module._compile;
  module._compile = function (source) {
    module._compile = compile;
    compiled += 1;
    if (filename.endsWith('widget.js')) {
      transformed += 1;
      source = source.replace('this.name = name;', 'this.name = name.toUpperCase();');
    }
    return module._compile(source, filename);
  };
  original(module, filename);
};

const Widget = require('./widget.js');
if (compiled !== 1 || transformed !== 1) throw new Error('hook did not transform widget.js');
if (typeof Widget !== 'function') throw new Error(`expected a class, got ${typeof Widget}`);
const widget = new Widget('delegated');
if (widget.name !== 'DELEGATED') throw new Error('unexpected widget');
if (require('./widget.js') !== Widget) throw new Error('require cache lost the export');

// A file the hook does not transform still reaches the loader through its
// original `_compile`.
const Plain = require('./plain.js');
if (compiled !== 2 || transformed !== 1) throw new Error('passthrough did not use _compile');
if (new Plain('plain').name !== 'plain') throw new Error('passthrough changed the export');

// A handler that builds the exports itself never delegates, and the record it
// produced is cached like any other load.
let ownCalls = 0;
Module._extensions['.js'] = function own(module) {
  ownCalls += 1;
  module.exports = 'from handler';
};
if (require('./own.js') !== 'from handler') throw new Error('handler export lost');
if (require('./own.js') !== 'from handler' || ownCalls !== 1) {
  throw new Error('own exports were not cached');
}

// A handler that throws before delegating leaves no cached record; the next
// require retries through the loader it kept.
let attempts = 0;
Module._extensions['.js'] = function failing(module, filename) {
  attempts += 1;
  if (attempts === 1) throw new Error('handler failed');
  original(module, filename);
};
try {
  require('./retry.js');
  throw new Error('handler failure did not propagate');
} catch (error) {
  if (error.message !== 'handler failed') throw error;
}
if (require('./retry.js') !== 'from file' || attempts !== 2) {
  throw new Error('retry did not delegate');
}
console.log('widget ok');
",
    ),
    (
        "/app/widget.js",
        "module.exports = class Widget {\n  constructor(name) {\n    this.name = name;\n  }\n};\n",
    ),
    (
        "/app/plain.js",
        "#!/usr/bin/env node\nmodule.exports = class Plain {\n  constructor(name) {\n    this.name = name;\n  }\n};\n",
    ),
    ("/app/own.js", "module.exports = 'from file';\n"),
    ("/app/retry.js", "module.exports = 'from file';\n"),
];

pub(super) fn print_list(cli: &Cli) {
    if cli.matches(NAME) {
        println!("{NAME}");
    }
}

pub(super) fn matches(cli: &Cli) -> bool {
    cli.matches(NAME)
}

pub(super) fn run_test() -> Outcome {
    let directory = match TempDir::new(DIRECTORY) {
        Ok(directory) => directory,
        Err(error) => {
            return Outcome::Failed(format!("cannot create test directory: {error}"));
        }
    };
    if let Err(error) = write_sources(directory.path()) {
        return Outcome::Failed(format!("cannot write unpacked fixture: {error}"));
    }

    let mut command = Command::new(env!("CARGO_BIN_EXE_thinpack-rt"));
    command.current_dir(directory.path()).arg("main.js");
    if let Outcome::Failed(report) = run_command(command, None, Some(EXPECTED), None) {
        return Outcome::Failed(format!("unpacked runtime:\n{report}"));
    }

    match build_packed_payload(ENTRY, FILES, &["module"], directory.path()) {
        Ok(binary) => super::run_binary(&binary, directory.path(), None, Some(EXPECTED)),
        Err(error) => Outcome::Failed(format!("cannot build packed test executable: {error}")),
    }
}

fn write_sources(directory: &Path) -> io::Result<()> {
    for &(path, source) in FILES {
        let relative = path
            .strip_prefix("/app/")
            .expect("packed fixture paths are rooted at /app");
        fs::write(directory.join(relative), source)?;
    }
    Ok(())
}
