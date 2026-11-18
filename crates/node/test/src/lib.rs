use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:test", thinpack_js::js!("node-test.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/test_runner",
            thinpack_js::js!("node-test-runner.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_test",
        deps: &["thinpack_node_async_hooks", "thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
