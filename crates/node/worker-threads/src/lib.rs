mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "thinpack:node-worker-globals",
        thinpack_js::js!("worker_globals.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:worker_threads",
        thinpack_js::js!("worker_threads.js"),
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
