mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "thinpack:node/url",
        thinpack_js::js!("whatwg_url.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:url", thinpack_js::js!("node-url.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/url_binding",
            thinpack_js::js!("url_binding.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/url_pattern",
            thinpack_js::js!("url_pattern.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/encoding_binding",
            thinpack_js::js!("encoding_binding.js"),
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
