//! Local side of a transfer: safely unpacking a downloaded tarball into a staging directory, and
//! packing the mirror back into a tarball whose headers carry the remote ownership and modes.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, Entry, EntryType, Header};

use super::manifest::{EntryKind, EntryMeta, Manifest};
use super::paths::{is_git_metadata_name, local_path_for, split_parent_name};
use super::{MAX_ENTRIES, MAX_EXTRACTED_BYTES, WarpSyncError};

const COPY_BUFFER_BYTES: usize = 64 * 1024;
const BYTES_PER_MIB: u64 = 1024 * 1024;
/// Modes of new entries whose local copy has no permission bits to go by.
const NEW_FILE_MODE: u32 = 0o644;
const NEW_DIR_MODE: u32 = 0o755;

/// What a new file keeps of its local mode: the `rwx` bits, and setuid/setgid so that they can be
/// refused rather than silently dropped.
const NEW_FILE_MODE_MASK: u32 = 0o6777;
const NEW_DIR_MODE_MASK: u32 = 0o777;

/// Permission bits the local user always keeps, so that mirrored files stay editable and
/// directories stay traversable. The remote mode is preserved in the manifest instead.
const LOCAL_FILE_MODE_FLOOR: u32 = 0o600;
const LOCAL_DIR_MODE_FLOOR: u32 = 0o700;

/// Local copies never get group or other write access, nor setuid, setgid or sticky bits, no
/// matter what the remote mode is.
const LOCAL_MODE_MASK: u32 = 0o755;

const SETUID_SETGID_BITS: u32 = 0o6000;

/// Room for the tar headers and padding of one entry when bounding the decompressed stream.
const TAR_ENTRY_OVERHEAD_BYTES: u64 = 1024;

#[derive(Debug, Clone, Copy)]
pub struct ExtractLimits {
    pub max_bytes: u64,
    pub max_entries: usize,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_EXTRACTED_BYTES,
            max_entries: MAX_ENTRIES,
        }
    }
}

/// Why an archive entry was left out of the mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Symlink,
    Hardlink,
    /// Device, pipe or other entry that is not a regular file or directory.
    Special,
    NonUtf8Name,
    /// A `.git` directory or file, which is never mirrored.
    GitMetadata,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::Symlink => "symbolic link",
            Self::Hardlink => "hard link",
            Self::Special => "special file",
            Self::NonUtf8Name => "name is not valid UTF-8",
            Self::GitMetadata => "Git metadata",
        };
        f.write_str(description)
    }
}

#[derive(Debug, Default)]
pub struct ExtractReport {
    /// Metadata of every extracted entry, keyed by absolute remote path.
    pub entries: BTreeMap<String, EntryMeta>,
    pub skipped: Vec<(String, SkipReason)>,
    pub files: usize,
    pub dirs: usize,
    pub total_bytes: u64,
}

/// Result of packing a local mirror subtree for upload.
#[derive(Debug)]
pub struct UploadArchive {
    /// The gzipped tarball.
    pub bytes: Vec<u8>,
    pub files: usize,
    pub dirs: usize,
    /// Total uncompressed size of the files.
    pub content_bytes: u64,
    /// Remote paths that do not exist in the manifest yet.
    pub new_files: Vec<String>,
    /// Remote paths that the manifest knows but the mirror no longer has. They are not removed
    /// from the remote host.
    pub missing_locally: Vec<String>,
}

/// Unpacks `tgz`, which must contain only `name` (as produced by `tar -C parent ./name`), into
/// `staging`. Only regular files and directories are created; the caller removes `staging` on
/// failure.
pub fn extract_download(
    tgz: &[u8],
    name: &str,
    remote_parent: &str,
    staging: &Path,
) -> Result<ExtractReport, WarpSyncError> {
    extract_download_with_limits(tgz, name, remote_parent, staging, ExtractLimits::default())
}

pub fn extract_download_with_limits(
    tgz: &[u8],
    name: &str,
    remote_parent: &str,
    staging: &Path,
    limits: ExtractLimits,
) -> Result<ExtractReport, WarpSyncError> {
    fs::create_dir_all(staging).map_err(|err| local_io("create", staging, &err))?;

    let mut extractor = Extractor {
        name,
        remote_parent,
        staging,
        limits,
        report: ExtractReport::default(),
    };
    let mut archive = Archive::new(BoundedReader::new(
        GzDecoder::new(tgz),
        stream_limit(&limits),
    ));
    let mut seen = 0usize;
    for entry in archive.entries().map_err(corrupt)? {
        let mut entry = entry.map_err(corrupt)?;
        seen += 1;
        if seen > limits.max_entries {
            return Err(too_large(format!(
                "the archive has more than {} entries",
                limits.max_entries
            )));
        }
        extractor.handle(&mut entry)?;
    }
    if seen == 0 {
        return Err(WarpSyncError::CorruptArchive(
            "the archive is empty".to_owned(),
        ));
    }
    verify_gzip_trailer(archive.into_inner())?;
    Ok(extractor.report)
}

/// Upper bound on the whole decompressed stream, including entries that are skipped and so never
/// count against `max_bytes`.
fn stream_limit(limits: &ExtractLimits) -> u64 {
    let overhead = TAR_ENTRY_OVERHEAD_BYTES.saturating_mul(limits.max_entries as u64);
    limits.max_bytes.saturating_add(overhead)
}

/// Reads to the end of the gzip stream so that its CRC is checked; tar itself stops at the end
/// marker, before the trailer.
fn verify_gzip_trailer(mut reader: BoundedReader<GzDecoder<&[u8]>>) -> Result<(), WarpSyncError> {
    io::copy(&mut reader, &mut io::sink()).map_err(corrupt)?;
    Ok(())
}

/// Fails once more than `remaining` bytes have been read, so that a small archive cannot expand
/// without bound.
struct BoundedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> BoundedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        match self.remaining.checked_sub(read as u64) {
            Some(remaining) => {
                self.remaining = remaining;
                Ok(read)
            }
            None => Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "the archive expands beyond the allowed size",
            )),
        }
    }
}

struct Extractor<'a> {
    name: &'a str,
    remote_parent: &'a str,
    staging: &'a Path,
    limits: ExtractLimits,
    report: ExtractReport,
}

impl Extractor<'_> {
    fn handle<R: Read>(&mut self, entry: &mut Entry<'_, R>) -> Result<(), WarpSyncError> {
        if let Some(is_git_root) = git_metadata(&entry.path_bytes()) {
            // Only the `.git` entry itself is reported; its contents follow it in the archive.
            return if is_git_root {
                self.skip(entry, SkipReason::GitMetadata)
            } else {
                Ok(())
            };
        }
        let kind = match entry.header().entry_type() {
            EntryType::Regular => EntryKind::File,
            EntryType::Directory => EntryKind::Dir,
            EntryType::Symlink => return self.skip(entry, SkipReason::Symlink),
            EntryType::Link => return self.skip(entry, SkipReason::Hardlink),
            // The enum is non-exhaustive upstream; everything else is a device, pipe, or
            // extension header that has no place in the mirror.
            _ => return self.skip(entry, SkipReason::Special),
        };

        let relative = self.relative_path(entry)?;
        let Some(relative_str) = relative.to_str() else {
            return self.skip(entry, SkipReason::NonUtf8Name);
        };
        let remote_path = self.remote_path(relative_str);
        let local_path = self.staging.join(&relative);

        let meta = match kind {
            EntryKind::Dir => {
                fs::create_dir_all(&local_path)
                    .map_err(|err| local_io("create", &local_path, &err))?;
                set_local_mode(&local_path, entry_mode(entry)?, LOCAL_DIR_MODE_FLOOR)?;
                self.report.dirs += 1;
                entry_meta(entry.header(), kind, None, None)?
            }
            EntryKind::File => {
                let size = entry.size();
                self.reserve_bytes(size)?;
                let sha256 = write_file(entry, &local_path)?;
                set_local_mode(&local_path, entry_mode(entry)?, LOCAL_FILE_MODE_FLOOR)?;
                self.report.files += 1;
                entry_meta(entry.header(), kind, Some(size), Some(sha256))?
            }
        };
        self.report.entries.insert(remote_path, meta);
        Ok(())
    }

    fn skip<R: Read>(
        &mut self,
        entry: &Entry<'_, R>,
        reason: SkipReason,
    ) -> Result<(), WarpSyncError> {
        let path = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        self.report.skipped.push((path, reason));
        Ok(())
    }

    /// The entry's path inside `name`, as a sequence of plain components. Anything that could
    /// point outside the staging directory, or outside `name`, is an error.
    fn relative_path<R: Read>(&self, entry: &Entry<'_, R>) -> Result<PathBuf, WarpSyncError> {
        let path = entry.path().map_err(corrupt)?;
        let mut relative = PathBuf::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => relative.push(part),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(WarpSyncError::UnexpectedArchiveEntry(
                        path.display().to_string(),
                    ));
                }
            }
        }
        let top_level = relative.components().next().map(Component::as_os_str);
        if top_level != Some(OsStr::new(self.name)) {
            return Err(WarpSyncError::UnexpectedArchiveEntry(
                path.display().to_string(),
            ));
        }
        Ok(relative)
    }

    fn remote_path(&self, relative: &str) -> String {
        let parent = self.remote_parent.trim_end_matches('/');
        format!("{parent}/{relative}")
    }

    fn reserve_bytes(&mut self, size: u64) -> Result<(), WarpSyncError> {
        let total = self.report.total_bytes.saturating_add(size);
        if total > self.limits.max_bytes {
            return Err(too_large(format!(
                "the archive expands to more than {} MiB",
                self.limits.max_bytes / BYTES_PER_MIB
            )));
        }
        self.report.total_bytes = total;
        Ok(())
    }
}

/// Whether an archive path is inside Git metadata: `Some(true)` for the `.git` entry itself,
/// `Some(false)` for anything below it.
fn git_metadata(path: &[u8]) -> Option<bool> {
    let components: Vec<&[u8]> = path
        .split(|byte| *byte == b'/')
        .filter(|component| !component.is_empty() && *component != b".")
        .collect();
    let position = components
        .iter()
        .position(|component| is_git_metadata_name(&String::from_utf8_lossy(component)))?;
    Some(position + 1 == components.len())
}

/// Copies the entry to `dest`, returning the hex SHA-256 of its contents.
fn write_file<R: Read>(entry: &mut Entry<'_, R>, dest: &Path) -> Result<String, WarpSyncError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| local_io("create", parent, &err))?;
    }
    let mut file = File::create(dest).map_err(|err| local_io("create", dest, &err))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    loop {
        let read = entry.read(&mut buffer).map_err(corrupt)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .map_err(|err| local_io("write", dest, &err))?;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn entry_mode<R: Read>(entry: &Entry<'_, R>) -> Result<u32, WarpSyncError> {
    entry.header().mode().map_err(corrupt)
}

fn entry_meta(
    header: &Header,
    kind: EntryKind,
    size: Option<u64>,
    sha256: Option<String>,
) -> Result<EntryMeta, WarpSyncError> {
    let id = |value: io::Result<u64>| -> Result<u32, WarpSyncError> {
        u32::try_from(value.map_err(corrupt)?)
            .map_err(|_| WarpSyncError::CorruptArchive("owner id out of range".to_owned()))
    };
    Ok(EntryMeta {
        kind,
        mode: header.mode().map_err(corrupt)? & 0o7777,
        uid: id(header.uid())?,
        gid: id(header.gid())?,
        uname: header
            .username()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_owned(),
        gname: header
            .groupname()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_owned(),
        mtime: header.mtime().map_err(corrupt)?,
        size,
        sha256,
    })
}

#[cfg(unix)]
fn set_local_mode(path: &Path, remote_mode: u32, floor: u32) -> Result<(), WarpSyncError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = (remote_mode & LOCAL_MODE_MASK) | floor;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|err| local_io("set permissions on", path, &err))
}

#[cfg(not(unix))]
fn set_local_mode(_path: &Path, _remote_mode: u32, _floor: u32) -> Result<(), WarpSyncError> {
    Ok(())
}

/// A regular file or directory found in the local mirror.
struct LocalItem {
    path: PathBuf,
    /// Path relative to the synced root, `/`-separated; empty for the root itself.
    relative: String,
    is_dir: bool,
    len: u64,
    mtime: u64,
    /// Permission bits of the local copy; `None` where the platform has none.
    mode: Option<u32>,
}

impl LocalItem {
    fn remote_path(&self, root: &str) -> String {
        if self.relative.is_empty() {
            root.to_owned()
        } else {
            format!("{root}/{}", self.relative)
        }
    }
}

/// Lists `local_root` and everything below it, parents before children. Symlinks and other
/// special files are ignored and never followed.
fn collect_local(local_root: &Path, root: &str) -> Result<Vec<LocalItem>, WarpSyncError> {
    let metadata = match fs::symlink_metadata(local_root) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(WarpSyncError::NotMirrored(root.to_owned()));
        }
        Err(err) => return Err(local_io("read", local_root, &err)),
    };
    if metadata.file_type().is_symlink() {
        return Err(WarpSyncError::LocalIo(format!(
            "{} is a symbolic link",
            local_root.display()
        )));
    }

    let mut items = Vec::new();
    let mut pending = vec![(local_root.to_owned(), String::new(), metadata)];
    while let Some((path, relative, metadata)) = pending.pop() {
        if items.len() >= MAX_ENTRIES {
            return Err(too_large(format!(
                "the mirror has more than {MAX_ENTRIES} entries"
            )));
        }
        let is_dir = metadata.is_dir();
        if is_dir {
            pending.extend(read_children(&path, &relative)?.into_iter().rev());
        }
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_secs());
        items.push(LocalItem {
            path,
            relative,
            is_dir,
            len: metadata.len(),
            mtime,
            mode: local_mode(&metadata),
        });
    }
    Ok(items)
}

#[cfg(unix)]
fn local_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;

    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn local_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

type PendingChild = (PathBuf, String, fs::Metadata);

/// Regular files and directories directly inside `dir`, except Git metadata, sorted by name.
fn read_children(dir: &Path, relative: &str) -> Result<Vec<PendingChild>, WarpSyncError> {
    let mut children = Vec::new();
    for child in fs::read_dir(dir).map_err(|err| local_io("read", dir, &err))? {
        let child = child.map_err(|err| local_io("read", dir, &err))?;
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).map_err(|err| local_io("read", &path, &err))?;
        if !(metadata.is_file() || metadata.is_dir()) {
            continue;
        }
        let file_name = child.file_name();
        let Some(file_name) = file_name.to_str() else {
            return Err(WarpSyncError::LocalIo(format!(
                "{} has a name that is not valid UTF-8",
                path.display()
            )));
        };
        if is_git_metadata_name(file_name) {
            continue;
        }
        let child_relative = if relative.is_empty() {
            file_name.to_owned()
        } else {
            format!("{relative}/{file_name}")
        };
        children.push((path, child_relative, metadata));
    }
    children.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(children)
}

/// Packs the mirror of the remote path `root`. Modes and ownership of known entries come from
/// the manifest; new entries inherit the owner of the closest known directory and keep the
/// permission bits of their local copy.
pub fn build_upload(
    root: &str,
    manifest: &Manifest,
    mirror_root: &Path,
    host_key: &str,
    max_upload_bytes: usize,
) -> Result<UploadArchive, WarpSyncError> {
    let known = manifest.entries_under(root);
    if known.is_empty() {
        return Err(WarpSyncError::NotMirrored(root.to_owned()));
    }
    let local_root = local_path_for(mirror_root, host_key, root);
    let items = collect_local(&local_root, root)?;

    let content_bytes: u64 = items
        .iter()
        .filter(|item| !item.is_dir)
        .map(|item| item.len)
        .sum();
    if content_bytes > MAX_EXTRACTED_BYTES {
        return Err(too_large(format!(
            "the mirror holds more than {} MiB",
            MAX_EXTRACTED_BYTES / BYTES_PER_MIB
        )));
    }

    let (_, name) = split_parent_name(root);
    let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut present = BTreeSet::new();
    let mut new_files = Vec::new();
    let mut summary = UploadArchive {
        bytes: Vec::new(),
        files: 0,
        dirs: 0,
        content_bytes,
        new_files: Vec::new(),
        missing_locally: Vec::new(),
    };
    for item in &items {
        let remote_path = item.remote_path(root);
        let meta = match known.get(&remote_path) {
            Some(meta) => check_kind(meta, item, &remote_path)?.clone(),
            None => {
                new_files.push(remote_path.clone());
                new_entry_meta(manifest, item, &remote_path)?
            }
        };
        // The manifest is only as trustworthy as the host that reported it; never recreate a
        // setuid or setgid program from it.
        if meta.kind == EntryKind::File && meta.mode & SETUID_SETGID_BITS != 0 {
            return Err(WarpSyncError::SpecialMode(remote_path));
        }
        append_item(&mut builder, item, &name, &meta)?;
        if item.is_dir {
            summary.dirs += 1;
        } else {
            summary.files += 1;
        }
        present.insert(remote_path);
    }

    let encoder = builder
        .into_inner()
        .map_err(|err| local_io_message("pack", &err))?;
    summary.bytes = encoder
        .finish()
        .map_err(|err| local_io_message("pack", &err))?;
    if summary.bytes.len() > max_upload_bytes {
        return Err(too_large(format!(
            "the upload is {} KiB compressed; the limit is {} KiB",
            summary.bytes.len().div_ceil(1024),
            max_upload_bytes / 1024
        )));
    }
    summary.new_files = new_files;
    summary.missing_locally = known
        .into_keys()
        .filter(|path| !present.contains(path))
        .collect();
    Ok(summary)
}

fn check_kind<'a>(
    meta: &'a EntryMeta,
    item: &LocalItem,
    remote_path: &str,
) -> Result<&'a EntryMeta, WarpSyncError> {
    let expected = if item.is_dir {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    if meta.kind == expected {
        Ok(meta)
    } else {
        Err(WarpSyncError::LocalIo(format!(
            "{remote_path} changed between file and directory since it was downloaded"
        )))
    }
}

fn new_entry_meta(
    manifest: &Manifest,
    item: &LocalItem,
    remote_path: &str,
) -> Result<EntryMeta, WarpSyncError> {
    let owner = manifest.nearest_dir_ancestor(remote_path).ok_or_else(|| {
        WarpSyncError::Manifest(format!("no ownership information for {remote_path}"))
    })?;
    Ok(EntryMeta {
        kind: if item.is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        mode: new_entry_mode(item),
        uid: owner.uid,
        gid: owner.gid,
        uname: owner.uname.clone(),
        gname: owner.gname.clone(),
        mtime: item.mtime,
        size: None,
        sha256: None,
    })
}

fn new_entry_mode(item: &LocalItem) -> u32 {
    let (mask, fallback) = if item.is_dir {
        (NEW_DIR_MODE_MASK, NEW_DIR_MODE)
    } else {
        (NEW_FILE_MODE_MASK, NEW_FILE_MODE)
    };
    item.mode.map_or(fallback, |mode| mode & mask)
}

fn append_item<W: Write>(
    builder: &mut Builder<W>,
    item: &LocalItem,
    name: &str,
    meta: &EntryMeta,
) -> Result<(), WarpSyncError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(match meta.kind {
        EntryKind::File => EntryType::Regular,
        EntryKind::Dir => EntryType::Directory,
    });
    header.set_mode(meta.mode);
    header.set_uid(u64::from(meta.uid));
    header.set_gid(u64::from(meta.gid));
    header.set_mtime(item.mtime);
    // Names too long for the header field are left blank; the numeric ids still identify the
    // owner.
    header.set_username(&meta.uname).ok();
    header.set_groupname(&meta.gname).ok();

    let archive_path = if item.relative.is_empty() {
        name.to_owned()
    } else {
        format!("{name}/{}", item.relative)
    };
    let result = if item.is_dir {
        header.set_size(0);
        builder.append_data(&mut header, &archive_path, io::empty())
    } else {
        // The header size must match the bytes written even if an editor saves the file while
        // packing, so take both from a single read.
        let contents = fs::read(&item.path).map_err(|err| local_io("read", &item.path, &err))?;
        header.set_size(contents.len() as u64);
        builder.append_data(&mut header, &archive_path, contents.as_slice())
    };
    result.map_err(|err| local_io("pack", &item.path, &err))
}

/// Remote paths under `root` whose local copy differs from what was downloaded, or that do not
/// exist in the manifest at all. Replacing the local subtree would discard them.
pub fn locally_modified_files(
    root: &str,
    manifest_entries: &BTreeMap<String, EntryMeta>,
    mirror_root: &Path,
    host_key: &str,
) -> Result<Vec<String>, WarpSyncError> {
    let local_root = local_path_for(mirror_root, host_key, root);
    let items = match collect_local(&local_root, root) {
        Ok(items) => items,
        Err(WarpSyncError::NotMirrored(_)) => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut modified = Vec::new();
    for item in items.iter().filter(|item| !item.is_dir) {
        let remote_path = item.remote_path(root);
        let recorded = manifest_entries
            .get(&remote_path)
            .and_then(|meta| meta.sha256.as_deref());
        if recorded != Some(file_sha256(&item.path)?.as_str()) {
            modified.push(remote_path);
        }
    }
    modified.sort();
    Ok(modified)
}

/// A regular file in the local mirror.
#[derive(Debug, Clone)]
pub struct LocalFile {
    pub path: PathBuf,
    pub sha256: String,
}

/// The regular files of the mirror of `root`, keyed by their remote path. Empty if `root` has not
/// been mirrored.
pub fn local_files(
    root: &str,
    mirror_root: &Path,
    host_key: &str,
) -> Result<BTreeMap<String, LocalFile>, WarpSyncError> {
    let local_root = local_path_for(mirror_root, host_key, root);
    let items = match collect_local(&local_root, root) {
        Ok(items) => items,
        Err(WarpSyncError::NotMirrored(_)) => return Ok(BTreeMap::new()),
        Err(err) => return Err(err),
    };
    items
        .into_iter()
        .filter(|item| !item.is_dir)
        .map(|item| {
            let sha256 = file_sha256(&item.path)?;
            Ok((
                item.remote_path(root),
                LocalFile {
                    path: item.path,
                    sha256,
                },
            ))
        })
        .collect()
}

fn file_sha256(path: &Path) -> Result<String, WarpSyncError> {
    let mut file = File::open(path).map_err(|err| local_io("read", path, &err))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| local_io("read", path, &err))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn corrupt(err: io::Error) -> WarpSyncError {
    if err.kind() == io::ErrorKind::FileTooLarge {
        return too_large("the archive expands to far more than its declared size".to_owned());
    }
    WarpSyncError::CorruptArchive(err.to_string())
}

fn too_large(limit_desc: String) -> WarpSyncError {
    WarpSyncError::TooLarge { limit_desc }
}

fn local_io(action: &str, path: &Path, err: &io::Error) -> WarpSyncError {
    WarpSyncError::LocalIo(format!("could not {action} {}: {err}", path.display()))
}

fn local_io_message(action: &str, err: &io::Error) -> WarpSyncError {
    WarpSyncError::LocalIo(format!("could not {action} the archive: {err}"))
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
