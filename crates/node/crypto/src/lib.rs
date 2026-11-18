// `cipher` is not named directly: the dependency exists to enable its `alloc` and
// `block-padding` features for the `aes`, `cbc`, and `ctr` re-exports.
#![allow(
    unused_crate_dependencies,
    reason = "the direct `cipher` dependency only enables features used through `aes`/`cbc`/`ctr`"
)]

mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "thinpack:node-crypto-globals",
        thinpack_js::js!("crypto_globals.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:crypto",
        thinpack_js::js!("crypto.js"),
    )];

    Extension {
        name: "thinpack_node_crypto",
        deps: &["thinpack_node_internals", "thinpack_node_buffer"],
        js_files: Cow::Borrowed(STARTUP),
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
