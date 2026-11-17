use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:querystring",
        deno_core::ascii_str_include!("node-querystring.js"),
    )];

    Extension {
        name: "thinpack_node_querystring",
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
