use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "thinpack:node/console",
        thinpack_js::js!("console.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:console",
        deno_core::ascii_str!(
            "module.exports = globalThis.console;\nmodule.exports.Console = globalThis.console.Console;\n"
        ),
    )];

    Extension {
        name: "thinpack_node_console",
        deps: &["thinpack_io"],
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
