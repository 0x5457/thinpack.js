use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "thinpack:node/fetch-globals",
        thinpack_js::js!("fetch_globals.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:thinpack/internal/undici",
        thinpack_js::js!("vendor/undici.js"),
    )];

    Extension {
        name: "thinpack_node_fetch",
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
