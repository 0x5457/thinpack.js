use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "node:diagnostics_channel",
            deno_core::ascii_str_include!("node-diagnostics-channel.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/diagnostics_channel",
            deno_core::ascii_str_include!("binding_diagnostics_channel.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/errors",
            deno_core::ascii_str_include!("binding_errors.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_diagnostics_channel",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
