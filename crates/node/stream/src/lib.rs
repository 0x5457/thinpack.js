use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
    "thinpack:node-stream-globals",
    thinpack_js::js!("stream_globals.js"),
)];

const BUILTINS: &[ExtensionFileSource] = &[
    ExtensionFileSource::new_compressed("node:stream", thinpack_js::js!("node-stream.js")),
    ExtensionFileSource::new_compressed("node:stream/consumers", thinpack_js::js!("consumers.js")),
    ExtensionFileSource::new_compressed(
        "node:stream/promises",
        thinpack_js::js!("node-stream-promises.js"),
    ),
    ExtensionFileSource::new_compressed("node:stream/web", thinpack_js::js!("node-stream-web.js")),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/legacy",
        thinpack_js::js!("internal/legacy.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/readable",
        thinpack_js::js!("internal/readable.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/writable",
        thinpack_js::js!("internal/writable.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/duplex",
        thinpack_js::js!("internal/duplex.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/transform",
        thinpack_js::js!("internal/transform.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/passthrough",
        thinpack_js::js!("internal/passthrough.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/pipeline",
        thinpack_js::js!("internal/pipeline.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/destroy",
        thinpack_js::js!("internal/destroy.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/duplexify",
        thinpack_js::js!("internal/duplexify.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/from",
        thinpack_js::js!("internal/from.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/compose",
        thinpack_js::js!("internal/compose.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/operators",
        thinpack_js::js!("internal/operators.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/end-of-stream",
        thinpack_js::js!("internal/end-of-stream.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/state",
        thinpack_js::js!("internal/state.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/lazy_transform",
        thinpack_js::js!("internal/lazy_transform.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/duplexpair",
        thinpack_js::js!("internal/duplexpair.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/stream_wrap",
        thinpack_js::js!("binding_stream_wrap.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/worker/io",
        thinpack_js::js!("internal/worker_io.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/readablestream",
        thinpack_js::js!("internal/webstreams/readablestream.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/writablestream",
        thinpack_js::js!("internal/webstreams/writablestream.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/transformstream",
        thinpack_js::js!("internal/webstreams/transformstream.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/queuingstrategies",
        thinpack_js::js!("internal/webstreams/queuingstrategies.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/adapters",
        thinpack_js::js!("internal/webstreams/adapters.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/util",
        thinpack_js::js!("internal/webstreams/util.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/transfer",
        thinpack_js::js!("internal/webstreams/transfer.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/compression",
        thinpack_js::js!("internal/webstreams/compression.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/webstreams/encoding",
        thinpack_js::js!("internal/webstreams/encoding.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/add-abort-signal",
        thinpack_js::js!("internal/add-abort-signal.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/async_context_frame",
        thinpack_js::js!("async_context_frame.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/uv",
        thinpack_js::js!("binding_uv.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/messaging",
        thinpack_js::js!("binding_messaging.js"),
    ),
];

#[must_use]
pub fn init() -> Extension {
    Extension {
        name: "thinpack_node_stream",
        deps: &[
            "thinpack_node_internals",
            "thinpack_node_buffer",
            "thinpack_node_events",
            "thinpack_node_string_decoder",
            "thinpack_node_util",
        ],
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
