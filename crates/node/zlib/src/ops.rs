use std::io::{Read, Write};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use flate2::Compression;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};

use brotli::BrotliDecompress;
use brotli::enc::backward_references::BrotliEncoderMode;
use brotli::enc::{BrotliCompress, BrotliEncoderParams};

pub(crate) const OPS: &[OpDecl] = &[
    op_zlib_compress(),
    op_zlib_decompress(),
    op_zlib_brotli_compress(),
    op_zlib_brotli_decompress(),
    op_zlib_zstd_compress(),
    op_zlib_zstd_decompress(),
];

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a direct `map_err` handler, which requires an owned error"
)]
fn zlib_error(error: std::io::Error) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ERR_ZLIB\0{error}"))
}

#[op2]
#[buffer]
fn op_zlib_compress(
    #[string] format: &str,
    #[buffer] data: &[u8],
    #[smi] level: i32,
) -> Result<Box<[u8]>, JsErrorBox> {
    let level = u32::try_from(level).map_or(6, |level| level.min(9));
    let compression = Compression::new(level);
    let mut output = Vec::new();
    let result = match format {
        "gzip" => GzEncoder::new(&mut output, compression).write_all(data),
        "deflate" => ZlibEncoder::new(&mut output, compression).write_all(data),
        "deflate-raw" => DeflateEncoder::new(&mut output, compression).write_all(data),
        other => {
            return Err(JsErrorBox::new(
                "Error",
                format!("ERR_ZLIB_UNKNOWN_FORMAT\0{other}"),
            ));
        }
    };
    result.map_err(zlib_error)?;
    Ok(output.into_boxed_slice())
}

#[op2]
#[buffer]
fn op_zlib_decompress(
    #[string] format: &str,
    #[buffer] data: &[u8],
) -> Result<Box<[u8]>, JsErrorBox> {
    let mut output = Vec::new();
    let result = match format {
        "gzip" => GzDecoder::new(data).read_to_end(&mut output),
        "deflate" => ZlibDecoder::new(data).read_to_end(&mut output),
        "deflate-raw" => DeflateDecoder::new(data).read_to_end(&mut output),
        other => {
            return Err(JsErrorBox::new(
                "Error",
                format!("ERR_ZLIB_UNKNOWN_FORMAT\0{other}"),
            ));
        }
    };
    result.map_err(zlib_error)?;
    Ok(output.into_boxed_slice())
}

#[op2]
#[buffer]
fn op_zlib_brotli_compress(
    #[buffer] data: &[u8],
    #[smi] quality: i32,
    #[smi] lgwin: i32,
    #[smi] mode: i32,
) -> Result<Box<[u8]>, JsErrorBox> {
    let params = BrotliEncoderParams {
        quality: quality.clamp(0, 11),
        lgwin: lgwin.clamp(10, 24),
        mode: match mode {
            1 => BrotliEncoderMode::BROTLI_MODE_TEXT,
            2 => BrotliEncoderMode::BROTLI_MODE_FONT,
            _ => BrotliEncoderMode::BROTLI_MODE_GENERIC,
        },
        ..Default::default()
    };
    let mut output = Vec::new();
    BrotliCompress(&mut &data[..], &mut output, &params).map_err(zlib_error)?;
    Ok(output.into_boxed_slice())
}

#[op2]
#[buffer]
fn op_zlib_brotli_decompress(#[buffer] data: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    let mut output = Vec::new();
    BrotliDecompress(&mut &data[..], &mut output).map_err(zlib_error)?;
    Ok(output.into_boxed_slice())
}

#[op2]
#[buffer]
fn op_zlib_zstd_compress(
    #[buffer] data: &[u8],
    #[smi] level: i32,
) -> Result<Box<[u8]>, JsErrorBox> {
    zstd::stream::encode_all(data, level)
        .map(Vec::into_boxed_slice)
        .map_err(zlib_error)
}

#[op2]
#[buffer]
fn op_zlib_zstd_decompress(#[buffer] data: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    zstd::stream::decode_all(data)
        .map(Vec::into_boxed_slice)
        .map_err(zlib_error)
}
