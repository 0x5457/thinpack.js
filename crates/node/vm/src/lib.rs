use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:vm",
        deno_core::ascii_str_include!("vm.js"),
    )];

    Extension {
        name: "thinpack_node_vm",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
