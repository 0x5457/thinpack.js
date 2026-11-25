mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:dgram",
        thinpack_js::js!("dgram.js"),
    )];

    Extension {
        name: "thinpack_node_dgram",
        deps: &["thinpack_node_internals", "thinpack_node_events"],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
