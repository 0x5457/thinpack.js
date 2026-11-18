use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

mod ops;

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:os",
        thinpack_js::js!("os.js"),
    )];

    Extension {
        name: "thinpack_node_os",
        deps: &["thinpack_node_internals", "thinpack_node_process"],
        ops: Cow::Borrowed(OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
