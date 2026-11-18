use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:assert", thinpack_js::js!("node-assert.js")),
        ExtensionFileSource::new_compressed(
            "node:assert/strict",
            thinpack_js::js!("node-assert-strict.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/assert/assertion_error",
            thinpack_js::js!("node-internal-assertion-error.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/assert/utils",
            thinpack_js::js!("node-internal-assert-utils.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/assert/myers_diff",
            thinpack_js::js!("node-internal-myers-diff.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/errors/error_source",
            thinpack_js::js!("error_source.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_assert",
        deps: &["thinpack_node_internals", "thinpack_node_util"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
