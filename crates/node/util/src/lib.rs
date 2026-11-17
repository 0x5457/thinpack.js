use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new("node:util", deno_core::ascii_str_include!("node-util.js")),
        ExtensionFileSource::new(
            "node:thinpack/internal/util/inspect/impl",
            deno_core::ascii_str_include!("node-internal-util-inspect.js"),
        ),
        ExtensionFileSource::new(
            "node:util/types",
            deno_core::ascii_str!(
                "module.exports = require('node:thinpack/internal/util/types');\n"
            ),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/mime",
            deno_core::ascii_str_include!("mime.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/util/parse_args/parse_args",
            deno_core::ascii_str_include!("parse_args.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/util/parse_args/utils",
            deno_core::ascii_str_include!("parse_args_utils.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/util/throttle",
            deno_core::ascii_str_include!("throttle.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/util/debounce",
            deno_core::ascii_str_include!("debounce.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/util/diff",
            deno_core::ascii_str_include!("util_diff.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/util/trace_sigint",
            deno_core::ascii_str_include!("util_trace_sigint.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/bootstrap/realm",
            deno_core::ascii_str_include!("bootstrap_realm.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/timers",
            deno_core::ascii_str_include!("internal_timers.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/watchdog",
            deno_core::ascii_str_include!("watchdog.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/timers",
            deno_core::ascii_str_include!("binding_timers.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_util",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
