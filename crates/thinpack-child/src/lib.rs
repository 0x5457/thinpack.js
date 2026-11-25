//! The spawned-process layer shared by `node:child_process` and `node:cluster`.
//!
//! Both modules own OS processes and their stdio pipes, and both expose the same four
//! facts about them: a pid, the output of `stdout`/`stderr`, an `exit` with a status or a
//! signal, and a `kill`. Node keeps that half in `lib/internal/child_process.js` and lets
//! `module:cluster` build its workers on top of `child_process`; Thinpack cannot do the
//! latter without linking the whole `child_process` capability into every cluster app, so
//! the shared half lives in this crate instead: one table, one set of pipe ops, and one
//! `Child` class the two JavaScript surfaces extend.
//!
//! What stays in the modules is what makes them different: `node:child_process` adds the
//! `spawn`/`exec`/`spawnSync` surface, and `node:cluster` adds the IPC channel and the
//! inherited descriptor a worker is started with.

mod error;
mod ops;
mod spawn;
mod table;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

pub use error::ChildError;
pub use spawn::{Completed, Exited, InheritedFd, Spawn, Spawned, Stdio, run_to_completion, spawn};
pub use table::{kill, pid};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:thinpack/internal/child_process",
        thinpack_js::js!("child_process.js"),
    )];

    Extension {
        name: "thinpack_child",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
