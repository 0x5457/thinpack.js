use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs::Permissions;
use std::fs::{DirBuilder, File, FileTimes, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
#[cfg(unix)]
use std::os::unix::fs::{
    DirBuilderExt, FileExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle};

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
type RawFd = i32;

use crate::virtual_mount::{self, VirtualMount};

use deno_core::{JsBuffer, OpDecl, OpState, op2};
use deno_error::JsErrorBox;

pub(crate) const OPS: &[OpDecl] = &[
    op_fs_open(),
    op_fs_close(),
    op_fs_read(),
    op_fs_read_all_utf8(),
    op_fs_read_file_async(),
    op_fs_read_fd_async(),
    op_fs_write_file_async(),
    op_fs_write(),
    op_fs_write_all_utf8(),
    op_fs_fsync(),
    op_fs_fdatasync(),
    op_fs_ftruncate(),
    op_fs_fstat(),
    op_fs_stat(),
    op_fs_lstat(),
    op_fs_readdir(),
    op_fs_mkdir(),
    op_fs_rmdir(),
    op_fs_rm(),
    op_fs_unlink(),
    op_fs_rename(),
    op_fs_copy_file(),
    op_fs_chmod(),
    op_fs_utimes(),
    op_fs_readlink(),
    op_fs_symlink(),
    op_fs_mkdtemp(),
    op_fs_realpath(),
    op_fs_access(),
    op_fs_exists(),
    op_fs_link(),
    op_fs_chown(),
    op_fs_lchown(),
    op_fs_fchown(),
    op_fs_statfs(),
    op_fs_fchmod(),
    op_fs_futimes(),
    op_fs_lutimes(),
    op_fs_watch_start(),
    op_fs_watch_take(),
    op_fs_watch_wait(),
    op_fs_watch_wake(),
    op_fs_watch_close(),
];

// Node's fs constants are the POSIX/Linux values on every platform (see
// `thinpack-node-internals/src/constants.js`), so translate the flags with the
// matching numbers instead of the host libc's.
const O_ACCMODE: i32 = 3;
const O_CREAT: i32 = 64;
const O_EXCL: i32 = 128;
const O_TRUNC: i32 = 512;
const O_APPEND: i32 = 1024;
#[cfg(unix)]
const TRANSLATED_FLAGS: i32 = O_ACCMODE | O_CREAT | O_EXCL | O_TRUNC | O_APPEND;
const COPYFILE_EXCL: u32 = 1;
const COPYFILE_FICLONE_FORCE: u32 = 4;
const UV_DIRENT_UNKNOWN: u32 = 0;
const UV_DIRENT_FILE: u32 = 1;
const UV_DIRENT_DIR: u32 = 2;
const UV_DIRENT_LINK: u32 = 3;
#[cfg(unix)]
const UV_DIRENT_FIFO: u32 = 4;
#[cfg(unix)]
const UV_DIRENT_SOCKET: u32 = 5;
#[cfg(unix)]
const UV_DIRENT_CHAR: u32 = 6;
#[cfg(unix)]
const UV_DIRENT_BLOCK: u32 = 7;
const EIO: i32 = 5;
const EROFS: i32 = 30;
const EEXIST: i32 = 17;
const EINVAL: i32 = 22;
const EFBIG: i32 = 27;
const ENOTSUP: i32 = 95;
const EISDIR: i32 = 21;
const ENOENT: i32 = 2;
const ENOSPC: i32 = 28;
const W_OK: u32 = 2;
const X_OK: u32 = 1;
const MAX_UTF8_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024 - 1;

#[expect(
    clippy::needless_pass_by_value,
    reason = "fs_error is used directly as the adapter for Result::map_err at every fallible call site; taking a reference would force a closure at each of them"
)]
fn fs_error(error: io::Error) -> JsErrorBox {
    // Deno's uv_compat translates both POSIX errno and Win32 error codes to the
    // negative libuv codes Node's fs binding reports.
    let errno = deno_core::uv_compat::io_error_to_uv(&error);
    JsErrorBox::new("FsError", errno.to_string())
}

fn errno_error(errno: i32) -> JsErrorBox {
    JsErrorBox::new("FsError", (-errno).to_string())
}

fn mount(state: &OpState) -> Option<&VirtualMount> {
    state.try_borrow::<VirtualMount>()
}

#[cfg(unix)]
fn borrowed(fd: RawFd) -> ManuallyDrop<File> {
    // SAFETY: `fd` is an open descriptor owned by the caller. `ManuallyDrop` keeps this
    // wrapper from closing it, so ownership stays with the caller.
    ManuallyDrop::new(unsafe { File::from_raw_fd(fd) })
}

#[cfg(windows)]
fn borrowed(fd: RawFd) -> ManuallyDrop<File> {
    // SAFETY: `fd` is an open CRT descriptor owned by the caller. `_get_osfhandle` maps it
    // to the underlying handle; `ManuallyDrop` keeps the handle open.
    let handle = unsafe { libc::get_osfhandle(fd) };
    ManuallyDrop::new(unsafe { File::from_raw_handle(handle as _) })
}

fn open(path: &str, flags: i32, mode: u32) -> io::Result<File> {
    let mut options = OpenOptions::new();
    match flags & O_ACCMODE {
        1 => options.write(true),
        2 => options.read(true).write(true),
        _ => options.read(true),
    }
    .create(flags & O_CREAT != 0)
    .truncate(flags & O_TRUNC != 0)
    .append(flags & O_APPEND != 0);
    if flags & O_EXCL != 0 {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        options.mode(mode).custom_flags(flags & !TRANSLATED_FLAGS);
    }
    #[cfg(windows)]
    {
        let _ = mode;
    }
    options.open(path)
}

fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        FileExt::read_at(file, buffer, offset)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(file, buffer, offset)
    }
}

fn write_at(file: &File, buffer: &[u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        FileExt::write_at(file, buffer, offset)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_write(file, buffer, offset)
    }
}

fn read_all_utf8(mut file: &File) -> io::Result<String> {
    let size = file.metadata()?.len();
    if size > MAX_UTF8_FILE_BYTES {
        return Err(io::Error::from_raw_os_error(EFBIG));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "size is bounded by MAX_UTF8_FILE_BYTES (2 GiB - 1), which fits a 32-bit usize"
    )]
    let mut bytes = Vec::with_capacity(size as usize);
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(unix)]
fn encode_stats(metadata: &Metadata) -> String {
    let mut fields = [
        metadata.dev(),
        u64::from(metadata.mode()),
        metadata.nlink(),
        u64::from(metadata.uid()),
        u64::from(metadata.gid()),
        metadata.rdev(),
        metadata.blksize(),
        metadata.ino(),
        metadata.len(),
        metadata.blocks(),
    ]
    .iter()
    .map(u64::to_string)
    .collect::<Vec<String>>();
    let changed = (metadata.ctime(), metadata.ctime_nsec());
    let times = [
        (metadata.atime(), metadata.atime_nsec()),
        (metadata.mtime(), metadata.mtime_nsec()),
        changed,
        changed,
    ];
    for (seconds, nanos) in times {
        fields.push(seconds.to_string());
        fields.push(nanos.to_string());
    }
    fields.join("\0")
}

#[cfg(windows)]
fn encode_stats(metadata: &Metadata) -> String {
    let len = metadata.len();
    let file_type = metadata.file_type();
    let mut mode: u64 = if file_type.is_dir() {
        0o040755
    } else if file_type.is_symlink() {
        0o120777
    } else {
        0o100644
    };
    if metadata.permissions().readonly() {
        mode &= !0o222;
    }
    let (atime, atime_nsec) = unix_time(metadata.accessed());
    let (mtime, mtime_nsec) = unix_time(metadata.modified());
    let (ctime, ctime_nsec) = unix_time(metadata.created());
    // The Windows metadata helpers that expose volume/file ids are unstable;
    // Node's stats tolerate zero for dev/ino and one link.
    let fields = [0, mode, 1, 0, 0, 0, 0, 0, len, len / 512];
    let mut fields: Vec<String> = fields.iter().map(u64::to_string).collect();
    for (seconds, nanos) in [
        (atime, atime_nsec),
        (mtime, mtime_nsec),
        (ctime, ctime_nsec),
        (ctime, ctime_nsec),
    ] {
        fields.push(seconds.to_string());
        fields.push(nanos.to_string());
    }
    fields.join("\0")
}

#[cfg(windows)]
fn unix_time(time: io::Result<SystemTime>) -> (i64, i64) {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or((0, 0), |elapsed| {
            (
                i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
                i64::from(elapsed.subsec_nanos()),
            )
        })
}

#[cfg(unix)]
fn dirent_type(file_type: std::fs::FileType) -> u32 {
    if file_type.is_file() {
        UV_DIRENT_FILE
    } else if file_type.is_dir() {
        UV_DIRENT_DIR
    } else if file_type.is_symlink() {
        UV_DIRENT_LINK
    } else if file_type.is_fifo() {
        UV_DIRENT_FIFO
    } else if file_type.is_socket() {
        UV_DIRENT_SOCKET
    } else if file_type.is_char_device() {
        UV_DIRENT_CHAR
    } else if file_type.is_block_device() {
        UV_DIRENT_BLOCK
    } else {
        UV_DIRENT_UNKNOWN
    }
}

#[cfg(windows)]
fn dirent_type(file_type: std::fs::FileType) -> u32 {
    if file_type.is_symlink() {
        UV_DIRENT_LINK
    } else if file_type.is_dir() {
        UV_DIRENT_DIR
    } else if file_type.is_file() {
        UV_DIRENT_FILE
    } else {
        UV_DIRENT_UNKNOWN
    }
}

/// Byte counts reported back to JS, which types them as 32-bit like Node's fs binding.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the op ABI reports transferred byte counts as u32, matching Node's fs binding"
)]
const fn byte_count(transferred: usize) -> u32 {
    transferred as u32
}

/// File offsets arrive from JS as `f64`; callers have already rejected or clamped negatives.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "every caller proves the offset non-negative first, so the saturating float to integer cast is the conversion libuv performs for JS numbers"
)]
const fn file_offset(position: f64) -> u64 {
    position as u64
}

#[cfg(unix)]
fn directory_builder(mode: u32) -> DirBuilder {
    let mut builder = DirBuilder::new();
    builder.mode(mode);
    builder
}

#[cfg(windows)]
fn directory_builder(_mode: u32) -> DirBuilder {
    DirBuilder::new()
}

fn make_directory(path: &str, mode: u32) -> Result<String, JsErrorBox> {
    let target = Path::new(path);
    match std::fs::metadata(target) {
        Ok(metadata) if metadata.is_dir() => return Ok(String::new()),
        Ok(_) => return Err(errno_error(EEXIST)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(fs_error(error)),
    }

    let mut missing: Vec<&Path> = target
        .ancestors()
        .take_while(|directory| !directory.exists())
        .collect();
    missing.reverse();

    let mut first_created = None;
    for directory in missing {
        if directory.as_os_str().is_empty() {
            continue;
        }
        match directory_builder(mode).create(directory) {
            Ok(()) => {
                if first_created.is_none() {
                    first_created = Some(directory);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(fs_error(error)),
        }
    }
    Ok(first_created
        .map(|directory| directory.to_string_lossy().into_owned())
        .unwrap_or_default())
}

fn random_suffix() -> Result<String, getrandom::Error> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    let mut bytes = [0u8; 6];
    getrandom::fill(&mut bytes)?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect())
}

#[op2(nofast)]
fn op_fs_open(
    state: &OpState,
    #[string] path: &str,
    flags: i32,
    mode: u32,
) -> Result<i32, JsErrorBox> {
    if let Some(mount) = mount(state)
        && mount.stat(path).is_some()
    {
        let readonly =
            flags & O_ACCMODE == 0 && flags & (O_CREAT | O_TRUNC | O_APPEND | O_EXCL) == 0;
        if !readonly {
            return Err(errno_error(EROFS));
        }
        return mount.open(path).ok_or_else(|| errno_error(EIO));
    }
    let file = open(path, flags, mode).map_err(fs_error)?;
    #[cfg(unix)]
    {
        Ok(ManuallyDrop::new(file).as_raw_fd())
    }
    #[cfg(windows)]
    {
        let handle = file.as_raw_handle();
        // SAFETY: the handle belongs to `file`; `_open_osfhandle` transfers its ownership
        // to the returned CRT descriptor.
        let fd = unsafe { libc::open_osfhandle(handle as _, 0) };
        if fd == -1 {
            return Err(fs_error(io::Error::last_os_error()));
        }
        std::mem::forget(file);
        Ok(fd)
    }
}

#[op2]
fn op_fs_close(state: &OpState, fd: RawFd) {
    if VirtualMount::is_virtual_fd(fd) {
        if let Some(mount) = mount(state) {
            mount.close(fd);
        }
        return;
    }
    #[cfg(unix)]
    {
        // SAFETY: real descriptors are owned by JS between open and close, and this op is
        // the only owner taking them back.
        drop(unsafe { File::from_raw_fd(fd) });
    }
    #[cfg(windows)]
    {
        // The CRT descriptor owns the handle; `_close` releases both.
        // SAFETY: the descriptor is owned by JS between open and close.
        unsafe { libc::close(fd) };
    }
}

#[op2]
fn op_fs_read(
    state: &OpState,
    fd: RawFd,
    #[buffer] buffer: &mut [u8],
    position: f64,
) -> Result<u32, JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        return mount(state)
            .and_then(|mount| mount.read_at(fd, buffer, position))
            .map(byte_count)
            .ok_or_else(|| errno_error(EIO));
    }
    let file = borrowed(fd);
    let read = if position < 0.0 {
        (&*file).read(buffer)
    } else {
        read_at(&file, buffer, file_offset(position))
    };
    read.map(byte_count).map_err(fs_error)
}

#[op2]
#[string]
fn op_fs_read_all_utf8(state: &OpState, fd: RawFd) -> Result<String, JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        let bytes = mount(state)
            .and_then(|mount| mount.read_to_end(fd))
            .ok_or_else(|| errno_error(EIO))?;
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    read_all_utf8(&borrowed(fd)).map_err(fs_error)
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_fs_read_file_async(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    flags: i32,
) -> Result<Box<[u8]>, JsErrorBox> {
    let virtual_contents = {
        let state = state.borrow();
        state
            .try_borrow::<VirtualMount>()
            .and_then(|mount| mount.read(&path))
    };
    if let Some(contents) = virtual_contents {
        return Ok(contents.into_boxed_slice());
    }
    tokio::task::spawn_blocking(move || -> Result<Box<[u8]>, JsErrorBox> {
        let file = open(&path, flags, 0o666).map_err(fs_error)?;
        let mut bytes = Vec::new();
        (&file).read_to_end(&mut bytes).map_err(fs_error)?;
        Ok(bytes.into_boxed_slice())
    })
    .await
    .map_err(|_| errno_error(EIO))?
}

#[op2(async(lazy))]
#[buffer]
async fn op_fs_read_fd_async(fd: RawFd) -> Result<Box<[u8]>, JsErrorBox> {
    let file = borrowed(fd).try_clone().map_err(fs_error)?;
    tokio::task::spawn_blocking(move || -> Result<Box<[u8]>, JsErrorBox> {
        let mut bytes = Vec::new();
        (&file).read_to_end(&mut bytes).map_err(fs_error)?;
        Ok(bytes.into_boxed_slice())
    })
    .await
    .map_err(|_| errno_error(EIO))?
}

#[op2(async(lazy))]
async fn op_fs_write_file_async(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    flags: i32,
    mode: u32,
    #[buffer] data: JsBuffer,
) -> Result<(), JsErrorBox> {
    let is_virtual = {
        let state = state.borrow();
        state
            .try_borrow::<VirtualMount>()
            .is_some_and(|mount| mount.stat(&path).is_some())
    };
    if is_virtual {
        return Err(errno_error(EROFS));
    }
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || -> Result<(), JsErrorBox> {
        let file = open(&path, flags, mode).map_err(fs_error)?;
        (&file).write_all(&data).map_err(fs_error)
    })
    .await
    .map_err(|_| errno_error(EIO))?
}

#[op2]
fn op_fs_write(fd: RawFd, #[buffer] buffer: &[u8], position: f64) -> Result<u32, JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        return Err(errno_error(EROFS));
    }
    let file = borrowed(fd);
    let written = if position < 0.0 {
        (&*file).write(buffer)
    } else {
        write_at(&file, buffer, file_offset(position))
    };
    written.map(byte_count).map_err(fs_error)
}

#[op2]
fn op_fs_write_all_utf8(fd: RawFd, #[string] data: &str) -> Result<(), JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        return Err(errno_error(EROFS));
    }
    let file = borrowed(fd);
    (&*file).write_all(data.as_bytes()).map_err(fs_error)
}

#[op2]
fn op_fs_fsync(fd: RawFd) -> Result<(), JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        return Ok(());
    }
    borrowed(fd).sync_all().map_err(fs_error)
}

#[op2]
fn op_fs_fdatasync(fd: RawFd) -> Result<(), JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        return Ok(());
    }
    borrowed(fd).sync_data().map_err(fs_error)
}

#[op2]
fn op_fs_ftruncate(fd: RawFd, length: f64) -> Result<(), JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        return Err(errno_error(EROFS));
    }
    borrowed(fd)
        .set_len(file_offset(length.max(0.0)))
        .map_err(fs_error)
}

#[op2]
#[string]
fn op_fs_fstat(state: &OpState, fd: RawFd) -> Result<String, JsErrorBox> {
    if VirtualMount::is_virtual_fd(fd) {
        let len = mount(state)
            .and_then(|mount| mount.len(fd))
            .ok_or_else(|| errno_error(EIO))?;
        return Ok(virtual_mount::encode_stat(&thinpack_core::VirtualStat {
            len,
            is_file: true,
            is_dir: false,
        }));
    }
    borrowed(fd)
        .metadata()
        .map(|metadata| encode_stats(&metadata))
        .map_err(fs_error)
}

#[op2]
#[string]
fn op_fs_stat(state: &OpState, #[string] path: &str) -> Result<String, JsErrorBox> {
    if let Some(stat) = mount(state).and_then(|mount| mount.stat(path)) {
        return Ok(virtual_mount::encode_stat(&stat));
    }
    std::fs::metadata(path)
        .map(|metadata| encode_stats(&metadata))
        .map_err(fs_error)
}

#[op2]
#[string]
fn op_fs_lstat(state: &OpState, #[string] path: &str) -> Result<String, JsErrorBox> {
    if let Some(stat) = mount(state).and_then(|mount| mount.stat(path)) {
        return Ok(virtual_mount::encode_stat(&stat));
    }
    std::fs::symlink_metadata(path)
        .map(|metadata| encode_stats(&metadata))
        .map_err(fs_error)
}

#[op2]
#[string]
fn op_fs_readdir(
    state: &OpState,
    #[string] path: &str,
    with_types: bool,
) -> Result<String, JsErrorBox> {
    if let Some(entries) = mount(state).and_then(|mount| mount.entries(path)) {
        return Ok(virtual_mount::encode_entries(&entries, with_types));
    }
    let directory = std::fs::read_dir(path).map_err(fs_error)?;
    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    for entry in directory {
        let entry = entry.map_err(fs_error)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = if with_types {
            let file_type = entry.file_type().map_err(fs_error)?;
            Some(dirent_type(file_type).to_string())
        } else {
            None
        };
        entries.push((name, kind));
    }
    // Node returns readdir(3) order (directory order); this sorts byte-wise instead
    // so results are deterministic across filesystems. The divergence is
    // observable in the entry order.
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut fields: Vec<String> = Vec::new();
    for (name, kind) in entries {
        fields.push(name);
        if let Some(kind) = kind {
            fields.push(kind);
        }
    }
    Ok(fields.join("\0"))
}

#[op2]
#[string]
fn op_fs_mkdir(#[string] path: &str, mode: u32, recursive: bool) -> Result<String, JsErrorBox> {
    if recursive {
        return make_directory(path, mode);
    }
    directory_builder(mode)
        .create(path)
        .map(|()| String::new())
        .map_err(fs_error)
}

#[op2(nofast)]
fn op_fs_rmdir(#[string] path: &str) -> Result<(), JsErrorBox> {
    std::fs::remove_dir(path).map_err(fs_error)
}

#[op2(nofast)]
fn op_fs_rm(#[string] path: &str, recursive: bool) -> Result<(), JsErrorBox> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(fs_error(error)),
    };
    if metadata.is_dir() && !recursive {
        return Err(errno_error(EISDIR));
    }
    let result = if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(fs_error)
}

#[op2(nofast)]
fn op_fs_unlink(#[string] path: &str) -> Result<(), JsErrorBox> {
    std::fs::remove_file(path).map_err(fs_error)
}

#[op2(nofast)]
fn op_fs_rename(#[string] old_path: &str, #[string] new_path: &str) -> Result<(), JsErrorBox> {
    std::fs::rename(old_path, new_path).map_err(fs_error)
}

#[op2(nofast)]
fn op_fs_copy_file(
    #[string] source: &str,
    #[string] destination: &str,
    mode: u32,
) -> Result<(), JsErrorBox> {
    if mode & COPYFILE_FICLONE_FORCE != 0 {
        return Err(errno_error(ENOTSUP));
    }
    let mut source = File::open(source).map_err(fs_error)?;
    let mut options = OpenOptions::new();
    options.write(true);
    if mode & COPYFILE_EXCL != 0 {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    let mut destination = options.open(destination).map_err(fs_error)?;
    io::copy(&mut source, &mut destination)
        .map(|_| ())
        .map_err(fs_error)
}

#[cfg(unix)]
fn chmod(path: &str, mode: u32) -> io::Result<()> {
    std::fs::set_permissions(path, Permissions::from_mode(mode))
}

#[cfg(windows)]
fn chmod(path: &str, mode: u32) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    // Windows chmod does not follow symlinks; resolve first, like Deno's std_fs.
    let resolved = host_path(std::fs::canonicalize(path)?);
    let wide: Vec<u16> = resolved.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated.
    if unsafe { libc::wchmod(wide.as_ptr(), mode.cast_signed()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[op2(nofast)]
fn op_fs_chmod(#[string] path: &str, mode: u32) -> Result<(), JsErrorBox> {
    chmod(path, mode).map_err(fs_error)
}

fn system_time(seconds: f64) -> Result<SystemTime, JsErrorBox> {
    let magnitude = Duration::try_from_secs_f64(seconds.abs()).map_err(|_| errno_error(EINVAL))?;
    let time = if seconds.is_sign_negative() {
        UNIX_EPOCH.checked_sub(magnitude)
    } else {
        UNIX_EPOCH.checked_add(magnitude)
    };
    time.ok_or_else(|| errno_error(EINVAL))
}

fn file_times(atime: f64, mtime: f64) -> Result<FileTimes, JsErrorBox> {
    Ok(FileTimes::new()
        .set_accessed(system_time(atime)?)
        .set_modified(system_time(mtime)?))
}

#[op2(nofast)]
fn op_fs_utimes(#[string] path: &str, atime: f64, mtime: f64) -> Result<(), JsErrorBox> {
    std::fs::set_times(path, file_times(atime, mtime)?).map_err(fs_error)
}

#[op2]
#[string]
fn op_fs_readlink(#[string] path: &str) -> Result<String, JsErrorBox> {
    std::fs::read_link(path)
        .map(|target: PathBuf| target.to_string_lossy().into_owned())
        .map_err(fs_error)
}

#[op2(nofast)]
fn op_fs_symlink(#[string] target: &str, #[string] path: &str) -> Result<(), JsErrorBox> {
    symlink(target, path).map_err(fs_error)
}

#[cfg(unix)]
fn symlink(target: &str, path: &str) -> io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(windows)]
fn symlink(target: &str, path: &str) -> io::Result<()> {
    if Path::new(target).is_dir() {
        std::os::windows::fs::symlink_dir(target, path)
    } else {
        std::os::windows::fs::symlink_file(target, path)
    }
}

#[op2]
#[string]
fn op_fs_mkdtemp(#[string] prefix: &str) -> Result<String, JsErrorBox> {
    for _ in 0..16 {
        let candidate = format!("{prefix}{}", random_suffix().map_err(|_| errno_error(EIO))?);
        match directory_builder(0o700).create(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(fs_error(error)),
        }
    }
    Err(errno_error(EEXIST))
}

#[op2]
#[string]
fn op_fs_realpath(state: &OpState, #[string] path: &str) -> Result<String, JsErrorBox> {
    if mount(state).is_some_and(|mount| mount.stat(path).is_some()) {
        return Ok(path.to_owned());
    }
    std::fs::canonicalize(path)
        .map(host_path)
        .map(|resolved: PathBuf| resolved.to_string_lossy().into_owned())
        .map_err(fs_error)
}

#[cfg(not(windows))]
const fn host_path(path: PathBuf) -> PathBuf {
    path
}

#[cfg(windows)]
fn host_path(path: PathBuf) -> PathBuf {
    // `canonicalize` prefixes drive paths with the verbatim marker; Node's
    // `realpath` returns the plain path.
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\")
        && !rest.starts_with("UNC\\")
    {
        return PathBuf::from(rest);
    }
    path
}

#[op2(nofast)]
fn op_fs_access(state: &OpState, #[string] path: &str, mode: u32) -> Result<(), JsErrorBox> {
    if let Some(stat) = mount(state).and_then(|mount| mount.stat(path)) {
        let wants_write = mode & W_OK != 0 || stat.is_dir && mode & X_OK != 0;
        return if wants_write {
            Err(errno_error(EROFS))
        } else {
            Ok(())
        };
    }
    check_access(path, mode).map_err(fs_error)
}

#[cfg(unix)]
fn check_access(path: &str, mode: u32) -> io::Result<()> {
    let path = CString::new(path).map_err(|_| io::Error::from_raw_os_error(EINVAL))?;
    #[expect(
        clippy::cast_possible_wrap,
        reason = "access(2) takes the R_OK|W_OK|X_OK|F_OK bitmask, which never comes close to i32::MAX"
    )]
    let mode = mode as libc::c_int;
    // SAFETY: `path` is NUL-terminated; access(2) reads only it and the mode bitmask.
    if unsafe { libc::access(path.as_ptr(), mode) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn check_access(path: &str, mode: u32) -> io::Result<()> {
    let metadata = std::fs::metadata(path)?;
    if mode & W_OK != 0 && metadata.permissions().readonly() {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    Ok(())
}

#[op2(nofast)]
fn op_fs_exists(state: &OpState, #[string] path: &str) -> bool {
    mount(state).is_some_and(|mount| mount.stat(path).is_some()) || Path::new(path).exists()
}

#[op2(fast)]
fn op_fs_link(#[string] existing: &str, #[string] new_path: &str) -> Result<(), JsErrorBox> {
    std::fs::hard_link(existing, new_path).map_err(fs_error)
}

#[cfg(unix)]
#[op2(fast)]
fn op_fs_chown(#[string] path: &str, #[smi] uid: i32, #[smi] gid: i32) -> Result<(), JsErrorBox> {
    let (uid, gid) = ownership(uid, gid);
    std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(fs_error)
}

#[cfg(unix)]
#[op2(fast)]
fn op_fs_lchown(#[string] path: &str, #[smi] uid: i32, #[smi] gid: i32) -> Result<(), JsErrorBox> {
    let (uid, gid) = ownership(uid, gid);
    std::os::unix::fs::lchown(path, Some(uid), Some(gid)).map_err(fs_error)
}

#[cfg(unix)]
#[op2(fast)]
fn op_fs_fchown(#[smi] fd: i32, #[smi] uid: i32, #[smi] gid: i32) -> Result<(), JsErrorBox> {
    let (uid, gid) = ownership(uid, gid);
    // SAFETY: fchown(2) reads only the descriptor and the two id values.
    let result = unsafe { libc::fchown(fd, uid, gid) };
    if result == -1 {
        return Err(fs_error(io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(unix)]
/// Node forwards -1 to mean "leave the owner unchanged", which chown(2) spells as `u32::MAX`.
#[expect(
    clippy::cast_sign_loss,
    reason = "-1 is the documented chown(2) sentinel and its two's-complement reinterpretation is exactly the value the syscall expects"
)]
const fn ownership(uid: i32, gid: i32) -> (u32, u32) {
    (uid as u32, gid as u32)
}

#[cfg(windows)]
#[op2(fast)]
fn op_fs_chown(
    #[string] _path: &str,
    #[smi] _uid: i32,
    #[smi] _gid: i32,
) -> Result<(), JsErrorBox> {
    Err(errno_error(ENOTSUP))
}

#[cfg(windows)]
#[op2(fast)]
fn op_fs_lchown(
    #[string] _path: &str,
    #[smi] _uid: i32,
    #[smi] _gid: i32,
) -> Result<(), JsErrorBox> {
    Err(errno_error(ENOTSUP))
}

#[cfg(windows)]
#[op2(fast)]
fn op_fs_fchown(#[smi] _fd: i32, #[smi] _uid: i32, #[smi] _gid: i32) -> Result<(), JsErrorBox> {
    Err(errno_error(ENOTSUP))
}

#[cfg(unix)]
#[op2]
#[string]
fn op_fs_statfs(#[string] path: &str) -> Result<String, JsErrorBox> {
    let path = CString::new(path).map_err(|_| errno_error(EINVAL))?;
    // SAFETY: libc::statvfs is a plain C struct of integers, so all-zero is a valid value.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stats` is a writable, correctly sized statvfs.
    if unsafe { libc::statvfs(path.as_ptr(), &raw mut stats) } == -1 {
        return Err(fs_error(io::Error::last_os_error()));
    }
    // Linux reports a filesystem magic here; the BSDs, including macOS, have no equivalent.
    // musl's `statvfs` has no `f_type`, and glibc's is a later addition, so the magic comes
    // from `statfs`, which is where Node reads it too (`uv_fs_statfs`).
    #[cfg(target_os = "linux")]
    let type_number = {
        // SAFETY: libc::statfs is a plain C struct of integers, so all-zero is a valid value.
        let mut fs_stats: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated and `fs_stats` is a writable, correctly sized statfs.
        if unsafe { libc::statfs(path.as_ptr(), &raw mut fs_stats) } == -1 {
            return Err(fs_error(io::Error::last_os_error()));
        }
        fs_stats.f_type
    };
    #[cfg(not(target_os = "linux"))]
    let type_number = 0;
    Ok([
        type_number.to_string(),
        stats.f_bsize.to_string(),
        stats.f_frsize.to_string(),
        stats.f_blocks.to_string(),
        stats.f_bfree.to_string(),
        stats.f_bavail.to_string(),
        stats.f_files.to_string(),
        stats.f_ffree.to_string(),
    ]
    .join("\0"))
}

#[cfg(windows)]
#[op2]
#[string]
fn op_fs_statfs(#[string] path: &str) -> Result<String, JsErrorBox> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceW;

    // Deno's std_fs uses the filesystem root and GetDiskFreeSpaceW, and retries
    // once because filters can transiently fail (libuv does the same).
    let root = Path::new(path)
        .canonicalize()
        .map_err(fs_error)?
        .ancestors()
        .last()
        .ok_or_else(|| errno_error(ENOENT))?
        .to_path_buf();
    let mut root: Vec<u16> = root.as_os_str().encode_wide().collect();
    root.push(0);
    let mut sectors_per_cluster = 0u32;
    let mut bytes_per_sector = 0u32;
    let mut available_clusters = 0u32;
    let mut total_clusters = 0u32;
    let mut code = 0i32;
    let mut retries = 0;
    while code == 0 && retries < 2 {
        // SAFETY: root is NUL-terminated and every out-pointer is valid.
        code = unsafe {
            GetDiskFreeSpaceW(
                root.as_ptr(),
                &mut sectors_per_cluster,
                &mut bytes_per_sector,
                &mut available_clusters,
                &mut total_clusters,
            )
        };
        retries += 1;
    }
    if code == 0 {
        return Err(fs_error(io::Error::last_os_error()));
    }
    let block_size = u64::from(bytes_per_sector) * u64::from(sectors_per_cluster);
    Ok([
        0,
        block_size,
        block_size,
        u64::from(total_clusters),
        u64::from(available_clusters),
        u64::from(available_clusters),
        0,
        0,
    ]
    .iter()
    .map(u64::to_string)
    .collect::<Vec<String>>()
    .join("\0"))
}

#[cfg(unix)]
#[cfg(target_vendor = "apple")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "macOS mode_t is u16; fchmod(2) only reads the low permission bits, and Node truncates the same way"
)]
const fn fchmod_mode(mode: u32) -> libc::mode_t {
    mode as libc::mode_t
}

#[cfg(unix)]
#[cfg(not(target_vendor = "apple"))]
const fn fchmod_mode(mode: u32) -> libc::mode_t {
    mode
}

#[cfg(unix)]
#[op2(fast)]
fn op_fs_fchmod(#[smi] fd: i32, #[smi] mode: u32) -> Result<(), JsErrorBox> {
    // SAFETY: fchmod(2) reads only the descriptor and the mode bits.
    if unsafe { libc::fchmod(fd, fchmod_mode(mode)) } == -1 {
        return Err(fs_error(io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(windows)]
#[op2(fast)]
fn op_fs_fchmod(#[smi] fd: i32, #[smi] mode: u32) -> Result<(), JsErrorBox> {
    let file = borrowed(fd);
    let mut permissions = file.metadata().map_err(fs_error)?.permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    file.set_permissions(permissions).map_err(fs_error)
}

#[op2(fast)]
fn op_fs_futimes(#[smi] fd: i32, atime: f64, mtime: f64) -> Result<(), JsErrorBox> {
    let file = borrowed(fd);
    file.set_times(file_times(atime, mtime)?).map_err(fs_error)
}

#[op2(fast)]
fn op_fs_lutimes(#[string] path: &str, atime: f64, mtime: f64) -> Result<(), JsErrorBox> {
    std::fs::set_times_nofollow(path, file_times(atime, mtime)?).map_err(fs_error)
}

struct Watch {
    events: Arc<Mutex<Vec<String>>>,
    notify: Arc<tokio::sync::Notify>,
    closed: Arc<AtomicBool>,
    wake: Arc<AtomicBool>,
    _watcher: notify::RecommendedWatcher,
}

fn watch_registry() -> &'static Mutex<HashMap<u32, Watch>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u32, Watch>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

static WATCHES: thinpack_core::IdCounter = thinpack_core::IdCounter::new();

#[op2(fast)]
fn op_fs_watch_start(#[string] path: &str, recursive: bool) -> Result<u32, JsErrorBox> {
    let watch = start_watch(path, recursive)?;
    let id = WATCHES.next();
    watch_registry().lock().unwrap().insert(id, watch);
    Ok(id)
}

fn start_watch(path: &str, recursive: bool) -> Result<Watch, JsErrorBox> {
    use notify::Watcher as _;

    let events = Arc::new(Mutex::new(Vec::new()));
    let handler_events = Arc::clone(&events);
    let notify = Arc::new(tokio::sync::Notify::new());
    let handler_notify = Arc::clone(&notify);
    let watched_name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        let Some(kind) = event_kind(event.kind) else {
            return;
        };
        let mut batch: Vec<String> = event
            .paths
            .iter()
            .filter_map(|changed| changed.file_name())
            .map(|name| format!("{kind}\0{}", name.to_string_lossy()))
            .collect();
        if batch.is_empty() && !watched_name.is_empty() {
            batch.push(format!("{kind}\0{watched_name}"));
        }
        if !batch.is_empty() {
            handler_events.lock().unwrap().extend(batch);
            handler_notify.notify_one();
        }
    })
    .map_err(watcher_error)?;

    let mode = if recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    };
    watcher
        .watch(Path::new(path), mode)
        .map_err(watcher_error)?;

    Ok(Watch {
        events,
        notify,
        closed: Arc::new(AtomicBool::new(false)),
        wake: Arc::new(AtomicBool::new(false)),
        _watcher: watcher,
    })
}

const fn event_kind(kind: notify::EventKind) -> Option<&'static str> {
    use notify::EventKind;
    use notify::event::{ModifyKind, RenameMode};

    match kind {
        // Access events (opens and closes) are not changes. Inotify reports a rename as
        // separate `From` and `To` events and then repeats both together as `Both`; the
        // first two already carry every changed name.
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => None,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) => {
            Some("rename")
        }
        EventKind::Modify(_) | EventKind::Any | EventKind::Other => Some("change"),
    }
}

fn watcher_error(error: notify::Error) -> JsErrorBox {
    let detail = error.to_string();
    let errno = match error.kind {
        notify::ErrorKind::PathNotFound => ENOENT,
        notify::ErrorKind::MaxFilesWatch => ENOSPC,
        notify::ErrorKind::InvalidConfig(_) | notify::ErrorKind::WatchNotFound => EINVAL,
        notify::ErrorKind::Io(error) => return fs_error(error),
        // Everything else — `Generic`, the FSEvents `WatchStartFailed`, and whatever notify adds
        // next — is an opaque backend failure with no errno of its own.
        _ => return fs_error(io::Error::other(detail)),
    };
    fs_error(io::Error::from_raw_os_error(errno))
}

fn take_events(events: &Mutex<Vec<String>>) -> Option<String> {
    let mut events = events.lock().unwrap();
    if events.is_empty() {
        return None;
    }
    Some(std::mem::take(&mut *events).join("\0"))
}

#[op2]
#[string]
fn op_fs_watch_take(#[smi] id: u32) -> String {
    let registry = watch_registry().lock().unwrap();
    let Some(watch) = registry.get(&id) else {
        return String::new();
    };
    take_events(&watch.events).unwrap_or_default()
}

/// Wait for the watcher's next batch. A close or a `wake` (the JS side switching
/// between the referenced async wait and the unref'd timer) returns an empty string.
#[op2(async(lazy), fast)]
#[string]
async fn op_fs_watch_wait(#[smi] id: u32) -> String {
    let (events, notify, closed, wake) = {
        let registry = watch_registry().lock().unwrap();
        let Some(watch) = registry.get(&id) else {
            return String::new();
        };
        (
            Arc::clone(&watch.events),
            Arc::clone(&watch.notify),
            Arc::clone(&watch.closed),
            Arc::clone(&watch.wake),
        )
    };
    loop {
        if let Some(payload) = take_events(&events) {
            return payload;
        }
        if closed.load(Ordering::Relaxed) || wake.swap(false, Ordering::Relaxed) {
            return String::new();
        }
        notify.notified().await;
    }
}

#[op2(fast)]
fn op_fs_watch_wake(#[smi] id: u32) {
    let registry = watch_registry().lock().unwrap();
    let Some(watch) = registry.get(&id) else {
        return;
    };
    watch.wake.store(true, Ordering::Relaxed);
    watch.notify.notify_one();
}

#[op2(fast)]
fn op_fs_watch_close(#[smi] id: u32) {
    let watch = watch_registry().lock().unwrap().remove(&id);
    if let Some(watch) = watch {
        watch.closed.store(true, Ordering::Relaxed);
        watch.notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::random_suffix;

    #[test]
    fn suffix_is_random_and_alphanumeric() {
        let first = random_suffix().expect("the operating system provides random bytes");
        let second = random_suffix().expect("the operating system provides random bytes");
        assert_eq!(first.len(), 6);
        assert!(first.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        assert_ne!(first, second);
    }
}
