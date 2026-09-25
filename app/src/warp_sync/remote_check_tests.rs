use super::*;

const A: &str = "/etc/conf/a.conf";
const B: &str = "/etc/conf/b.conf";
const HASH_1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const HASH_2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn file(sha256: Option<&str>) -> EntryMeta {
    EntryMeta {
        kind: EntryKind::File,
        mode: 0o644,
        uid: 0,
        gid: 0,
        uname: "root".to_owned(),
        gname: "root".to_owned(),
        mtime: 0,
        size: None,
        sha256: sha256.map(str::to_owned),
    }
}

fn known(items: &[(&str, EntryMeta)]) -> BTreeMap<String, EntryMeta> {
    items
        .iter()
        .map(|(path, meta)| ((*path).to_owned(), meta.clone()))
        .collect()
}

fn remote(items: &[(&str, &str)]) -> BTreeMap<String, String> {
    items
        .iter()
        .map(|(path, hash)| ((*path).to_owned(), (*hash).to_owned()))
        .collect()
}

fn archive(new_files: &[&str], missing_locally: &[&str]) -> UploadArchive {
    UploadArchive {
        bytes: Vec::new(),
        files: 0,
        dirs: 0,
        content_bytes: 0,
        new_files: new_files.iter().map(|path| (*path).to_owned()).collect(),
        missing_locally: missing_locally
            .iter()
            .map(|path| (*path).to_owned())
            .collect(),
    }
}

#[test]
fn unchanged_files_are_not_conflicts() {
    let conflicts = find_remote_conflicts(
        &known(&[(A, file(Some(HASH_1)))]),
        &archive(&[], &[]),
        &remote(&[(A, HASH_1)]),
    );

    assert!(conflicts.is_empty());
}

#[test]
fn a_file_with_a_different_hash_is_changed() {
    let conflicts = find_remote_conflicts(
        &known(&[(A, file(Some(HASH_1))), (B, file(Some(HASH_1)))]),
        &archive(&[], &[]),
        &remote(&[(A, HASH_2), (B, HASH_1)]),
    );

    assert_eq!(conflicts.changed, vec![A.to_owned()]);
    assert!(conflicts.missing.is_empty());
}

#[test]
fn a_file_the_host_no_longer_reports_is_missing() {
    let conflicts = find_remote_conflicts(
        &known(&[(A, file(Some(HASH_1)))]),
        &archive(&[], &[]),
        &remote(&[]),
    );

    assert_eq!(conflicts.missing, vec![A.to_owned()]);
}

#[test]
fn files_the_upload_does_not_write_are_ignored() {
    let conflicts = find_remote_conflicts(
        &known(&[(A, file(Some(HASH_1)))]),
        &archive(&[], &[A]),
        &remote(&[(A, HASH_2)]),
    );

    assert!(conflicts.is_empty());
}

#[test]
fn entries_without_a_recorded_hash_or_that_are_directories_are_ignored() {
    let directory = EntryMeta {
        kind: EntryKind::Dir,
        sha256: Some(HASH_1.to_owned()),
        ..file(None)
    };
    let conflicts = find_remote_conflicts(
        &known(&[(A, file(None)), ("/etc/conf", directory)]),
        &archive(&[], &[]),
        &remote(&[(A, HASH_2)]),
    );

    assert!(conflicts.is_empty());
}

#[test]
fn a_new_local_file_that_exists_on_the_host_is_a_conflict() {
    let conflicts =
        find_remote_conflicts(&known(&[]), &archive(&[A, B], &[]), &remote(&[(A, HASH_1)]));

    assert_eq!(conflicts.already_exist, vec![A.to_owned()]);
    assert!(!conflicts.is_empty());
}
