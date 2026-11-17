mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

pub use ops::{TakeStreamError, take_stream};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:net",
        deno_core::ascii_str_include!("net.js"),
    )];

    Extension {
        name: "thinpack_node_net",
        deps: &["thinpack_node_internals", "thinpack_node_stream"],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
