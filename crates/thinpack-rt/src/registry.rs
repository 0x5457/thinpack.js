#[rustfmt::skip]
#[allow(
    clippy::vec_init_then_push,
    reason = "one push per enabled capability mirrors the `cfg` list one to one"
)]
#[allow(
    clippy::missing_const_for_fn,
    reason = "the body is only const when every capability is compiled out"
)]
#[allow(
    unused_variables,
    reason = "only a per-isolate capability reads the inputs"
)]
pub(crate) fn extensions(isolate: &thinpack_core::IsolateInputs) -> Vec<deno_core::Extension> {
    #[allow(unused_mut, reason = "nothing is pushed when every capability is compiled out")]
    let mut extensions = Vec::new();
    #[cfg(any(feature = "node-child-process", feature = "node-cluster"))]
    extensions.push(thinpack_child::init());
    #[cfg(feature = "node-async-hooks")]
    extensions.push(thinpack_node_async_hooks::init());
    #[cfg(feature = "node-constants")]
    extensions.push(thinpack_node_constants::init());
    #[cfg(feature = "node-diagnostics-channel")]
    extensions.push(thinpack_node_diagnostics_channel::init());
    #[cfg(feature = "node-dns")]
    extensions.push(thinpack_node_dns::init());
    #[cfg(feature = "node-ffi")]
    extensions.extend(thinpack_node_ffi::init(isolate));
    #[cfg(feature = "node-inspector")]
    extensions.push(thinpack_node_inspector::init());
    #[cfg(feature = "node-perf-hooks")]
    extensions.push(thinpack_node_perf_hooks::init());
    #[cfg(feature = "node-process")]
    extensions.push(thinpack_node_process::init());
    #[cfg(feature = "node-sqlite")]
    extensions.push(thinpack_node_sqlite::init());
    #[cfg(feature = "node-timers")]
    extensions.push(thinpack_node_timers::init());
    #[cfg(feature = "node-util")]
    extensions.push(thinpack_node_util::init());
    #[cfg(feature = "node-vm")]
    extensions.push(thinpack_node_vm::init());
    #[cfg(feature = "node-assert")]
    extensions.push(thinpack_node_assert::init());
    #[cfg(feature = "node-buffer")]
    extensions.push(thinpack_node_buffer::init());
    #[cfg(feature = "node-console")]
    extensions.push(thinpack_node_console::init());
    #[cfg(feature = "node-events")]
    extensions.push(thinpack_node_events::init());
    #[cfg(feature = "node-os")]
    extensions.push(thinpack_node_os::init());
    #[cfg(feature = "node-path")]
    extensions.push(thinpack_node_path::init());
    #[cfg(feature = "node-punycode")]
    extensions.push(thinpack_node_punycode::init());
    #[cfg(feature = "node-test")]
    extensions.push(thinpack_node_test::init());
    #[cfg(feature = "node-tty")]
    extensions.push(thinpack_node_tty::init());
    #[cfg(feature = "node-domain")]
    extensions.push(thinpack_node_domain::init());
    #[cfg(feature = "node-module")]
    extensions.push(thinpack_node_module::init());
    #[cfg(feature = "node-querystring")]
    extensions.push(thinpack_node_querystring::init());
    #[cfg(feature = "node-readline")]
    extensions.push(thinpack_node_readline::init());
    #[cfg(feature = "node-sea")]
    extensions.push(thinpack_node_sea::init());
    #[cfg(feature = "node-string-decoder")]
    extensions.push(thinpack_node_string_decoder::init());
    #[cfg(feature = "node-v8")]
    extensions.push(thinpack_node_v8::init());
    #[cfg(feature = "node-worker-threads")]
    extensions.push(thinpack_node_worker_threads::init());
    #[cfg(feature = "node-stream")]
    extensions.push(thinpack_node_stream::init());
    #[cfg(feature = "node-url")]
    extensions.push(thinpack_node_url::init());
    #[cfg(feature = "node-child-process")]
    extensions.push(thinpack_node_child_process::init());
    #[cfg(feature = "node-crypto")]
    extensions.push(thinpack_node_crypto::init());
    #[cfg(feature = "node-fs")]
    extensions.extend(thinpack_node_fs::init(isolate));
    #[cfg(feature = "node-net")]
    extensions.push(thinpack_node_net::init());
    #[cfg(feature = "node-zlib")]
    extensions.push(thinpack_node_zlib::init());
    #[cfg(feature = "node-cluster")]
    extensions.push(thinpack_node_cluster::init());
    #[cfg(feature = "node-http")]
    extensions.push(thinpack_node_http::init());
    #[cfg(feature = "node-tls")]
    extensions.push(thinpack_node_tls::init());
    #[cfg(feature = "node-fetch")]
    extensions.push(thinpack_node_fetch::init());
    #[cfg(feature = "node-http2")]
    extensions.push(thinpack_node_http2::init());
    #[cfg(feature = "node-https")]
    extensions.push(thinpack_node_https::init());
    extensions
}

#[rustfmt::skip]
#[allow(
    clippy::vec_init_then_push,
    reason = "one push per enabled capability mirrors the `cfg` list one to one"
)]
#[allow(
    clippy::missing_const_for_fn,
    reason = "the body is only const when every capability is compiled out"
)]
pub(crate) fn enabled() -> Vec<&'static str> {
    #[allow(unused_mut, reason = "nothing is pushed when every capability is compiled out")]
    let mut names = Vec::new();
    #[cfg(feature = "node-async-hooks")]
    names.push("async_hooks");
    #[cfg(feature = "node-constants")]
    names.push("constants");
    #[cfg(feature = "node-diagnostics-channel")]
    names.push("diagnostics_channel");
    #[cfg(feature = "node-dns")]
    names.push("dns");
    #[cfg(feature = "node-ffi")]
    names.push("ffi");
    #[cfg(feature = "node-inspector")]
    names.push("inspector");
    #[cfg(feature = "node-perf-hooks")]
    names.push("perf_hooks");
    #[cfg(feature = "node-process")]
    names.push("process");
    #[cfg(feature = "node-sqlite")]
    names.push("sqlite");
    #[cfg(feature = "node-timers")]
    names.push("timers");
    #[cfg(feature = "node-util")]
    names.push("util");
    #[cfg(feature = "node-vm")]
    names.push("vm");
    #[cfg(feature = "node-assert")]
    names.push("assert");
    #[cfg(feature = "node-buffer")]
    names.push("buffer");
    #[cfg(feature = "node-console")]
    names.push("console");
    #[cfg(feature = "node-events")]
    names.push("events");
    #[cfg(feature = "node-os")]
    names.push("os");
    #[cfg(feature = "node-path")]
    names.push("path");
    #[cfg(feature = "node-punycode")]
    names.push("punycode");
    #[cfg(feature = "node-test")]
    names.push("test");
    #[cfg(feature = "node-tty")]
    names.push("tty");
    #[cfg(feature = "node-domain")]
    names.push("domain");
    #[cfg(feature = "node-module")]
    names.push("module");
    #[cfg(feature = "node-querystring")]
    names.push("querystring");
    #[cfg(feature = "node-readline")]
    names.push("readline");
    #[cfg(feature = "node-sea")]
    names.push("sea");
    #[cfg(feature = "node-string-decoder")]
    names.push("string_decoder");
    #[cfg(feature = "node-v8")]
    names.push("v8");
    #[cfg(feature = "node-worker-threads")]
    names.push("worker_threads");
    #[cfg(feature = "node-stream")]
    names.push("stream");
    #[cfg(feature = "node-url")]
    names.push("url");
    #[cfg(feature = "node-child-process")]
    names.push("child_process");
    #[cfg(feature = "node-crypto")]
    names.push("crypto");
    #[cfg(feature = "node-fs")]
    names.push("fs");
    #[cfg(feature = "node-net")]
    names.push("net");
    #[cfg(feature = "node-zlib")]
    names.push("zlib");
    #[cfg(feature = "node-cluster")]
    names.push("cluster");
    #[cfg(feature = "node-http")]
    names.push("http");
    #[cfg(feature = "node-tls")]
    names.push("tls");
    #[cfg(feature = "node-fetch")]
    names.push("fetch");
    #[cfg(feature = "node-http2")]
    names.push("http2");
    #[cfg(feature = "node-https")]
    names.push("https");
    names
}
