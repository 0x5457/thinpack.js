use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use thinpack_core::{VirtualEntry, VirtualFiles, VirtualStat};
use thinpack_resolve::{FileSystem, Format};

#[derive(Debug)]
pub struct Vfs {
    files: BTreeMap<PathBuf, Vec<u8>>,
    directories: BTreeSet<PathBuf>,
    formats: BTreeMap<PathBuf, Format>,
    conditions: Vec<String>,
}

const ROOT: &str = "/app";

impl Vfs {
    /// # Errors
    ///
    /// Returns [`thinpack_pack::Error::Invalid`] when a file in the manifest lives
    /// outside `/app` or is missing from the payload blob.
    pub fn from_pack(pack: &thinpack_pack::Pack) -> Result<Self, thinpack_pack::Error> {
        let mut files = BTreeMap::new();
        let mut directories = BTreeSet::new();

        for record in &pack.manifest.files {
            let path = PathBuf::from(&record.path);
            if !path.starts_with(ROOT) {
                return Err(thinpack_pack::Error::Invalid(format!(
                    "{}: payload paths must live under {ROOT}",
                    record.path
                )));
            }
            let Some(contents) = pack.slice_of(record) else {
                return Err(thinpack_pack::Error::Invalid(format!(
                    "{}: missing from the payload blob",
                    record.path
                )));
            };
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if !directories.insert(directory.to_path_buf()) {
                    break;
                }
                parent = directory.parent();
            }
            files.insert(path, contents.to_vec());
        }

        let mut formats = BTreeMap::new();
        for record in &pack.manifest.formats {
            let path = PathBuf::from(&record.path);
            if !path.starts_with(ROOT) || !files.contains_key(&path) {
                return Err(thinpack_pack::Error::Invalid(format!(
                    "{}: format override does not name a packed file",
                    record.path
                )));
            }
            formats.insert(path, record.format);
        }

        Ok(Self {
            files,
            directories,
            formats,
            conditions: pack.manifest.conditions.clone(),
        })
    }
}

impl VirtualFiles for Vfs {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(Path::new(path)).cloned()
    }

    fn stat(&self, path: &str) -> Option<VirtualStat> {
        let path = Path::new(path);
        if let Some(contents) = self.files.get(path) {
            return Some(VirtualStat {
                len: contents.len() as u64,
                is_file: true,
                is_dir: false,
            });
        }
        self.directories.contains(path).then_some(VirtualStat {
            len: 0,
            is_file: false,
            is_dir: true,
        })
    }

    fn entries(&self, path: &str) -> Option<Vec<VirtualEntry>> {
        let directory = Path::new(path);
        if !self.directories.contains(directory) {
            return None;
        }
        let mut entries: BTreeMap<String, bool> = BTreeMap::new();
        for file in self.files.keys() {
            if file.parent() == Some(directory)
                && let Some(name) = file.file_name()
            {
                entries.insert(name.to_string_lossy().into_owned(), false);
            }
        }
        for child in &self.directories {
            if child.parent() == Some(directory)
                && let Some(name) = child.file_name()
                && child.starts_with(ROOT)
            {
                entries.insert(name.to_string_lossy().into_owned(), true);
            }
        }
        Some(
            entries
                .into_iter()
                .map(|(name, is_dir)| VirtualEntry { name, is_dir })
                .collect(),
        )
    }
}

impl FileSystem for Vfs {
    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.directories.contains(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn format_override(&self, path: &Path) -> Option<Format> {
        self.formats.get(path).copied()
    }

    fn custom_conditions(&self) -> &[String] {
        &self.conditions
    }
}
