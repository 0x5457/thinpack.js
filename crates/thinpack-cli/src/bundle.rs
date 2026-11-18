use std::error::Error;
use std::fs;
use std::io;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use thinpack_analyze::Analysis;
use thinpack_caps::Module;
use thinpack_pack::{AssetRecord, Compression, FileRecord, Manifest};

use crate::options::AssetInput;
use crate::templates::TemplateInfo;

pub(crate) struct Bundle {
    pub manifest: Manifest,
    pub blob: Vec<u8>,
}

pub(crate) struct PackedFile {
    path: String,
    contents: Vec<u8>,
}
pub(crate) fn project_root(entry: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let directory = entry
        .parent()
        .ok_or("entry has no parent directory")?
        .to_path_buf();
    for ancestor in directory.ancestors() {
        if ancestor
            .components()
            .any(|component| component.as_os_str() == "node_modules")
        {
            continue;
        }
        if ancestor.join("package.json").is_file() {
            return Ok(ancestor.to_path_buf());
        }
    }
    Ok(directory)
}
pub(crate) fn build(
    root: &Path,
    entry: &Path,
    capabilities: &[&'static Module],
    info: &TemplateInfo,
    analysis: &Analysis,
    assets: &[AssetInput],
) -> Result<Bundle, Box<dyn Error>> {
    let mut files = collect_analysis_files(root, analysis)?;
    files.extend(collect_package_json(root, &files)?);
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let (mut manifest, mut blob) = build_manifest(root, entry, capabilities, info, &files);
    for asset in assets {
        let contents =
            fs::read(&asset.path).map_err(|error| format!("{}: {error}", asset.path.display()))?;
        if manifest
            .assets
            .iter()
            .any(|existing| existing.key == asset.key)
        {
            return Err(format!("duplicate asset key `{}`", asset.key).into());
        }
        let offset = blob.len() as u64;
        blob.extend_from_slice(&contents);
        manifest.assets.push(AssetRecord {
            key: asset.key.clone(),
            offset,
            length: record_length(&contents),
        });
    }

    Ok(Bundle { manifest, blob })
}
pub(crate) fn write(
    template: &Path,
    manifest: &Manifest,
    blob: &[u8],
    output: &Path,
) -> Result<(usize, usize), Box<dyn Error>> {
    let mut compressed = Vec::new();
    brotli::enc::BrotliCompress(
        &mut Cursor::new(blob),
        &mut compressed,
        &brotli::enc::BrotliEncoderParams {
            quality: 11,
            lgwin: 22,
            ..Default::default()
        },
    )
    .map_err(|error| format!("brotli: {error}"))?;

    let payload = thinpack_pack::assemble(manifest, &compressed, Compression::Brotli)?;
    let runtime = fs::read(template).map_err(|error| format!("{}: {error}", template.display()))?;
    let binary = thinpack_pack::append_to_binary(&runtime, &payload)?;
    fs::write(output, &binary).map_err(|error| format!("{}: {error}", output.display()))?;

    copy_permissions(template, output)?;
    Ok((payload.len(), binary.len()))
}

/// Carries the template's executable bit over to the artifact.
#[cfg(unix)]
fn copy_permissions(template: &Path, output: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(template)?.permissions().mode();
    fs::set_permissions(output, fs::Permissions::from_mode(mode))
}

/// Windows decides executability by extension, not by a mode bit.
#[cfg(not(unix))]
fn copy_permissions(_template: &Path, _output: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) fn default_output(entry: &Path) -> PathBuf {
    let name = entry.file_stem().map_or_else(
        || "app".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    PathBuf::from(name)
}

fn collect_analysis_files(
    root: &Path,
    analysis: &Analysis,
) -> Result<Vec<PackedFile>, Box<dyn Error>> {
    let mut files = Vec::with_capacity(analysis.files.len());
    for path in &analysis.files {
        files.push(PackedFile {
            path: virtual_path(root, path),
            contents: fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
        });
    }
    Ok(files)
}

fn collect_package_json(
    root: &Path,
    already: &[PackedFile],
) -> Result<Vec<PackedFile>, Box<dyn Error>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                // `.git` and cargo's `target` never carry a packed
                // `package.json`; skipping them keeps the walk off large trees.
                if entry.file_name() != ".git" && entry.file_name() != "target" {
                    stack.push(path);
                }
            } else if metadata.is_file()
                && path.file_name().is_some_and(|name| name == "package.json")
            {
                let virtual_path = virtual_path(root, &path);
                if already.iter().any(|file| file.path == virtual_path) {
                    continue;
                }
                files.push(PackedFile {
                    path: virtual_path,
                    contents: fs::read(&path)?,
                });
            }
        }
    }
    Ok(files)
}

/// Lexically removes `.` components and resolves `..`, the same semantics the
/// runtime's virtual-path lookup applies when it resolves a specifier.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Maps a host path under `root` to its `/app` virtual path. This is the one
/// place that builds virtual paths, so `.` and `..` components never reach a
/// manifest record: the runtime's lookup normalizes specifiers, and a record
/// that kept them would never be found.
fn virtual_path(root: &Path, path: &Path) -> String {
    let root = normalize(root);
    let relative = normalize(path.strip_prefix(&root).unwrap_or(path));
    format!("/app/{}", relative.to_string_lossy().replace('\\', "/"))
}

/// A manifest record stores lengths as `u32`; a longer file also pushes the blob past
/// `u32::MAX`, which `thinpack_pack::assemble` rejects before a bundle is written.
#[expect(
    clippy::cast_possible_truncation,
    reason = "lengths above `u32::MAX` cannot reach a written bundle"
)]
const fn record_length(contents: &[u8]) -> u32 {
    contents.len() as u32
}

fn build_manifest(
    root: &Path,
    entry: &Path,
    capabilities: &[&'static Module],
    info: &TemplateInfo,
    files: &[PackedFile],
) -> (Manifest, Vec<u8>) {
    let mut blob = Vec::new();
    let mut records = Vec::with_capacity(files.len());
    for file in files {
        let offset = blob.len() as u64;
        blob.extend_from_slice(&file.contents);
        records.push(FileRecord {
            path: file.path.clone(),
            offset,
            length: record_length(&file.contents),
        });
    }
    let manifest = Manifest {
        entry: virtual_path(root, entry),
        capabilities: capabilities
            .iter()
            .map(|module| module.name.to_owned())
            .collect(),
        template: thinpack_pack::Template {
            version: info.version.clone(),
            backend: info.backend.clone(),
            capabilities_hash: info.capabilities_hash,
        },
        files: records,
        assets: Vec::new(),
    };
    (manifest, blob)
}
