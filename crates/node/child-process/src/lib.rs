use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

mod ops;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:child_process",
        thinpack_js::js!("child_process.js"),
    )];

    Extension {
        name: "thinpack_node_child_process",
        deps: &[
            "thinpack_node_internals",
            "thinpack_node_events",
            "thinpack_node_stream",
        ],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
