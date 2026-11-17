use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "node:events",
            deno_core::ascii_str_include!("node-events.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/events/symbols",
            deno_core::ascii_str_include!("events_symbols.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_events",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
