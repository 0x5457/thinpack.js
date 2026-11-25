use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

mod ops;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:vm",
        thinpack_js::js!("vm.js"),
    )];

    Extension {
        name: "thinpack_node_vm",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(ops::OPS),
        op_state_fn: Some(Box::new(|state| {
            state.put(ops::Contexts::default());
        })),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
