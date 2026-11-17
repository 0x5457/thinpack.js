mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "thinpack:node-worker-globals",
        deno_core::ascii_str_include!("worker_globals.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:worker_threads",
        deno_core::ascii_str_include!("worker_threads.js"),
    )];

    Extension {
        name: "thinpack_node_worker_threads",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(ops::OPS),
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
