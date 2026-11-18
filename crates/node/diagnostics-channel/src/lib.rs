use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed(
            "node:diagnostics_channel",
            thinpack_js::js!("node-diagnostics-channel.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/diagnostics_channel",
            thinpack_js::js!("binding_diagnostics_channel.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/errors",
            thinpack_js::js!("binding_errors.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_diagnostics_channel",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
