use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;

use thinpack_core::{VirtualEntry, VirtualFilesRef, VirtualStat};

const FIRST_VIRTUAL_FD: i32 = 1 << 30;
const S_IFREG: u64 = 0o100_000;
const S_IFDIR: u64 = 0o044_000;

pub struct VirtualMount {
    files: VirtualFilesRef,
    handles: RefCell<BTreeMap<i32, Handle>>,
    next: Cell<i32>,
}

impl fmt::Debug for VirtualMount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualMount").finish_non_exhaustive()
    }
}

struct Handle {
    contents: Vec<u8>,
    position: usize,
}

impl VirtualMount {
    pub fn new(files: VirtualFilesRef) -> Self {
        Self {
            files,
            handles: RefCell::new(BTreeMap::new()),
            next: Cell::new(FIRST_VIRTUAL_FD),
        }
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files.read(path)
    }

    pub fn stat(&self, path: &str) -> Option<VirtualStat> {
        self.files.stat(path)
    }

    pub fn entries(&self, path: &str) -> Option<Vec<VirtualEntry>> {
        self.files.entries(path)
    }

    pub fn open(&self, path: &str) -> Option<i32> {
        let contents = self.files.read(path)?;
        let fd = self.next.get();
        self.next.set(fd + 1);
        self.handles.borrow_mut().insert(
            fd,
            Handle {
                contents,
                position: 0,
            },
        );
        Some(fd)
    }

    #[must_use]
    pub const fn is_virtual_fd(fd: i32) -> bool {
        fd >= FIRST_VIRTUAL_FD
    }

    pub fn close(&self, fd: i32) -> bool {
        self.handles.borrow_mut().remove(&fd).is_some()
    }

    pub fn len(&self, fd: i32) -> Option<u64> {
        self.handles
            .borrow()
            .get(&fd)
            .map(|handle| handle.contents.len() as u64)
    }

    pub fn read_at(&self, fd: i32, buffer: &mut [u8], position: f64) -> Option<usize> {
        let mut handles = self.handles.borrow_mut();
        let handle = handles.get_mut(&fd)?;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the negative case takes the other branch, so this cast only sees a non-negative JS position"
        )]
        let start = if position < 0.0 {
            handle.position
        } else {
            position as usize
        };
        let start = start.min(handle.contents.len());
        let available = &handle.contents[start..];
        let copied = available.len().min(buffer.len());
        buffer[..copied].copy_from_slice(&available[..copied]);
        if position < 0.0 {
            handle.position = start + copied;
        }
        Some(copied)
    }

    pub fn read_to_end(&self, fd: i32) -> Option<Vec<u8>> {
        let mut handles = self.handles.borrow_mut();
        let handle = handles.get_mut(&fd)?;
        let rest = handle.contents[handle.position.min(handle.contents.len())..].to_vec();
        handle.position = handle.contents.len();
        Some(rest)
    }
}

pub(crate) fn encode_stat(stat: &VirtualStat) -> String {
    let mode = if stat.is_dir {
        S_IFDIR | 0o755
    } else {
        S_IFREG | 0o644
    };
    let fields: [u64; 10] = [
        0,
        mode,
        1,
        0,
        0,
        0,
        4096,
        0,
        stat.len,
        stat.len.div_ceil(512),
    ];
    let mut encoded: Vec<String> = fields.iter().map(u64::to_string).collect();
    for _ in 0..3 {
        encoded.push("0".to_owned());
        encoded.push("0".to_owned());
    }
    encoded.join("\0")
}

pub(crate) fn encode_entries(entries: &[VirtualEntry], with_types: bool) -> String {
    let mut fields: Vec<String> = Vec::new();
    for entry in entries {
        fields.push(entry.name.clone());
        if with_types {
            fields.push(if entry.is_dir { "2" } else { "1" }.to_owned());
        }
    }
    fields.join("\0")
}
