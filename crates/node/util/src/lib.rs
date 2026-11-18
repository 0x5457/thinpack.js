use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:util", thinpack_js::js!("node-util.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/util/inspect/impl",
            thinpack_js::js!("node-internal-util-inspect.js"),
        ),
        ExtensionFileSource::new(
            "node:util/types",
            deno_core::ascii_str!(
                "module.exports = require('node:thinpack/internal/util/types');\n"
            ),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/mime",
            thinpack_js::js!("mime.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/util/parse_args/parse_args",
            thinpack_js::js!("parse_args.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/util/parse_args/utils",
            thinpack_js::js!("parse_args_utils.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/util/throttle",
            thinpack_js::js!("throttle.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/util/debounce",
            thinpack_js::js!("debounce.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/util/diff",
            thinpack_js::js!("util_diff.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/util/trace_sigint",
            thinpack_js::js!("util_trace_sigint.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/bootstrap/realm",
            thinpack_js::js!("bootstrap_realm.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/timers",
            thinpack_js::js!("internal_timers.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/watchdog",
            thinpack_js::js!("watchdog.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/timers",
            thinpack_js::js!("binding_timers.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_util",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
