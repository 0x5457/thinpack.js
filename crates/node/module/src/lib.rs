use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

// Feature input: module.js exposes the loader hook bridge.
use thinpack_loader as _;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:module",
        thinpack_js::js!("module.js"),
    )];

    Extension {
        name: "thinpack_node_module",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
