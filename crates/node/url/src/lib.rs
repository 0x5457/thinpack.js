mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "thinpack:node/url",
        deno_core::ascii_str_include!("whatwg_url.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new("node:url", deno_core::ascii_str_include!("node-url.js")),
        ExtensionFileSource::new(
            "node:thinpack/internal/url_binding",
            deno_core::ascii_str_include!("url_binding.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/url_pattern",
            deno_core::ascii_str_include!("url_pattern.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/encoding_binding",
            deno_core::ascii_str_include!("encoding_binding.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_url",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(OPS),
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
