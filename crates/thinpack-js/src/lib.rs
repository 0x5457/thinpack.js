//! Compile-time brotli compression for the embedded JavaScript sources.
//!
//! The runtime keeps the Node.js sources in its own binary. As plain text they
//! are the largest incompressible-looking part of the template: 2.0 MB of
//! JavaScript shrinks to 0.4 MB with brotli, and the decoder is already linked
//! for the payload. [`js!`] replaces `include_str!` at the embed site, and
//! `deno_core` decompresses a module the first time it loads it.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use proc_macro::{Literal, TokenStream, TokenTree};

/// Brotli quality: the sources are compressed once, at build time, and decoded
/// on every start, but 0.4 MB against 0.5 MB at quality 9 is worth the decode.
const QUALITY: u32 = 11;
const WINDOW: u32 = 22;

/// Expands to the brotli-compressed bytes of `src/$file` of the calling crate, as
/// a `&'static [u8]`.
///
/// The path is resolved against the calling crate's `src/` directory
/// (`$CARGO_MANIFEST_DIR/src/$file`), so an embed site always names a file
/// relative to its own crate root; a `js!` call inside a nested module reads the
/// same file as one at the crate root.
///
/// The expansion also mentions the file through `include_str!`, so cargo rebuilds
/// the crate that embeds it when the JavaScript changes.
///
/// # Panics
///
/// Panics when `src/$file` is missing, is not UTF-8, or when the argument is not a
/// single plain string literal; a bad embed site is a build error either way.
#[proc_macro]
pub fn js(input: TokenStream) -> TokenStream {
    let path = string_literal(input);
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let source = fs::read(manifest.join("src").join(&path))
        .unwrap_or_else(|error| panic!("cannot read src/{path}: {error}"));
    // The source is JavaScript text, and the engine decodes it as UTF-8 when the module
    // loads. ASCII was enough for Node's `lib/`; undici's bundle carries an author name in a
    // licence header, and rewriting a vendored file to satisfy a build assertion is worse.
    assert!(
        std::str::from_utf8(&source).is_ok(),
        "{path}: embedded JavaScript must be UTF-8"
    );

    let compressed = compress(&source);
    let bytes = Literal::byte_string(&compressed);
    let tracking = Literal::string(&path);
    let mut expansion = String::with_capacity(64);
    write!(
        expansion,
        "{{ const _: &str = include_str!({tracking}); {bytes} }}"
    )
    .expect("writing to a string cannot fail");
    expansion.parse().expect("the expansion is valid Rust")
}

fn compress(source: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::with_capacity(source.len() / 4);
    {
        let mut writer = brotli::CompressorWriter::new(&mut compressed, 4096, QUALITY, WINDOW);
        writer
            .write_all(source)
            .expect("writing to an in-memory buffer cannot fail");
    }
    compressed
}

fn string_literal(input: TokenStream) -> String {
    let mut tokens = input.into_iter();
    let Some(TokenTree::Literal(literal)) = tokens.next() else {
        panic!("js! expects a string literal path");
    };
    assert!(
        tokens.next().is_none(),
        "js! takes exactly one string literal"
    );
    let text = literal.to_string();
    let path = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("js! expects a plain relative path, got {text}"));
    assert!(
        !path.contains('\\'),
        "js! expects a plain relative path, got {text}"
    );
    path.to_owned()
}
