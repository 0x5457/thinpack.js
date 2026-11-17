use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "node:assert",
            deno_core::ascii_str_include!("node-assert.js"),
        ),
        ExtensionFileSource::new(
            "node:assert/strict",
            deno_core::ascii_str_include!("node-assert-strict.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/assert/assertion_error",
            deno_core::ascii_str_include!("node-internal-assertion-error.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/assert/utils",
            deno_core::ascii_str_include!("node-internal-assert-utils.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/assert/myers_diff",
            deno_core::ascii_str_include!("node-internal-myers-diff.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/errors/error_source",
            deno_core::ascii_str_include!("error_source.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_assert",
        deps: &["thinpack_node_internals", "thinpack_node_util"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
