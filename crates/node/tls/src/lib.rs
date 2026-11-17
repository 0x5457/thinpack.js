mod ops;

pub use ops::{AsyncReadWrite, ServerAcceptor, TakeError, take_connection};

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:tls",
        deno_core::ascii_str_include!("tls.js"),
    )];

    Extension {
        name: "thinpack_node_tls",
        deps: &["thinpack_node_internals", "thinpack_node_stream"],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
