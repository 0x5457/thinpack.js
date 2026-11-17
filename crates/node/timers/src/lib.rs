use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "thinpack:node/timers",
        deno_core::ascii_str_include!("timers.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "node:timers",
            deno_core::ascii_str!(
                "const timers = globalThis.__thinpack_timers;\n\
                 module.exports = {\n\
                 \x20 setTimeout: timers.setTimeout,\n\
                 \x20 clearTimeout: timers.clearTimeout,\n\
                 \x20 setInterval: timers.setInterval,\n\
                 \x20 clearInterval: timers.clearInterval,\n\
                 \x20 setImmediate: timers.setImmediate,\n\
                 \x20 clearImmediate: timers.clearImmediate,\n\
                 };\n"
            ),
        ),
        ExtensionFileSource::new(
            "node:timers/promises",
            deno_core::ascii_str_include!("promises.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_timers",
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
