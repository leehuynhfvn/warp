use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

const ROOT: &str = "/etc/conf";

fn sha(data: &str) -> String {
    hex::encode(Sha256::digest(data.as_bytes()))
}

fn meta(kind: EntryKind, sha256: Option<String>) -> EntryMeta {
    EntryMeta {
        kind,
        mode: 0o644,
        uid: 0,
        gid: 0,
        uname: "root".to_owned(),
        gname: "root".to_owned(),
        mtime: 0,
        size: None,
        sha256,
    }
}

/// A server copy and a mirror on disk, plus what the mirror last synced.
struct Fixture {
    dir: TempDir,
    remote_entries: BTreeMap<String, EntryMeta>,
    local_files: BTreeMap<String, LocalFile>,
    recorded: BTreeMap<String, EntryMeta>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("server")).unwrap();
        fs::create_dir_all(dir.path().join("mirror")).unwrap();
        let mut remote_entries = BTreeMap::new();
        remote_entries.insert(ROOT.to_owned(), meta(EntryKind::Dir, None));
        Self {
            dir,
            remote_entries,
            local_files: BTreeMap::new(),
            recorded: BTreeMap::new(),
        }
    }

    fn server(&mut self, name: &str, contents: &[u8]) -> &mut Self {
        fs::write(self.dir.path().join("server").join(name), contents).unwrap();
        self.remote_entries.insert(
            format!("{ROOT}/{name}"),
            meta(EntryKind::File, Some(hex::encode(Sha256::digest(contents)))),
        );
        self
    }

    fn mirror(&mut self, name: &str, contents: &[u8]) -> &mut Self {
        let path = self.dir.path().join("mirror").join(name);
        fs::write(&path, contents).unwrap();
        self.local_files.insert(
            format!("{ROOT}/{name}"),
            LocalFile {
                path,
                sha256: hex::encode(Sha256::digest(contents)),
            },
        );
        self
    }

    fn recorded(&mut self, name: &str, contents: &str) -> &mut Self {
        self.recorded.insert(
            format!("{ROOT}/{name}"),
            meta(EntryKind::File, Some(sha(contents))),
        );
        self
    }

    fn compare(&self) -> Comparison {
        compare_trees(&ComparedTrees {
            remote_root: ROOT,
            remote_copy: &self.dir.path().join("server"),
            remote_entries: &self.remote_entries,
            local_files: &self.local_files,
            recorded: &self.recorded,
        })
        .unwrap()
    }
}

fn changes(comparison: &Comparison) -> Vec<(&str, FileChange)> {
    comparison
        .differences
        .iter()
        .map(|difference| (difference.remote_path.as_str(), difference.change))
        .collect()
}

#[test]
fn identical_trees_have_no_differences() {
    let mut fixture = Fixture::new();
    fixture.server("a", b"same\n").mirror("a", b"same\n");

    let comparison = fixture.compare();

    assert!(comparison.differences.is_empty());
    assert_eq!(comparison.identical_files, 1);
    assert!(comparison.unified_diff.is_empty());
}

#[test]
fn a_change_is_attributed_to_the_side_that_moved() {
    let mut fixture = Fixture::new();
    fixture
        .recorded("local", "old\n")
        .server("local", b"old\n")
        .mirror("local", b"edited\n")
        .recorded("server", "old\n")
        .server("server", b"edited\n")
        .mirror("server", b"old\n")
        .recorded("both", "old\n")
        .server("both", b"theirs\n")
        .mirror("both", b"mine\n")
        .server("unknown", b"one\n")
        .mirror("unknown", b"two\n");

    let comparison = fixture.compare();

    assert_eq!(
        changes(&comparison),
        vec![
            ("/etc/conf/both", FileChange::ChangedOnBoth),
            ("/etc/conf/local", FileChange::ChangedLocally),
            ("/etc/conf/server", FileChange::ChangedOnServer),
            ("/etc/conf/unknown", FileChange::ChangedUnknown),
        ]
    );
}

#[test]
fn files_present_on_one_side_only_are_classified_by_the_record() {
    let mut fixture = Fixture::new();
    fixture
        .server("new-on-server", b"x\n")
        .recorded("deleted-locally", "x\n")
        .server("deleted-locally", b"x\n")
        .mirror("new-locally", b"x\n")
        .recorded("deleted-on-server", "x\n")
        .mirror("deleted-on-server", b"x\n");

    let comparison = fixture.compare();

    assert_eq!(
        changes(&comparison),
        vec![
            ("/etc/conf/deleted-locally", FileChange::DeletedLocally),
            ("/etc/conf/deleted-on-server", FileChange::DeletedOnServer),
            ("/etc/conf/new-locally", FileChange::NewLocally),
            ("/etc/conf/new-on-server", FileChange::NewOnServer),
        ]
    );
}

#[test]
fn the_diff_shows_the_server_as_old_and_the_mirror_as_new() {
    let mut fixture = Fixture::new();
    fixture
        .server("a", b"one\ntwo\nthree\n")
        .mirror("a", b"one\n2\nthree\n");

    let diff = fixture.compare().unified_diff;

    assert!(diff.contains("--- server:/etc/conf/a"), "{diff}");
    assert!(diff.contains("+++ mirror:/etc/conf/a"), "{diff}");
    assert!(diff.contains("-two\n+2\n"), "{diff}");
}

#[test]
fn a_file_missing_on_one_side_is_diffed_against_nothing() {
    let mut fixture = Fixture::new();
    fixture.server("only-server", b"a\n").mirror("only-mirror", b"b\n");

    let diff = fixture.compare().unified_diff;

    assert!(diff.contains("+++ /dev/null"), "{diff}");
    assert!(diff.contains("--- /dev/null"), "{diff}");
    assert!(diff.contains("-a\n"), "{diff}");
    assert!(diff.contains("+b\n"), "{diff}");
}

#[test]
fn binary_files_are_listed_without_a_line_diff() {
    let mut fixture = Fixture::new();
    fixture.server("blob", b"\x00\x01\x02").mirror("blob", b"\x00\x01\x03");

    let comparison = fixture.compare();

    assert_eq!(comparison.differences.len(), 1);
    assert_eq!(
        comparison.unified_diff,
        "Binary files differ: /etc/conf/blob\n"
    );
}

#[test]
fn files_that_are_not_utf8_count_as_binary() {
    let mut fixture = Fixture::new();
    fixture.server("latin", b"caf\xe9\n").mirror("latin", b"cafe\n");

    let diff = fixture.compare().unified_diff;

    assert_eq!(diff, "Binary files differ: /etc/conf/latin\n");
}

#[test]
fn large_files_are_listed_without_a_line_diff() {
    let mut fixture = Fixture::new();
    let large = "x\n".repeat(MAX_DIFFED_FILE_BYTES as usize / 2 + 1);
    fixture.server("big", large.as_bytes()).mirror("big", b"small\n");

    let comparison = fixture.compare();

    assert_eq!(
        comparison.unified_diff,
        "File too large to compare: /etc/conf/big\n"
    );
}

#[test]
fn a_long_diff_is_cut_off_and_says_so() {
    let mut fixture = Fixture::new();
    for name in ["f0", "f1", "f2", "f3"] {
        fixture.server(name, b"old\n").mirror(name, b"new\n");
    }

    let comparison = compare_trees_with_limit(
        &ComparedTrees {
            remote_root: ROOT,
            remote_copy: &fixture.dir.path().join("server"),
            remote_entries: &fixture.remote_entries,
            local_files: &fixture.local_files,
            recorded: &fixture.recorded,
        },
        1,
    )
    .unwrap();

    assert_eq!(comparison.differences.len(), 4);
    assert!(comparison.unified_diff.contains("--- server:/etc/conf/f0"));
    assert!(!comparison.unified_diff.contains("f1\n+++"));
    assert!(
        comparison
            .unified_diff
            .contains("3 more file(s) differ but are not shown")
    );
}

#[test]
fn a_single_file_is_compared_through_its_own_path() {
    let dir = tempfile::tempdir().unwrap();
    let server = dir.path().join("hosts");
    fs::write(&server, "old\n").unwrap();
    let mirror = dir.path().join("mirror-hosts");
    fs::write(&mirror, "new\n").unwrap();
    let remote_entries = BTreeMap::from([(
        "/etc/hosts".to_owned(),
        meta(EntryKind::File, Some(sha("old\n"))),
    )]);
    let local_files = BTreeMap::from([(
        "/etc/hosts".to_owned(),
        LocalFile {
            path: mirror,
            sha256: sha("new\n"),
        },
    )]);

    let comparison = compare_trees(&ComparedTrees {
        remote_root: "/etc/hosts",
        remote_copy: &server,
        remote_entries: &remote_entries,
        local_files: &local_files,
        recorded: &BTreeMap::new(),
    })
    .unwrap();

    assert!(comparison.unified_diff.contains("-old\n+new\n"));
}

#[test]
fn the_report_summarizes_before_the_diff() {
    let mut fixture = Fixture::new();
    fixture
        .server("a", b"one\n")
        .mirror("a", b"two\n")
        .server("same", b"x\n")
        .mirror("same", b"x\n");

    let report = render_report(ROOT, "prod-1", &fixture.compare());

    assert!(report.starts_with("# Warp Sync: prod-1:/etc/conf\n"));
    assert!(report.contains("# 1 file(s) differ, 1 are identical.\n"));
    assert!(report.contains("#   differs: /etc/conf/a\n"));
    assert!(report.contains("--- server:/etc/conf/a"));
}

#[test]
fn a_disk_path_follows_the_remote_path_below_the_root() {
    let copy = Path::new("/staging/conf");

    assert_eq!(
        disk_path(copy, "/etc/conf", "/etc/conf/sub/a"),
        Path::new("/staging/conf/sub/a")
    );
    assert_eq!(disk_path(copy, "/etc/conf", "/etc/conf"), copy);
}
