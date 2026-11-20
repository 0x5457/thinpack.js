use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const MAGIC: u64 = u64::from_le_bytes(*b"THINPACK");

pub const FORMAT_VERSION: u32 = 4;

/// Runtime API level used when selecting templates with the same package version.
pub const TEMPLATE_API: &str = "1";

pub const TRAILER_LEN: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Compression {
    None = 0,
    Brotli = 1,
}

impl Compression {
    fn from_byte(byte: u8) -> Result<Self, Error> {
        match byte {
            0 => Ok(Self::None),
            1 => Ok(Self::Brotli),
            other => Err(Error::Invalid(format!("unknown compression {other}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    pub path: String,
    pub offset: u64,
    pub length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRecord {
    pub key: String,
    pub offset: u64,
    pub length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub version: String,
    pub backend: String,
    pub capabilities_hash: u64,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub entry: String,
    pub capabilities: Vec<String>,
    pub template: Template,
    pub files: Vec<FileRecord>,
    pub assets: Vec<AssetRecord>,
}

impl Manifest {
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the manifest has more entries or a longer
    /// string than the payload format can encode.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::with_capacity(64 + (self.files.len() + self.assets.len()) * 48);
        push_count(&mut bytes, self.files.len(), "files")?;
        push_count(&mut bytes, self.capabilities.len(), "capabilities")?;
        push_string(&mut bytes, &self.entry)?;
        push_string(&mut bytes, &self.template.version)?;
        push_string(&mut bytes, &self.template.backend)?;
        bytes.extend_from_slice(&self.template.capabilities_hash.to_le_bytes());
        for capability in &self.capabilities {
            push_string(&mut bytes, capability)?;
        }
        for file in &self.files {
            push_string(&mut bytes, &file.path)?;
            bytes.extend_from_slice(&file.offset.to_le_bytes());
            bytes.extend_from_slice(&file.length.to_le_bytes());
        }
        push_count(&mut bytes, self.assets.len(), "assets")?;
        for asset in &self.assets {
            push_string(&mut bytes, &asset.key)?;
            bytes.extend_from_slice(&asset.offset.to_le_bytes());
            bytes.extend_from_slice(&asset.length.to_le_bytes());
        }
        Ok(bytes)
    }

    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the bytes are truncated, carry trailing
    /// data or encode a string that is not valid UTF-8.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader { bytes, index: 0 };
        let file_count = reader.u32()? as usize;
        let capability_count = reader.u32()? as usize;
        let entry = reader.string()?;
        let template = Template {
            version: reader.string()?,
            backend: reader.string()?,
            capabilities_hash: reader.u64()?,
        };

        let mut capabilities = Vec::with_capacity(capability_count);
        for _ in 0..capability_count {
            capabilities.push(reader.string()?);
        }

        let mut files = Vec::with_capacity(file_count);
        for _ in 0..file_count {
            files.push(FileRecord {
                path: reader.string()?,
                offset: reader.u64()?,
                length: reader.u32()?,
            });
        }

        let asset_count = reader.u32()? as usize;
        let mut assets = Vec::with_capacity(asset_count);
        for _ in 0..asset_count {
            assets.push(AssetRecord {
                key: reader.string()?,
                offset: reader.u64()?,
                length: reader.u32()?,
            });
        }

        if reader.index != bytes.len() {
            return Err(Error::Invalid("trailing manifest bytes".to_owned()));
        }
        Ok(Self {
            entry,
            capabilities,
            template,
            files,
            assets,
        })
    }
}

#[derive(Debug)]
pub struct Pack {
    pub manifest: Manifest,
    pub blob: Vec<u8>,
}

impl Pack {
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the payload framing is truncated or
    /// carries trailing data, and when the manifest or brotli blob is invalid.
    pub fn read(payload: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader {
            bytes: payload,
            index: 0,
        };
        let compression = Compression::from_byte(reader.byte()?)?;
        let manifest_len = reader.u32()? as usize;
        let manifest_bytes = reader.take(manifest_len)?;
        let blob_len = reader.u32()? as usize;
        let blob_bytes = reader.take(blob_len)?;
        if reader.index != payload.len() {
            return Err(Error::Invalid("trailing payload bytes".to_owned()));
        }

        let blob = match compression {
            Compression::None => blob_bytes.to_vec(),
            Compression::Brotli => {
                let mut output = Vec::new();
                brotli_decompressor::BrotliDecompress(
                    &mut std::io::Cursor::new(blob_bytes),
                    &mut output,
                )
                .map_err(|error| Error::Invalid(format!("brotli: {error}")))?;
                output
            }
        };

        Ok(Self {
            manifest: Manifest::decode(manifest_bytes)?,
            blob,
        })
    }
    #[must_use]
    pub fn slice_of(&self, file: &FileRecord) -> Option<&[u8]> {
        self.slice(file.offset, file.length)
    }

    #[must_use]
    pub fn asset(&self, key: &str) -> Option<&[u8]> {
        let asset = self.manifest.assets.iter().find(|asset| asset.key == key)?;
        self.slice(asset.offset, asset.length)
    }

    pub fn asset_keys(&self) -> impl Iterator<Item = &str> {
        self.manifest.assets.iter().map(|asset| asset.key.as_str())
    }

    fn slice(&self, offset: u64, length: u32) -> Option<&[u8]> {
        let start = usize::try_from(offset).ok()?;
        self.blob.get(start..start + length as usize)
    }
}

/// # Errors
///
/// Returns [`Error::Invalid`] when the blob or the encoded manifest does not fit
/// the payload format.
pub fn assemble(
    manifest: &Manifest,
    blob: &[u8],
    compression: Compression,
) -> Result<Vec<u8>, Error> {
    let blob_len =
        u32::try_from(blob.len()).map_err(|_| Error::Invalid("blob too large".to_owned()))?;
    let manifest_bytes = manifest.encode()?;
    let manifest_len = u32::try_from(manifest_bytes.len())
        .map_err(|_| Error::Invalid("manifest too large".to_owned()))?;
    let mut payload = Vec::with_capacity(1 + 4 + manifest_bytes.len() + 4 + blob.len());
    payload.push(compression as u8);
    push_u32(&mut payload, manifest_len);
    payload.extend_from_slice(&manifest_bytes);
    push_u32(&mut payload, blob_len);
    payload.extend_from_slice(blob);
    Ok(payload)
}

/// # Errors
///
/// Returns [`Error::Invalid`] when the payload is larger than `u64::MAX` bytes.
pub fn append_to_binary(runtime: &[u8], payload: &[u8]) -> Result<Vec<u8>, Error> {
    let payload_len =
        u64::try_from(payload.len()).map_err(|_| Error::Invalid("payload too large".to_owned()))?;
    let mut binary = Vec::with_capacity(runtime.len() + 8 + payload.len() + TRAILER_LEN);
    binary.extend_from_slice(runtime);
    while binary.len() % 8 != 0 {
        binary.push(0);
    }
    binary.extend_from_slice(payload);

    binary.extend_from_slice(&payload_len.to_le_bytes());
    binary.extend_from_slice(&MAGIC.to_le_bytes());
    binary.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    binary.extend_from_slice(&0u32.to_le_bytes());
    Ok(binary)
}

/// # Errors
///
/// Returns [`Error::Invalid`] when the executable cannot be opened or read, when
/// its trailer points past the end of the file or when the embedded payload does
/// not decode.
pub fn read_from_executable(path: &Path) -> Result<Option<Pack>, Error> {
    let mut file =
        File::open(path).map_err(|error| Error::Invalid(format!("{}: {error}", path.display())))?;
    let length = file
        .metadata()
        .map_err(|error| Error::Invalid(format!("{}: {error}", path.display())))?
        .len();
    if length < TRAILER_LEN as u64 {
        return Ok(None);
    }

    let mut trailer = [0u8; TRAILER_LEN];
    let trailer_len = i64::try_from(TRAILER_LEN)
        .map_err(|_| Error::Invalid("trailer is too large".to_owned()))?;
    file.seek(SeekFrom::End(-trailer_len))
        .and_then(|_| file.read_exact(&mut trailer))
        .map_err(|error| Error::Invalid(format!("{}: {error}", path.display())))?;
    let Some(payload_len) = trailer_payload_len(&trailer) else {
        return Ok(None);
    };
    if payload_len
        .checked_add(TRAILER_LEN as u64)
        .is_none_or(|total| length < total)
    {
        return Err(Error::Invalid(
            "payload length exceeds the executable size".to_owned(),
        ));
    }

    file.seek(SeekFrom::Start(length - payload_len - TRAILER_LEN as u64))
        .map_err(|error| Error::Invalid(format!("{}: {error}", path.display())))?;
    let payload_len = usize::try_from(payload_len)
        .map_err(|_| Error::Invalid("payload is too large for this platform".to_owned()))?;
    let mut payload = vec![0u8; payload_len];
    file.read_exact(&mut payload)
        .map_err(|error| Error::Invalid(format!("{}: {error}", path.display())))?;
    Ok(Some(Pack::read(&payload)?))
}

#[must_use]
pub fn trailer_payload_len(trailer: &[u8]) -> Option<u64> {
    if trailer.len() != TRAILER_LEN {
        return None;
    }
    let length = u64::from_le_bytes(trailer[0..8].try_into().ok()?);
    let magic = u64::from_le_bytes(trailer[8..16].try_into().ok()?);
    let version = u32::from_le_bytes(trailer[16..20].try_into().ok()?);
    (magic == MAGIC && version == FORMAT_VERSION).then_some(length)
}

/// # Errors
///
/// Returns [`Error::Invalid`] when none of `THINPACK_CACHE`, `XDG_CACHE_HOME` and
/// `HOME` is set.
pub fn cache_root() -> Result<PathBuf, Error> {
    if let Some(root) = std::env::var_os("THINPACK_CACHE") {
        return Ok(PathBuf::from(root));
    }
    if let Some(root) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(root).join("thinpack"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        Error::Invalid("none of THINPACK_CACHE, XDG_CACHE_HOME, HOME is set".to_owned())
    })?;
    Ok(PathBuf::from(home).join(".cache/thinpack"))
}

#[derive(Debug)]
pub enum Error {
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

struct Reader<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self
            .index
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| Error::Invalid("unexpected end of payload".to_owned()))?;
        let slice = &self.bytes[self.index..end];
        self.index = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    fn string(&mut self) -> Result<String, Error> {
        let length = u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes")) as usize;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::Invalid("path is not valid UTF-8".to_owned()))
    }
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_count(bytes: &mut Vec<u8>, count: usize, field: &str) -> Result<(), Error> {
    let count = u32::try_from(count)
        .map_err(|_| Error::Invalid(format!("too many {field} for the manifest format")))?;
    push_u32(bytes, count);
    Ok(())
}

fn push_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), Error> {
    let length = u16::try_from(value.len())
        .map_err(|_| Error::Invalid(format!("`{value}` is longer than 64 KiB")))?;
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            entry: "/app/main.js".to_owned(),
            capabilities: vec!["console".to_owned(), "process".to_owned()],
            template: Template {
                version: "0.1.0".to_owned(),
                backend: "quickjs".to_owned(),
                capabilities_hash: 42,
            },
            files: vec![
                FileRecord {
                    path: "/app/main.js".to_owned(),
                    offset: 0,
                    length: 4,
                },
                FileRecord {
                    path: "/app/package.json".to_owned(),
                    offset: 4,
                    length: 2,
                },
            ],
            assets: vec![AssetRecord {
                key: "greeting".to_owned(),
                offset: 6,
                length: 5,
            }],
        }
    }

    #[test]
    fn payload_roundtrip() {
        let manifest = manifest();
        let payload = assemble(&manifest, b"main{}hello", Compression::None).unwrap();
        let pack = Pack::read(&payload).unwrap();
        assert_eq!(pack.manifest.entry, "/app/main.js");
        let files = &pack.manifest.files;
        assert_eq!(pack.slice_of(&files[0]), Some(&b"main"[..]));
        assert_eq!(pack.slice_of(&files[1]), Some(&b"{}"[..]));
        assert_eq!(pack.asset("greeting"), Some(&b"hello"[..]));
        assert_eq!(pack.asset_keys().collect::<Vec<_>>(), vec!["greeting"]);
    }

    #[test]
    fn trailer_roundtrip() {
        let payload = b"payload-bytes".to_vec();
        let binary = append_to_binary(b"ELF-runtime", &payload).unwrap();
        let trailer = &binary[binary.len() - TRAILER_LEN..];
        assert_eq!(trailer_payload_len(trailer), Some(payload.len() as u64));
        assert!(trailer_payload_len(b"short").is_none());
        assert!(trailer_payload_len(&[0u8; TRAILER_LEN]).is_none());
    }

    #[test]
    fn manifest_rejects_trailing_bytes() {
        let mut bytes = manifest().encode().expect("manifest fits the format");
        bytes.push(0);
        assert!(Manifest::decode(&bytes).is_err());
    }
}
