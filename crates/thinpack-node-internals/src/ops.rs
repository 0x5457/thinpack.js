//! Ops backing the buffer binding.
//!
//! Node implements these primitives in C++ (`node_buffer.cc` for the encodings,
//! `CHECK`-backed loops elsewhere). In JS each of them costs one `charCodeAt` or
//! one typed-array store per unit, which on `QuickJS` is two orders of magnitude
//! slower than the same loop in Rust: encoding a 200 `KiB` buffer to hex took
//! 50 ms against Node's 0.1 ms, and `Buffer.compare` took 14 ms against 0.05 ms.
//!
//! What stays in JS is whatever needs the raw `UTF-16` code units of a string:
//! `latin1`/`ascii` writes mask each code unit with `& 0xff`, `ucs2` reads and
//! writes code units verbatim, and a JS string may hold unpaired surrogates that
//! a Rust `String` cannot. The `QuickJS` `UTF-16` entry points normalise those to
//! `U+FFFD`, which is not the same thing.
//!
//! The UTF-8 three take a `v8::Local<v8::String>` rather than `#[string]`: the
//! argument conversion decodes the `QuickJS` `WTF-8` bytes as UTF-8, which turns
//! one unpaired surrogate into three `U+FFFD`. `write_utf8_v2` replaces it once,
//! and a replacement is three bytes, so the length `utf8_length` reports still
//! matches what the encode writes.

// `op2` parses every attribute on an op fn as an op attribute: unknown ones are a
// compile error and `allow` is dropped, so a lint attribute has to sit at module
// scope.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "a buffer index fits in an `i32`: a larger buffer cannot be allocated, and Node reports these as JS numbers"
)]
#![expect(
    clippy::needless_pass_by_ref_mut,
    reason = "`&mut v8::PinScope` is the argument shape op2 hands the op; these conversions only read it"
)]

use std::cmp::Ordering;

use deno_core::v8;
use deno_core::{OpDecl, op2};

pub(crate) const OPS: &[OpDecl] = &[
    op_binding_ascii_slice(),
    op_binding_base64_decode(),
    op_binding_base64_slice(),
    op_binding_compare(),
    op_binding_get_own_non_index_properties(),
    op_binding_hex_decode(),
    op_binding_hex_slice(),
    op_binding_index_of(),
    op_binding_index_of_byte(),
    op_binding_is_latin1(),
    op_binding_is_ascii(),
    op_binding_is_utf8(),
    op_binding_latin1_slice(),
    op_binding_latin1_write(),
    op_binding_string_length_utf8(),
    op_binding_swap(),
    op_binding_utf8_length(),
    op_binding_utf8_units(),
    op_binding_utf8_slice(),
    op_binding_utf8_write(),
];

/// `Buffer#write` with `latin1` or `ascii`: Node writes the low byte of every
/// UTF-16 code unit, so `\u20ac` becomes `0xac`. The op only sees strings the JS
/// side checked with `String#isWellFormed`: on `QuickJS` the units are recovered
/// from the string's UTF-8 bytes, which round-trip exactly for those, while an
/// unpaired surrogate would come back as U+FFFD.
#[op2(nofast)]
#[number]
pub(crate) fn op_binding_latin1_write<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    #[buffer] target: &mut [u8],
    #[number] offset: usize,
    #[number] max_bytes: usize,
    value: v8::Local<'s, v8::String>,
) -> usize {
    let start = offset.min(target.len());
    let written = max_bytes.min(target.len() - start).min(value.length());
    value.write_one_byte_v2(
        scope,
        0,
        &mut target[start..start + written],
        v8::WriteFlags::empty(),
    );
    written
}

/// `util.getOwnNonIndexProperties`: own property keys with the array indices
/// dropped. The engine skips them while collecting, so a typed-array view costs
/// what an empty object costs; the JS version enumerated every index as a string
/// and then called `Number()`/`String()` on it to drop it again.
#[op2]
pub(crate) fn op_binding_get_own_non_index_properties<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<'s, v8::Object>,
    #[smi] filter: u32,
) -> v8::Local<'s, v8::Value> {
    let mut property_filter = v8::PropertyFilter::ALL_PROPERTIES;
    if filter & ENUMERABLE_ONLY != 0 {
        property_filter = property_filter | v8::PropertyFilter::ONLY_ENUMERABLE;
    }
    if filter & SYMBOLS_SKIPPED != 0 {
        property_filter = property_filter | v8::PropertyFilter::SKIP_SYMBOLS;
    }
    let args = v8::GetPropertyNamesArgs {
        mode: v8::KeyCollectionMode::OwnOnly,
        property_filter,
        index_filter: v8::IndexFilter::SkipIndices,
        key_conversion: v8::KeyConversionMode::ConvertToString,
    };
    object
        .get_property_names(scope, args)
        .map_or_else(|| v8::undefined(scope).into(), v8::Local::into)
}

/// The `util` binding's own filter bits (`node-internal-util.js` exports them as
/// `constants`); they are not the engine's `PropertyFilter` values.
const ENUMERABLE_ONLY: u32 = 2;
const SYMBOLS_SKIPPED: u32 = 8;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn clamp_range(len: usize, start: usize, end: usize) -> (usize, usize) {
    let start = start.min(len);
    (start, end.clamp(start, len))
}

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
    let (start, end) = clamp_range(bytes.len(), start, end);
    String::from_utf8_lossy(&bytes[start..end]).into_owned()
}

/// `Buffer.compare` ordering: the first differing byte decides, then the length.
#[op2(fast)]
pub(crate) fn op_binding_compare(#[buffer] left: &[u8], #[buffer] right: &[u8]) -> i32 {
    match left.cmp(right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// First position of `needle` at `from, from + step, ..` up to and including
/// `last` when `forward` is set, otherwise at `from, from - step, ..` down to
/// zero. The JS side clamps `from` to `last` before walking backwards, because
/// `Buffer#indexOf` searches before the start offset when `from` is past the end.
#[op2(fast)]
pub(crate) fn op_binding_index_of(
    #[buffer] haystack: &[u8],
    #[buffer] needle: &[u8],
    #[number] from: usize,
    #[number] last: usize,
    #[number] step: usize,
    forward: bool,
) -> i32 {
    let step = step.max(1);
    let last = last.min(haystack.len());
    let from = from.min(haystack.len());
    if needle.is_empty() {
        return -1;
    }

    if forward {
        let mut position = from;
        while position <= last {
            if haystack[position..].starts_with(needle) {
                return position as i32;
            }
            position += step;
        }
        return -1;
    }

    let mut position = from;
    loop {
        if haystack[position..].starts_with(needle) {
            return position as i32;
        }
        if position < step {
            return -1;
        }
        position -= step;
    }
}

/// First position of `value` (a byte) at `from, from + 1, ..` below `last` when
/// `forward` is set, otherwise the last one at or below `from` that is also below
/// `last`. Same clamping contract as [`op_binding_index_of`].
#[op2(fast)]
pub(crate) fn op_binding_index_of_byte(
    #[buffer] haystack: &[u8],
    #[smi] value: u32,
    #[number] from: usize,
    #[number] last: usize,
    forward: bool,
) -> i32 {
    let last = last.min(haystack.len());
    let from = from.min(haystack.len());
    if forward {
        return haystack
            .get(from..last)
            .and_then(|search| search.iter().position(|&byte| u32::from(byte) == value))
            .map_or(-1, |offset| (from + offset) as i32);
    }

    let end = from.saturating_add(1).min(last);
    haystack[..end]
        .iter()
        .rposition(|&byte| u32::from(byte) == value)
        .map_or(-1, |position| position as i32)
}

/// `Buffer#swap16`/`32`/`64`: reverse the bytes inside every `size`-byte group.
#[op2(fast)]
pub(crate) fn op_binding_swap(#[buffer] bytes: &mut [u8], #[number] size: usize) {
    if size < 2 {
        return;
    }
    for group in bytes.chunks_exact_mut(size) {
        group.reverse();
    }
}

#[op2(fast)]
pub(crate) const fn op_binding_is_ascii(#[buffer] bytes: &[u8]) -> bool {
    bytes.is_ascii()
}

#[op2(fast)]
pub(crate) const fn op_binding_is_utf8(#[buffer] bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

/// `Buffer#toString('ascii')`.
#[op2]
#[string]
pub(crate) fn op_binding_ascii_slice(
    #[buffer] bytes: &[u8],
    #[number] start: usize,
    #[number] end: usize,
) -> String {
    let (start, end) = clamp_range(bytes.len(), start, end);
    bytes[start..end]
        .iter()
        .map(|byte| char::from(byte & 0x7f))
        .collect()
}

/// `Buffer#toString('latin1')`.
#[op2]
#[string]
pub(crate) fn op_binding_latin1_slice(
    #[buffer] bytes: &[u8],
    #[number] start: usize,
    #[number] end: usize,
) -> String {
    let (start, end) = clamp_range(bytes.len(), start, end);
    bytes[start..end]
        .iter()
        .map(|byte| char::from(*byte))
        .collect()
}

/// `Buffer#toString('hex')`.
#[op2]
#[string]
pub(crate) fn op_binding_hex_slice(
    #[buffer] bytes: &[u8],
    #[number] start: usize,
    #[number] end: usize,
) -> String {
    let (start, end) = clamp_range(bytes.len(), start, end);
    let mut out = String::with_capacity((end - start) * 2);
    for byte in &bytes[start..end] {
        out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Decodes a hex string up to the first pair that is not two hex digits, the way
/// `Buffer#write` and `Buffer.fill` consume their input.
#[op2]
#[buffer]
pub(crate) fn op_binding_hex_decode(#[string] value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let (Some(high), Some(low)) = (hex_value(pair[0]), hex_value(pair[1])) else {
            break;
        };
        out.push((high << 4) | low);
    }
    out
}

/// `Buffer.isLatin1`: every code unit is representable in Latin-1.
#[op2(fast)]
pub(crate) fn op_binding_is_latin1(#[string] value: &str) -> bool {
    value.chars().all(|character| u32::from(character) <= 0xff)
}

/// `Buffer#stringLength(input, 'utf8')`: the UTF-16 length of the decoded bytes.
#[op2(fast)]
#[number]
pub(crate) fn op_binding_string_length_utf8(#[buffer] bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).encode_utf16().count()
}

/// UTF-16 length of a valid UTF-8 prefix, so `TextEncoder#encodeInto` can report
/// how many code units it consumed.
#[op2(fast)]
#[number]
pub(crate) fn op_binding_utf8_units(#[buffer] bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).encode_utf16().count()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `Buffer#toString('base64')` and `Buffer#toString('base64url')`.
#[op2]
#[string]
pub(crate) fn op_binding_base64_slice(
    #[buffer] bytes: &[u8],
    #[number] start: usize,
    #[number] end: usize,
    url: bool,
) -> String {
    let (start, end) = clamp_range(bytes.len(), start, end);
    let digits = if url { BASE64_URL } else { BASE64 };
    let bytes = &bytes[start..end];

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);

        out.push(char::from(digits[((triple >> 18) & 0x3f) as usize]));
        out.push(char::from(digits[((triple >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(digits[((triple >> 6) & 0x3f) as usize]));
        } else if !url {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(digits[(triple & 0x3f) as usize]));
        } else if !url {
            out.push('=');
        }
    }
    out
}

/// Decodes base64 leniently: characters outside the alphabet are skipped, `=`
/// ends the input, and a trailing group of fewer than eight bits is dropped.
#[op2]
#[buffer]
pub(crate) fn op_binding_base64_decode(#[string] value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in bytes {
        if byte == b'=' {
            break;
        }
        let Some(value) = base64_value(byte) else {
            continue;
        };
        accumulator = ((accumulator << 6) | u32::from(value)) & 0x00ff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulator >> bits) & 0xff) as u8);
        }
    }
    out
}

const fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}
