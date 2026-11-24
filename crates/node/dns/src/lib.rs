mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:dns", thinpack_js::js!("dns.js")),
        ExtensionFileSource::new(
            "node:dns/promises",
            deno_core::ascii_str!("module.exports = require('node:dns').promises;\n"),
        ),
    ];

    Extension {
        name: "thinpack_node_dns",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(ops::OPS),
        op_state_fn: Some(Box::new(|state| state.put(ops::Resolvers::default()))),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
