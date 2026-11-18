mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:sea", thinpack_js::js!("node-sea.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/sea",
            thinpack_js::js!("binding_sea.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_sea",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
