use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

pub(crate) fn init() -> Extension {
    const JS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "thinpack:node-dom-exception",
            deno_core::ascii_str_include!("bootstrap/dom_exception.js"),
        ),
        ExtensionFileSource::new(
            "thinpack:node-structured-clone",
            deno_core::ascii_str_include!("bootstrap/structured_clone.js"),
        ),
        ExtensionFileSource::new(
            "thinpack:node-globals",
            deno_core::ascii_str_include!("bootstrap/node_globals.js"),
        ),
        ExtensionFileSource::new(
            "thinpack:node-event-globals",
            deno_core::ascii_str_include!("bootstrap/event_globals.js"),
        ),
        ExtensionFileSource::new(
            "thinpack:node-encoding-globals",
            deno_core::ascii_str_include!("bootstrap/encoding_globals.js"),
        ),
        ExtensionFileSource::new(
            "thinpack:node-intl-fallback",
            deno_core::ascii_str_include!("bootstrap/intl_fallback.js"),
        ),
        ExtensionFileSource::new(
            "thinpack:node-abort",
            deno_core::ascii_str_include!("bootstrap/abort_controller.js"),
        ),
    ];

    Extension {
        name: "thinpack_bootstrap",
        js_files: Cow::Borrowed(JS),
        ..Default::default()
    }
}
