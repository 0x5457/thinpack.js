use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use thinpack_analyze::Analysis;
use thinpack_caps::Module;
use thinpack_pack::{AssetRecord, Compression, FileRecord, FormatRecord, Manifest};
use thinpack_resolve::normalize;

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
        if thinpack_resolve::is_under_node_modules(ancestor) {
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
    conditions: &[String],
) -> Result<Bundle, Box<dyn Error>> {
    let mut files = collect_analysis_files(root, analysis)?;
    for path in &analysis.static_files {
        let Some(virtual_path) = virtual_path(root, path) else {
            eprintln!(
                "warning: static asset {} is outside the project root {}; it is left out of the payload",
                path.display(),
                root.display()
            );
            continue;
        };
        if files.iter().any(|file| file.path == virtual_path) {
            continue;
        }
        files.push(PackedFile {
            path: virtual_path,
            contents: fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
        });
    }
    let manifests = collect_manifests(root, &analysis.manifests, &files)?;
    files.extend(manifests);
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let (mut manifest, mut blob) = build_manifest(
        root,
        entry,
        capabilities,
        info,
        &files,
        analysis,
        conditions,
    );
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
        // Resolution walks `node_modules` above the root, but the payload is rooted
        // there: a file outside it cannot be reached by a specifier in the artifact.
        let Some(virtual_path) = virtual_path(root, path) else {
            if analysis.required.contains(path) {
                return Err(format!(
                    "{} resolves outside the project root {}; the payload is rooted there, so the file would be unreachable at run time. Copy it into the project or install the dependency inside the root",
                    path.display(),
                    root.display()
                )
                .into());
            }
            // `analyze_entry` already warned: only a guarded `require`, `import()`
            // or optional dependency reaches this file.
            continue;
        };
        // TypeScript ships as the JavaScript packing stripped it to; every other
        // file ships byte for byte.
        let contents = analysis.stripped.get(path).map_or_else(
            || fs::read(path).map_err(|error| format!("{}: {error}", path.display())),
            |javascript| Ok(javascript.clone().into_bytes()),
        )?;
        files.push(PackedFile {
            path: virtual_path,
            contents,
        });
    }
    Ok(files)
}

/// The `package.json` files the payload needs: the ones the resolver and
/// [`thinpack_resolve::format_of`] read during analysis, plus the app's own manifest
/// at the root. The loader reads the same set at run time, and anything above the
/// root could not be reached from `/app`.
fn collect_manifests(
    root: &Path,
    manifests: &BTreeSet<PathBuf>,
    packed: &[PackedFile],
) -> Result<Vec<PackedFile>, Box<dyn Error>> {
    let mut seen: BTreeSet<String> = packed.iter().map(|file| file.path.clone()).collect();
    let root_manifest = root.join("package.json");
    let candidates = manifests
        .iter()
        .map(PathBuf::as_path)
        .chain(root_manifest.is_file().then_some(root_manifest.as_path()));
    let mut files = Vec::new();
    for manifest in candidates {
        let Some(path) = virtual_path(root, manifest) else {
            continue;
        };
        if seen.insert(path.clone()) {
            files.push(PackedFile {
                path,
                contents: fs::read(manifest)
                    .map_err(|error| format!("{}: {error}", manifest.display()))?,
            });
        }
    }
    Ok(files)
}

/// Maps a host path under `root` to its `/app` virtual path, or `None` when the
/// path is outside the root and therefore unreachable in the payload. This is the
/// one place that builds virtual paths, so `.` and `..` components never reach a
/// manifest record: the runtime's lookup normalizes specifiers, and a record that
/// kept them would never be found.
fn virtual_path(root: &Path, path: &Path) -> Option<String> {
    let relative = normalize(path.strip_prefix(root).ok()?);
    Some(format!(
        "/app/{}",
        relative.to_string_lossy().replace('\\', "/")
    ))
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
    analysis: &Analysis,
    conditions: &[String],
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
        entry: virtual_path(root, entry).expect("the project root contains the entry"),
        capabilities: capabilities
            .iter()
            .map(|module| module.name.to_owned())
            .collect(),
        conditions: conditions.to_vec(),
        template: thinpack_pack::Template {
            version: info.version.clone(),
            backend: info.backend,
            capabilities_hash: info.capabilities_hash,
        },
        files: records,
        formats: analysis
            .formats
            .iter()
            .filter_map(|(path, format)| {
                virtual_path(root, path).map(|path| FormatRecord {
                    path,
                    format: *format,
                })
            })
            .collect(),
        assets: Vec::new(),
    };
    (manifest, blob)
}
