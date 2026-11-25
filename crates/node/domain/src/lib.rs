use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:domain",
        thinpack_js::js!("domain.js"),
    )];

    Extension {
        name: "thinpack_node_domain",
        deps: &["thinpack_node_internals", "thinpack_node_events"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
