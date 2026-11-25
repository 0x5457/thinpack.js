mod events;
mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:http2",
        thinpack_js::js!("http2.js"),
    )];

    Extension {
        name: "thinpack_node_http2",
        deps: &[
            "thinpack_node_internals",
            "thinpack_node_stream",
            "thinpack_node_net",
            "thinpack_node_tls",
        ],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
