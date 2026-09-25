use std::fs;

use tempfile::TempDir;

use super::*;

const TEST_MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024;

struct TestEntry {
    path: &'static [u8],
    kind: EntryType,
    mode: u32,
    uid: u64,
    gid: u64,
    uname: &'static str,
    gname: &'static str,
    data: &'static [u8],
}

impl TestEntry {
    fn file(path: &'static [u8], data: &'static [u8]) -> Self {
        Self {
            path,
            kind: EntryType::Regular,
            mode: 0o640,
            uid: 0,
            gid: 0,
            uname: "root",
            gname: "root",
            data,
        }
    }

    fn dir(path: &'static [u8]) -> Self {
        Self {
            kind: EntryType::Directory,
            mode: 0o750,
            ..Self::file(path, b"")
        }
    }

    fn of_kind(path: &'static [u8], kind: EntryType) -> Self {
        Self {
            kind,
            ..Self::file(path, b"")
        }
    }

    fn owned_by(self, uid: u64, gid: u64, uname: &'static str, gname: &'static str) -> Self {
        Self {
            uid,
            gid,
            uname,
            gname,
            ..self
        }
    }

    fn with_mode(self, mode: u32) -> Self {
        Self { mode, ..self }
    }
}

/// Builds a tarball with names written verbatim, bypassing the builder's path sanitizing so that
/// hostile archives can be produced.
fn build_tgz(entries: &[TestEntry]) -> Vec<u8> {
    let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for entry in entries {
        let mut header = Header::new_gnu();
        header.as_old_mut().name[..entry.path.len()].copy_from_slice(entry.path);
        header.set_entry_type(entry.kind);
        header.set_mode(entry.mode);
        header.set_uid(entry.uid);
        header.set_gid(entry.gid);
        header.set_mtime(1_727_000_000);
        header.set_username(entry.uname).unwrap();
        header.set_groupname(entry.gname).unwrap();
        header.set_size(entry.data.len() as u64);
        if entry.kind == EntryType::Symlink || entry.kind == EntryType::Link {
            header.set_link_name("target").unwrap();
        }
        header.set_cksum();
        builder.append(&header, entry.data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn extract(tgz: &[u8], staging: &Path) -> Result<ExtractReport, WarpSyncError> {
    extract_download(tgz, "conf", "/etc", staging)
}

fn sample_tgz() -> Vec<u8> {
    build_tgz(&[
        TestEntry::dir(b"conf/").owned_by(33, 33, "www-data", "www-data"),
        TestEntry::file(b"conf/a.conf", b"old a").with_mode(0o640),
        TestEntry::file(b"conf/b.conf", b"old b").with_mode(0o600),
    ])
}

/// A mirror that contains the download of [`sample_tgz`] and its manifest.
struct Mirror {
    dir: TempDir,
    manifest: Manifest,
}

impl Mirror {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = Manifest::new("h");
        let staging = dir.path().join("h/etc");
        let report = extract(&sample_tgz(), &staging).unwrap();
        manifest.replace_subtree("/etc/conf", report.entries);
        Self { dir, manifest }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn local(&self, relative: &str) -> PathBuf {
        self.root().join("h/etc/conf").join(relative)
    }

    fn build(&self) -> Result<UploadArchive, WarpSyncError> {
        build_upload(
            "/etc/conf",
            &self.manifest,
            self.root(),
            "h",
            TEST_MAX_UPLOAD_BYTES,
        )
    }
}

#[cfg(unix)]
fn set_local_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_local_mode(_path: &Path, _mode: u32) {}

fn entries_of_upload(upload: &UploadArchive) -> ExtractReport {
    let staging = tempfile::tempdir().unwrap();
    extract(&upload.bytes, &staging.path().join("s")).unwrap()
}

#[test]
fn extracts_files_and_directories_with_remote_metadata() {
    let staging = tempfile::tempdir().unwrap();

    let report = extract(&sample_tgz(), staging.path()).unwrap();

    assert_eq!(report.files, 2);
    assert_eq!(report.dirs, 1);
    assert_eq!(report.total_bytes, 10);
    assert!(report.skipped.is_empty());
    let keys: Vec<_> = report.entries.keys().collect();
    assert_eq!(keys, ["/etc/conf", "/etc/conf/a.conf", "/etc/conf/b.conf"]);

    let dir = &report.entries["/etc/conf"];
    assert_eq!(
        (dir.kind, dir.mode, dir.uid, dir.gid),
        (EntryKind::Dir, 0o750, 33, 33)
    );
    assert_eq!(
        (dir.uname.as_str(), dir.gname.as_str()),
        ("www-data", "www-data")
    );

    let file = &report.entries["/etc/conf/a.conf"];
    assert_eq!(
        (file.kind, file.mode, file.uid),
        (EntryKind::File, 0o640, 0)
    );
    assert_eq!(file.size, Some(5));
    assert_eq!(file.mtime, 1_727_000_000);
    assert_eq!(
        file.sha256.as_deref(),
        Some(hex::encode(Sha256::digest(b"old a")).as_str())
    );

    assert_eq!(
        fs::read(staging.path().join("conf/a.conf")).unwrap(),
        b"old a"
    );
}

#[cfg(unix)]
#[test]
fn local_permissions_stay_usable_by_the_local_user() {
    use std::os::unix::fs::PermissionsExt;

    let staging = tempfile::tempdir().unwrap();
    let tgz = build_tgz(&[
        TestEntry::dir(b"conf/").with_mode(0o500),
        TestEntry::file(b"conf/secret", b"x").with_mode(0o400),
        TestEntry::file(b"conf/script", b"x").with_mode(0o755),
    ]);

    let report = extract(&tgz, staging.path()).unwrap();

    let mode = |relative: &str| {
        fs::metadata(staging.path().join(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode("conf"), 0o700);
    assert_eq!(mode("conf/secret"), 0o600);
    assert_eq!(mode("conf/script"), 0o755);
    assert_eq!(report.entries["/etc/conf/secret"].mode, 0o400);
}

#[cfg(unix)]
#[test]
fn local_copies_drop_group_write_and_special_bits() {
    use std::os::unix::fs::PermissionsExt;

    let staging = tempfile::tempdir().unwrap();
    let tgz = build_tgz(&[
        TestEntry::dir(b"conf/").with_mode(0o1777),
        TestEntry::file(b"conf/su", b"x").with_mode(0o4777),
        TestEntry::file(b"conf/shared", b"x").with_mode(0o664),
    ]);

    extract(&tgz, staging.path()).unwrap();

    let mode = |relative: &str| {
        fs::metadata(staging.path().join(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(mode("conf"), 0o755);
    assert_eq!(mode("conf/su"), 0o755);
    assert_eq!(mode("conf/shared"), 0o644);
}

#[test]
fn setuid_and_sticky_bits_are_recorded() {
    let staging = tempfile::tempdir().unwrap();
    let tgz = build_tgz(&[
        TestEntry::dir(b"conf/").with_mode(0o1777),
        TestEntry::file(b"conf/su", b"x").with_mode(0o4755),
    ]);

    let report = extract(&tgz, staging.path()).unwrap();

    assert_eq!(report.entries["/etc/conf"].mode, 0o1777);
    assert_eq!(report.entries["/etc/conf/su"].mode, 0o4755);
}

#[test]
fn remote_paths_under_the_root_directory_have_a_single_slash() {
    let staging = tempfile::tempdir().unwrap();
    let tgz = build_tgz(&[TestEntry::file(b"conf", b"x")]);

    let report = extract_download(&tgz, "conf", "/", staging.path()).unwrap();

    assert_eq!(report.entries.keys().collect::<Vec<_>>(), ["/conf"]);
}

#[test]
fn hostile_paths_are_rejected_and_nothing_escapes_staging() {
    for path in [
        &b"conf/../../evil"[..],
        b"../evil",
        b"/etc/passwd",
        b"other/file",
        b"conffile",
    ] {
        let sandbox = tempfile::tempdir().unwrap();
        let staging = sandbox.path().join("staging");
        let tgz = build_tgz(&[TestEntry::dir(b"conf/"), TestEntry::file(path, b"pwned")]);

        let result = extract(&tgz, &staging);

        assert!(
            matches!(result, Err(WarpSyncError::UnexpectedArchiveEntry(_))),
            "{}",
            String::from_utf8_lossy(path)
        );
        let names: Vec<_> = fs::read_dir(sandbox.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["staging"], "{}", String::from_utf8_lossy(path));
    }
}

#[test]
fn links_and_special_files_are_skipped_not_created() {
    let staging = tempfile::tempdir().unwrap();
    let tgz = build_tgz(&[
        TestEntry::dir(b"conf/"),
        TestEntry::of_kind(b"conf/sym", EntryType::Symlink),
        TestEntry::of_kind(b"conf/hard", EntryType::Link),
        TestEntry::of_kind(b"conf/pipe", EntryType::Fifo),
        TestEntry::of_kind(b"conf/dev", EntryType::Char),
        TestEntry::file(b"conf/real", b"ok"),
    ]);

    let report = extract(&tgz, staging.path()).unwrap();

    assert_eq!(
        report.skipped,
        [
            ("conf/sym".to_owned(), SkipReason::Symlink),
            ("conf/hard".to_owned(), SkipReason::Hardlink),
            ("conf/pipe".to_owned(), SkipReason::Special),
            ("conf/dev".to_owned(), SkipReason::Special),
        ]
    );
    assert_eq!(report.files, 1);
    for skipped in ["sym", "hard", "pipe", "dev"] {
        assert!(fs::symlink_metadata(staging.path().join("conf").join(skipped)).is_err());
    }
}

#[test]
fn git_metadata_is_skipped_and_reported_once() {
    let staging = tempfile::tempdir().unwrap();
    let tgz = build_tgz(&[
        TestEntry::dir(b"conf/"),
        TestEntry::dir(b"conf/.git/"),
        TestEntry::file(b"conf/.git/config", b"[core]\n\tfsmonitor = touch /tmp/x\n"),
        TestEntry::dir(b"conf/sub/"),
        TestEntry::file(b"conf/sub/.git", b"gitdir: /home/user/repo/.git\n"),
        TestEntry::dir(b"conf/.GIT/"),
        TestEntry::file(b"conf/.GIT/hooks/pre-commit", b"#!/bin/sh\n"),
        TestEntry::file(b"conf/.gitignore", b"*.conf\n"),
    ]);

    let report = extract(&tgz, staging.path()).unwrap();

    assert_eq!(
        report.skipped,
        [
            ("conf/.git/".to_owned(), SkipReason::GitMetadata),
            ("conf/sub/.git".to_owned(), SkipReason::GitMetadata),
            ("conf/.GIT/".to_owned(), SkipReason::GitMetadata),
        ]
    );
    assert!(!staging.path().join("conf/.git").exists());
    assert!(!staging.path().join("conf/sub/.git").exists());
    assert!(!staging.path().join("conf/.GIT").exists());
    assert!(staging.path().join("conf/.gitignore").exists());
    assert!(!report.entries.keys().any(|path| path.contains("/.git/")));
}

#[test]
fn upload_leaves_out_git_metadata_created_locally() {
    let mirror = Mirror::new();
    fs::create_dir(mirror.local(".git")).unwrap();
    fs::write(mirror.local(".git/config"), "[core]").unwrap();

    let upload = mirror.build().unwrap();

    assert!(upload.new_files.is_empty(), "{:?}", upload.new_files);
    assert!(
        !entries_of_upload(&upload)
            .entries
            .keys()
            .any(|path| path.contains(".git"))
    );
}

#[test]
fn entries_below_a_skipped_symlink_stay_inside_staging() {
    let sandbox = tempfile::tempdir().unwrap();
    let staging = sandbox.path().join("staging");
    let tgz = build_tgz(&[
        TestEntry::dir(b"conf/"),
        TestEntry::of_kind(b"conf/link", EntryType::Symlink),
        TestEntry::file(b"conf/link/evil", b"x"),
    ]);

    extract(&tgz, &staging).unwrap();

    assert!(staging.join("conf/link/evil").is_file());
    assert!(
        !fs::symlink_metadata(staging.join("conf/link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn names_that_are_not_utf8_are_skipped() {
    let staging = tempfile::tempdir().unwrap();
    let tgz = build_tgz(&[
        TestEntry::dir(b"conf/"),
        TestEntry::file(b"conf/bad\xff", b"x"),
    ]);

    let report = extract(&tgz, staging.path()).unwrap();

    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].1, SkipReason::NonUtf8Name);
}

#[test]
fn too_many_entries_is_rejected() {
    let staging = tempfile::tempdir().unwrap();
    let limits = ExtractLimits {
        max_bytes: 1024,
        max_entries: 2,
    };

    let result =
        extract_download_with_limits(&sample_tgz(), "conf", "/etc", staging.path(), limits);

    assert!(matches!(result, Err(WarpSyncError::TooLarge { .. })));
}

#[test]
fn too_many_bytes_is_rejected() {
    let staging = tempfile::tempdir().unwrap();
    let limits = ExtractLimits {
        max_bytes: 9,
        max_entries: 10,
    };

    let result =
        extract_download_with_limits(&sample_tgz(), "conf", "/etc", staging.path(), limits);

    assert!(matches!(result, Err(WarpSyncError::TooLarge { .. })));
}

#[test]
fn skipped_entries_cannot_expand_without_bound() {
    static PAYLOAD: [u8; 1024 * 1024] = [0; 1024 * 1024];
    let staging = tempfile::tempdir().unwrap();
    let mut pipe = TestEntry::of_kind(b"conf/pipe", EntryType::Fifo);
    pipe.data = &PAYLOAD;
    let tgz = build_tgz(&[TestEntry::dir(b"conf/"), pipe]);
    let limits = ExtractLimits {
        max_bytes: 100,
        max_entries: 2,
    };

    let result = extract_download_with_limits(&tgz, "conf", "/etc", staging.path(), limits);

    assert!(matches!(result, Err(WarpSyncError::TooLarge { .. })));
}

#[test]
fn garbage_is_a_corrupt_archive() {
    let staging = tempfile::tempdir().unwrap();

    let result = extract(b"definitely not gzip", staging.path());

    assert!(matches!(result, Err(WarpSyncError::CorruptArchive(_))));
}

#[test]
fn truncated_archive_is_corrupt() {
    let staging = tempfile::tempdir().unwrap();
    let tgz = sample_tgz();

    let result = extract(&tgz[..tgz.len() / 2], staging.path());

    assert!(matches!(result, Err(WarpSyncError::CorruptArchive(_))));
}

#[test]
fn bad_gzip_checksum_is_corrupt() {
    let staging = tempfile::tempdir().unwrap();
    let mut tgz = sample_tgz();
    let crc_start = tgz.len() - 8;
    tgz[crc_start] ^= 0xff;

    let result = extract(&tgz, staging.path());

    assert!(matches!(result, Err(WarpSyncError::CorruptArchive(_))));
}

#[test]
fn empty_archive_is_corrupt() {
    let staging = tempfile::tempdir().unwrap();

    let result = extract(&build_tgz(&[]), staging.path());

    assert!(matches!(result, Err(WarpSyncError::CorruptArchive(_))));
}

#[test]
fn upload_of_an_untouched_mirror_reproduces_the_downloaded_metadata() {
    let mirror = Mirror::new();

    let upload = mirror.build().unwrap();

    assert_eq!((upload.files, upload.dirs), (2, 1));
    assert!(upload.new_files.is_empty());
    assert!(upload.missing_locally.is_empty());
    let report = entries_of_upload(&upload);
    for (path, meta) in mirror.manifest.entries_under("/etc/conf") {
        let packed = &report.entries[&path];
        assert_eq!(
            (
                packed.kind,
                packed.mode,
                packed.uid,
                packed.gid,
                &packed.uname,
                &packed.gname
            ),
            (
                meta.kind,
                meta.mode,
                meta.uid,
                meta.gid,
                &meta.uname,
                &meta.gname
            ),
            "{path}"
        );
        assert_eq!(packed.sha256, meta.sha256, "{path}");
    }
}

#[test]
fn upload_carries_edits_new_files_and_reports_missing_ones() {
    let mirror = Mirror::new();
    fs::write(mirror.local("a.conf"), "edited a").unwrap();
    fs::remove_file(mirror.local("b.conf")).unwrap();
    fs::write(mirror.local("c.conf"), "new c").unwrap();
    fs::create_dir(mirror.local("sub")).unwrap();
    fs::write(mirror.local("sub/d.conf"), "new d").unwrap();
    set_local_mode(&mirror.local("c.conf"), 0o644);
    set_local_mode(&mirror.local("sub"), 0o755);

    let upload = mirror.build().unwrap();

    assert_eq!(
        upload.new_files,
        ["/etc/conf/c.conf", "/etc/conf/sub", "/etc/conf/sub/d.conf"]
    );
    assert_eq!(upload.missing_locally, ["/etc/conf/b.conf"]);
    assert_eq!(upload.content_bytes, "edited a".len() as u64 + 10);

    let report = entries_of_upload(&upload);
    let edited = &report.entries["/etc/conf/a.conf"];
    assert_eq!(
        (edited.mode, edited.uid, edited.uname.as_str()),
        (0o640, 0, "root")
    );
    assert_eq!(edited.size, Some(8));

    let new_file = &report.entries["/etc/conf/c.conf"];
    assert_eq!((new_file.mode, new_file.uid, new_file.gid), (0o644, 33, 33));
    assert_eq!(new_file.uname, "www-data");

    let new_dir = &report.entries["/etc/conf/sub"];
    assert_eq!(
        (new_dir.kind, new_dir.mode, new_dir.uid),
        (EntryKind::Dir, 0o755, 33)
    );
    let nested = &report.entries["/etc/conf/sub/d.conf"];
    assert_eq!((nested.uid, nested.gname.as_str()), (33, "www-data"));
    assert!(!report.entries.contains_key("/etc/conf/b.conf"));
}

#[cfg(unix)]
#[test]
fn new_entries_take_the_permission_bits_of_the_local_copy() {
    let mirror = Mirror::new();
    fs::write(mirror.local("secret.conf"), "s").unwrap();
    fs::write(mirror.local("tool.sh"), "t").unwrap();
    fs::create_dir(mirror.local("private")).unwrap();
    set_local_mode(&mirror.local("secret.conf"), 0o600);
    set_local_mode(&mirror.local("tool.sh"), 0o755);
    set_local_mode(&mirror.local("private"), 0o700);

    let report = entries_of_upload(&mirror.build().unwrap());

    assert_eq!(report.entries["/etc/conf/secret.conf"].mode, 0o600);
    assert_eq!(report.entries["/etc/conf/tool.sh"].mode, 0o755);
    assert_eq!(report.entries["/etc/conf/private"].mode, 0o700);
}

#[cfg(unix)]
#[test]
fn the_modes_of_new_entries_are_reported_for_the_confirmation() {
    let mirror = Mirror::new();
    fs::write(mirror.local("secret.conf"), "s").unwrap();
    set_local_mode(&mirror.local("secret.conf"), 0o600);
    fs::write(mirror.local("a.conf"), "edited").unwrap();

    let upload = mirror.build().unwrap();

    assert_eq!(
        upload.new_modes,
        BTreeMap::from([("/etc/conf/secret.conf".to_owned(), 0o600)])
    );
}

#[cfg(unix)]
#[test]
fn the_modes_of_the_levels_created_for_a_new_path_are_reported_too() {
    let mirror = Mirror::with_new_file("one/two/new.conf", "new");
    set_local_mode(&mirror.local("one/two/new.conf"), 0o640);

    let upload = mirror.build_new("/etc/conf/one/two/new.conf").unwrap();

    assert_eq!(
        upload.new_modes,
        BTreeMap::from([
            ("/etc/conf/one".to_owned(), 0o755),
            ("/etc/conf/one/two".to_owned(), 0o755),
            ("/etc/conf/one/two/new.conf".to_owned(), 0o640),
        ])
    );
}

#[cfg(unix)]
#[test]
fn the_local_mode_of_a_known_entry_does_not_change_what_the_server_has() {
    let mirror = Mirror::new();
    set_local_mode(&mirror.local("a.conf"), 0o777);

    let report = entries_of_upload(&mirror.build().unwrap());

    assert_eq!(report.entries["/etc/conf/a.conf"].mode, 0o640);
}

#[cfg(unix)]
#[test]
fn a_new_file_with_setuid_or_setgid_bits_is_refused() {
    for mode in [0o4755, 0o2755, 0o6755] {
        let mirror = Mirror::new();
        fs::write(mirror.local("new.sh"), "x").unwrap();
        set_local_mode(&mirror.local("new.sh"), mode);

        let result = mirror.build();

        assert!(
            matches!(result, Err(WarpSyncError::SpecialMode(ref path)) if path == "/etc/conf/new.sh"),
            "{mode:o}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_new_directory_never_gets_special_bits_from_the_local_copy() {
    let mirror = Mirror::new();
    fs::create_dir(mirror.local("sub")).unwrap();
    set_local_mode(&mirror.local("sub"), 0o3775);

    let report = entries_of_upload(&mirror.build().unwrap());

    assert_eq!(report.entries["/etc/conf/sub"].mode, 0o775);
}

impl Mirror {
    fn build_new(&self, root: &str) -> Result<UploadArchive, WarpSyncError> {
        build_new_upload(
            root,
            "/etc/conf",
            &self.manifest,
            self.root(),
            "h",
            TEST_MAX_UPLOAD_BYTES,
        )
    }

    fn with_new_file(relative: &str, contents: &str) -> Self {
        let mirror = Self::new();
        let path = mirror.local(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        mirror
    }
}

/// Unpacks a new-path upload the way the server, and the manifest update, will see it.
fn entries_of_new_upload(upload: &UploadArchive, top_level: &str) -> ExtractReport {
    let staging = tempfile::tempdir().unwrap();
    extract_download(&upload.bytes, top_level, "/etc/conf", &staging.path().join("s")).unwrap()
}

#[test]
fn a_new_path_is_packed_under_its_levels_with_the_owner_of_the_anchor() {
    let mirror = Mirror::with_new_file("one/two/new.conf", "new");

    let upload = mirror.build_new("/etc/conf/one/two/new.conf").unwrap();

    assert_eq!((upload.files, upload.dirs), (1, 2));
    assert_eq!(
        upload.new_files,
        [
            "/etc/conf/one",
            "/etc/conf/one/two",
            "/etc/conf/one/two/new.conf"
        ]
    );
    assert!(upload.missing_locally.is_empty());
    let report = entries_of_new_upload(&upload, "one");
    assert_eq!(report.entries.len(), 3);
    for path in ["/etc/conf/one", "/etc/conf/one/two"] {
        let level = &report.entries[path];
        assert_eq!(
            (level.kind, level.mode, level.uid, level.gid, level.uname.as_str()),
            (EntryKind::Dir, 0o755, 33, 33, "www-data"),
            "{path}"
        );
    }
    let file = &report.entries["/etc/conf/one/two/new.conf"];
    assert_eq!((file.uid, file.gid, file.gname.as_str()), (33, 33, "www-data"));
    assert_eq!(file.size, Some(3));
}

#[test]
fn a_new_directory_is_packed_with_everything_below_it_and_nothing_beside_it() {
    let mirror = Mirror::with_new_file("one/a.conf", "a");
    fs::write(mirror.local("one/b.conf"), "b").unwrap();
    fs::write(mirror.local("sibling.conf"), "not part of it").unwrap();
    fs::write(mirror.local("a.conf"), "edited, not part of it").unwrap();

    let upload = mirror.build_new("/etc/conf/one").unwrap();

    assert_eq!((upload.files, upload.dirs), (2, 1));
    let report = entries_of_new_upload(&upload, "one");
    let mut paths: Vec<&str> = report.entries.keys().map(String::as_str).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "/etc/conf/one",
            "/etc/conf/one/a.conf",
            "/etc/conf/one/b.conf"
        ]
    );
}

#[test]
fn a_new_path_that_the_manifest_already_knows_is_not_new() {
    let mirror = Mirror::new();

    let result = mirror.build_new("/etc/conf/a.conf");

    assert!(matches!(result, Err(WarpSyncError::Manifest(_))), "{result:?}");
}

#[test]
fn a_new_path_must_be_below_the_closest_synced_directory() {
    let mut mirror = Mirror::with_new_file("sub/new.conf", "x");
    let sub = mirror.manifest.nearest_dir_ancestor("/etc/conf/a.conf").unwrap().clone();
    mirror
        .manifest
        .upsert_entries(BTreeMap::from([("/etc/conf/sub".to_owned(), sub)]));

    let result = mirror.build_new("/etc/conf/sub/new.conf");

    assert!(matches!(result, Err(WarpSyncError::Manifest(_))), "{result:?}");
}

#[test]
fn a_new_path_with_levels_that_could_climb_out_is_refused() {
    let mirror = Mirror::new();
    for root in [
        "/etc/conf/../x",
        "/etc/conf/a/../../x",
        "/etc/conf/./x",
        "/etc/conf",
        "/etc/other/x",
    ] {
        let result = mirror.build_new(root);

        assert!(matches!(result, Err(WarpSyncError::InvalidPath(_))), "{root}: {result:?}");
    }
}

#[test]
fn a_new_path_without_a_local_copy_is_not_mirrored() {
    let mirror = Mirror::new();

    let result = mirror.build_new("/etc/conf/nowhere/x.conf");

    assert!(matches!(result, Err(WarpSyncError::NotMirrored(_))), "{result:?}");
}

#[cfg(unix)]
#[test]
fn a_new_path_upload_refuses_setuid_files_and_respects_the_size_limit() {
    let mirror = Mirror::with_new_file("one/x.sh", "x");
    set_local_mode(&mirror.local("one/x.sh"), 0o4755);
    assert!(matches!(
        mirror.build_new("/etc/conf/one"),
        Err(WarpSyncError::SpecialMode(_))
    ));

    set_local_mode(&mirror.local("one/x.sh"), 0o644);
    let too_small = build_new_upload(
        "/etc/conf/one",
        "/etc/conf",
        &mirror.manifest,
        mirror.root(),
        "h",
        10,
    );
    assert!(matches!(too_small, Err(WarpSyncError::TooLarge { .. })));
}

#[test]
fn upload_of_a_single_mirrored_file() {
    let mut mirror = Mirror::new();
    let meta = mirror.manifest.entry("/etc/conf/a.conf").unwrap().clone();
    mirror.manifest.replace_subtree(
        "/etc/conf/a.conf",
        BTreeMap::from([("/etc/conf/a.conf".to_owned(), meta)]),
    );

    let upload = build_upload(
        "/etc/conf/a.conf",
        &mirror.manifest,
        mirror.root(),
        "h",
        TEST_MAX_UPLOAD_BYTES,
    )
    .unwrap();

    assert_eq!((upload.files, upload.dirs), (1, 0));
    let report = extract_download(
        &upload.bytes,
        "a.conf",
        "/etc/conf",
        &tempfile::tempdir().unwrap().path().join("s"),
    )
    .unwrap();
    assert_eq!(report.entries["/etc/conf/a.conf"].mode, 0o640);
}

#[test]
fn upload_refuses_setuid_and_setgid_files() {
    for mode in [0o4755, 0o2755, 0o6755] {
        let mut mirror = Mirror::new();
        let mut meta = mirror.manifest.entry("/etc/conf/a.conf").unwrap().clone();
        meta.mode = mode;
        mirror
            .manifest
            .upsert_entries(BTreeMap::from([("/etc/conf/a.conf".to_owned(), meta)]));

        let result = mirror.build();

        assert!(
            matches!(result, Err(WarpSyncError::SpecialMode(ref path)) if path == "/etc/conf/a.conf"),
            "{mode:o}"
        );
    }
}

#[test]
fn upload_keeps_setgid_directories() {
    let mut mirror = Mirror::new();
    let mut meta = mirror.manifest.entry("/etc/conf").unwrap().clone();
    meta.mode = 0o2775;
    mirror
        .manifest
        .upsert_entries(BTreeMap::from([("/etc/conf".to_owned(), meta)]));

    let upload = mirror.build().unwrap();

    let report = entries_of_upload(&upload);
    assert_eq!(report.entries["/etc/conf"].mode, 0o2775);
}

#[test]
fn upload_without_a_manifest_entry_is_not_mirrored() {
    let mirror = Mirror::new();

    let result = build_upload(
        "/etc/other",
        &mirror.manifest,
        mirror.root(),
        "h",
        TEST_MAX_UPLOAD_BYTES,
    );

    assert!(matches!(result, Err(WarpSyncError::NotMirrored(_))));
}

#[test]
fn upload_with_the_local_copy_removed_is_not_mirrored() {
    let mirror = Mirror::new();
    fs::remove_dir_all(mirror.local("")).unwrap();

    assert!(matches!(mirror.build(), Err(WarpSyncError::NotMirrored(_))));
}

#[test]
fn upload_rejects_a_file_that_became_a_directory() {
    let mirror = Mirror::new();
    fs::remove_file(mirror.local("a.conf")).unwrap();
    fs::create_dir(mirror.local("a.conf")).unwrap();

    assert!(matches!(mirror.build(), Err(WarpSyncError::LocalIo(_))));
}

#[test]
fn the_upload_limit_is_taken_from_the_caller() {
    let mirror = Mirror::new();

    let result = build_upload("/etc/conf", &mirror.manifest, mirror.root(), "h", 10);

    assert!(matches!(result, Err(WarpSyncError::TooLarge { .. })));
}

#[test]
fn upload_over_the_size_limit_is_rejected() {
    let mirror = Mirror::new();
    let noise: Vec<u8> = (0..TEST_MAX_UPLOAD_BYTES as u64 + 512 * 1024)
        .scan(0x2545_f491_4f6c_dd1du64, |state, _| {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            Some((*state >> 24) as u8)
        })
        .collect();
    fs::write(mirror.local("a.conf"), noise).unwrap();

    assert!(matches!(
        mirror.build(),
        Err(WarpSyncError::TooLarge { .. })
    ));
}

#[cfg(unix)]
#[test]
fn local_symlinks_are_not_uploaded_or_followed() {
    let mirror = Mirror::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), mirror.local("leak")).unwrap();
    std::os::unix::fs::symlink(outside.path(), mirror.local("leakdir")).unwrap();

    let upload = mirror.build().unwrap();

    assert!(upload.new_files.is_empty());
    let report = entries_of_upload(&upload);
    assert!(report.entries.keys().all(|path| !path.contains("leak")));
}

#[test]
fn unmodified_mirror_has_no_local_changes() {
    let mirror = Mirror::new();
    let entries = mirror.manifest.entries_under("/etc/conf");

    let modified = locally_modified_files("/etc/conf", &entries, mirror.root(), "h").unwrap();

    assert!(modified.is_empty());
}

#[test]
fn edited_and_untracked_files_count_as_local_changes() {
    let mirror = Mirror::new();
    let entries = mirror.manifest.entries_under("/etc/conf");
    fs::write(mirror.local("b.conf"), "edited").unwrap();
    fs::write(mirror.local("a.conf"), "old a").unwrap();
    fs::write(mirror.local("new.conf"), "mine").unwrap();

    let modified = locally_modified_files("/etc/conf", &entries, mirror.root(), "h").unwrap();

    assert_eq!(modified, ["/etc/conf/b.conf", "/etc/conf/new.conf"]);
}

#[test]
fn deleted_files_and_missing_mirrors_are_not_local_changes() {
    let mirror = Mirror::new();
    let entries = mirror.manifest.entries_under("/etc/conf");
    fs::remove_file(mirror.local("a.conf")).unwrap();

    let modified = locally_modified_files("/etc/conf", &entries, mirror.root(), "h").unwrap();
    assert!(modified.is_empty());

    let elsewhere = locally_modified_files("/etc/nothing", &entries, mirror.root(), "h").unwrap();
    assert!(elsewhere.is_empty());
}
