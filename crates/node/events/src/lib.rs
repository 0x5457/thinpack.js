use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:events", thinpack_js::js!("node-events.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/events/symbols",
            thinpack_js::js!("events_symbols.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_events",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
