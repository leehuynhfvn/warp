use std::fs;

use super::*;

fn meta(kind: EntryKind, uid: u32) -> EntryMeta {
    EntryMeta {
        kind,
        mode: 0o644,
        uid,
        gid: uid,
        uname: "u".to_owned(),
        gname: "g".to_owned(),
        mtime: 1_727_000_000,
        size: None,
        sha256: None,
    }
}

fn entries(items: &[(&str, EntryKind, u32)]) -> BTreeMap<String, EntryMeta> {
    items
        .iter()
        .map(|(path, kind, uid)| ((*path).to_owned(), meta(*kind, *uid)))
        .collect()
}

#[test]
fn subtree_membership_compares_whole_components() {
    let mut manifest = Manifest::new("h");
    manifest.replace_subtree(
        "/etc",
        entries(&[
            ("/etc/nginx", EntryKind::Dir, 0),
            ("/etc/nginx/nginx.conf", EntryKind::File, 0),
            ("/etc/nginx2", EntryKind::Dir, 0),
            ("/etc/nginx2/x", EntryKind::File, 0),
        ]),
    );

    let under: Vec<String> = manifest.entries_under("/etc/nginx").into_keys().collect();

    assert_eq!(under, ["/etc/nginx", "/etc/nginx/nginx.conf"]);
}

#[test]
fn replace_subtree_removes_old_entries_and_keeps_siblings() {
    let mut manifest = Manifest::new("h");
    manifest.replace_subtree(
        "/etc/nginx",
        entries(&[
            ("/etc/nginx", EntryKind::Dir, 0),
            ("/etc/nginx/old.conf", EntryKind::File, 0),
        ]),
    );
    manifest.replace_subtree("/etc/nginx2", entries(&[("/etc/nginx2", EntryKind::Dir, 1)]));

    manifest.replace_subtree(
        "/etc/nginx",
        entries(&[
            ("/etc/nginx", EntryKind::Dir, 5),
            ("/etc/nginx/new.conf", EntryKind::File, 5),
        ]),
    );

    let all: Vec<String> = manifest.entries_under("/etc").into_keys().collect();
    assert_eq!(all, ["/etc/nginx", "/etc/nginx/new.conf", "/etc/nginx2"]);
    assert_eq!(manifest.entry("/etc/nginx").unwrap().uid, 5);
    assert_eq!(manifest.entry("/etc/nginx2").unwrap().uid, 1);
}

#[test]
fn upsert_overwrites_and_keeps_other_entries() {
    let mut manifest = Manifest::new("h");
    manifest.replace_subtree(
        "/etc/nginx",
        entries(&[
            ("/etc/nginx", EntryKind::Dir, 0),
            ("/etc/nginx/a.conf", EntryKind::File, 0),
            ("/etc/nginx/gone-locally.conf", EntryKind::File, 0),
        ]),
    );

    manifest.upsert_entries(entries(&[
        ("/etc/nginx/a.conf", EntryKind::File, 7),
        ("/etc/nginx/new.conf", EntryKind::File, 7),
    ]));

    let all: Vec<String> = manifest.entries_under("/etc/nginx").into_keys().collect();
    assert_eq!(
        all,
        [
            "/etc/nginx",
            "/etc/nginx/a.conf",
            "/etc/nginx/gone-locally.conf",
            "/etc/nginx/new.conf"
        ]
    );
    assert_eq!(manifest.entry("/etc/nginx/a.conf").unwrap().uid, 7);
    assert_eq!(manifest.entry("/etc/nginx/gone-locally.conf").unwrap().uid, 0);
}

#[test]
fn entries_under_a_file_is_just_the_file() {
    let mut manifest = Manifest::new("h");
    manifest.replace_subtree("/etc/hosts", entries(&[("/etc/hosts", EntryKind::File, 0)]));

    assert_eq!(manifest.entries_under("/etc/hosts").len(), 1);
    assert!(manifest.entries_under("/etc/host").is_empty());
}

#[test]
fn nearest_dir_ancestor_skips_files_and_the_path_itself() {
    let mut manifest = Manifest::new("h");
    manifest.replace_subtree(
        "/etc",
        entries(&[
            ("/etc", EntryKind::Dir, 1),
            ("/etc/nginx", EntryKind::Dir, 2),
            ("/etc/nginx/nginx.conf", EntryKind::File, 3),
        ]),
    );

    let ancestor = manifest.nearest_dir_ancestor("/etc/nginx/conf.d/new.conf");
    assert_eq!(ancestor.unwrap().uid, 2);

    let ancestor = manifest.nearest_dir_ancestor("/etc/nginx/nginx.conf/child");
    assert_eq!(ancestor.unwrap().uid, 2);

    let ancestor = manifest.nearest_dir_ancestor("/etc/nginx");
    assert_eq!(ancestor.unwrap().uid, 1);

    assert!(manifest.nearest_dir_ancestor("/etc").is_none());
    assert!(manifest.nearest_dir_ancestor("/var/log/x").is_none());
}

#[test]
fn sync_records_are_kept_per_root() {
    let mut manifest = Manifest::new("h");
    manifest.record_sync(
        "/etc/nginx",
        SyncRecord {
            remote_user: "root".to_owned(),
            at_unix: 7,
        },
    );

    assert_eq!(manifest.last_sync("/etc/nginx").unwrap().remote_user, "root");
    assert!(manifest.last_sync("/etc").is_none());
}

#[test]
fn round_trips_through_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/prod-1.json");
    let mut manifest = Manifest::new("prod-1");
    let mut file = meta(EntryKind::File, 0);
    file.size = Some(1234);
    file.sha256 = Some("abc".to_owned());
    file.mode = 0o4755;
    manifest.replace_subtree(
        "/etc/nginx",
        BTreeMap::from([
            ("/etc/nginx".to_owned(), meta(EntryKind::Dir, 0)),
            ("/etc/nginx/nginx.conf".to_owned(), file),
        ]),
    );
    manifest.record_sync(
        "/etc/nginx",
        SyncRecord {
            remote_user: "root".to_owned(),
            at_unix: 1_727_000_000,
        },
    );

    manifest.save_atomic(&path).unwrap();
    let loaded = Manifest::load_or_default(&path, "prod-1").unwrap();

    assert_eq!(loaded, manifest);
    assert_eq!(loaded.entry("/etc/nginx/nginx.conf").unwrap().mode, 0o4755);
}

#[test]
fn json_layout_omits_absent_optional_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.json");
    let mut manifest = Manifest::new("h");
    manifest.replace_subtree("/d", entries(&[("/d", EntryKind::Dir, 0)]));

    manifest.save_atomic(&path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    assert_eq!(json["version"], 1);
    assert_eq!(json["entries"]["/d"]["kind"], "dir");
    assert!(json["entries"]["/d"].get("sha256").is_none());
}

#[test]
fn missing_file_loads_as_empty_manifest() {
    let dir = tempfile::tempdir().unwrap();

    let manifest = Manifest::load_or_default(&dir.path().join("none.json"), "h").unwrap();

    assert_eq!(manifest, Manifest::new("h"));
}

#[test]
fn unknown_version_is_an_error_and_the_file_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.json");
    let original = r#"{"version": 2, "host_key": "h", "entries": {}}"#;
    fs::write(&path, original).unwrap();

    let result = Manifest::load_or_default(&path, "h");

    assert!(matches!(result, Err(WarpSyncError::Manifest(_))));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn corrupt_json_is_an_error_and_the_file_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.json");
    fs::write(&path, "{ not json").unwrap();

    let result = Manifest::load_or_default(&path, "h");

    assert!(matches!(result, Err(WarpSyncError::Manifest(_))));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
}

#[test]
fn save_atomic_leaves_no_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.json");
    let manifest = Manifest::new("h");

    manifest.save_atomic(&path).unwrap();
    manifest.save_atomic(&path).unwrap();

    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["m.json"]);
}
