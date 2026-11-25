use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

pub(crate) fn init() -> Extension {
    const JS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed(
            "thinpack:node-dom-exception",
            thinpack_js::js!("bootstrap/dom_exception.js"),
        ),
        ExtensionFileSource::new_compressed(
            "thinpack:node-structured-clone",
            thinpack_js::js!("bootstrap/structured_clone.js"),
        ),
        ExtensionFileSource::new_compressed(
            "thinpack:node-globals",
            thinpack_js::js!("bootstrap/node_globals.js"),
        ),
        ExtensionFileSource::new_compressed(
            "thinpack:node-report-error",
            thinpack_js::js!("bootstrap/report_error.js"),
        ),
        ExtensionFileSource::new_compressed(
            "thinpack:node-event-globals",
            thinpack_js::js!("bootstrap/event_globals.js"),
        ),
        ExtensionFileSource::new_compressed(
            "thinpack:node-encoding-globals",
            thinpack_js::js!("bootstrap/encoding_globals.js"),
        ),
        ExtensionFileSource::new_compressed(
            "thinpack:node-intl-fallback",
            thinpack_js::js!("bootstrap/intl_fallback.js"),
        ),
        ExtensionFileSource::new_compressed(
            "thinpack:node-abort",
            thinpack_js::js!("bootstrap/abort_controller.js"),
        ),
    ];

    Extension {
        name: "thinpack_bootstrap",
        js_files: Cow::Borrowed(JS),
        ..Default::default()
    }
}
