//! Ops backing the buffer binding.
//!
//! Node performs the UTF-8 conversions in C++ (`node_buffer.cc` and simdutf).
//! Doing them in JS costs a `charCodeAt` per code unit and a typed-array store
//! per byte, which is the slowest path left in `Buffer`; the remaining encodings
//! stay in `binding_buffer.js` because they are not on the hot path.
//!
//! The strings cross the boundary through `utf8_length`/`write_utf8_v2` rather
//! than the `#[string]` argument conversion: the latter decodes `QuickJS` WTF-8
//! as UTF-8, which turns one unpaired surrogate into three U+FFFD. `write_utf8_v2`
//! replaces it once, and a replacement is three bytes, so the length `utf8_length`
//! reports still matches what the encode writes.

// `op2` collects every attribute on an op fn as an op attribute, so lint
// attributes cannot be attached to the ops below.
#![expect(
    clippy::needless_pass_by_ref_mut,
    reason = "`&mut v8::PinScope` is the argument shape op2 hands the op; these conversions only read it"
)]

use deno_core::v8;
use deno_core::{OpDecl, op2};

pub(crate) const OPS: &[OpDecl] = &[
    op_binding_utf8_length(),
    op_binding_utf8_slice(),
    op_binding_utf8_write(),
];

/// Bytes `value` occupies once encoded, counting unpaired surrogates as U+FFFD.
#[op2(nofast)]
#[number]
pub(crate) fn op_binding_utf8_length<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::String>,
) -> usize {
    value.utf8_length(scope)
}

/// Encodes `value` into `target[offset..]`, never past `length` bytes, and
/// returns the bytes written. A character that does not fit is not split.
#[op2(nofast)]
#[number]
pub(crate) fn op_binding_utf8_write<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    #[buffer] target: &mut [u8],
    #[number] offset: usize,
    #[number] length: usize,
    value: v8::Local<'s, v8::String>,
) -> usize {
    let start = offset.min(target.len());
    let room = length.min(target.len() - start);
    value.write_utf8_v2(
        scope,
        &mut target[start..start + room],
        v8::WriteFlags::kReplaceInvalidUtf8,
        None,
    )
}

/// Decodes `bytes[start..end]` as UTF-8, replacing each malformed sequence with
/// U+FFFD the way `Buffer#toString` does.
#[op2]
#[string]
pub(crate) fn op_binding_utf8_slice(
    #[buffer] bytes: &[u8],
    #[number] start: usize,
    #[number] end: usize,
) -> String {
    let start = start.min(bytes.len());
    let end = end.clamp(start, bytes.len());
    String::from_utf8_lossy(&bytes[start..end]).into_owned()
}
