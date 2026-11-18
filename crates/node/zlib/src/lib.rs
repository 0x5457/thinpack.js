mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:zlib",
        thinpack_js::js!("zlib.js"),
    )];

    Extension {
        name: "thinpack_node_zlib",
        deps: &[
            "thinpack_node_internals",
            "thinpack_node_buffer",
            "thinpack_node_stream",
        ],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
