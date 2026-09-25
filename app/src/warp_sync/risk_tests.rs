use super::*;

fn modes(items: &[(&str, u32)]) -> BTreeMap<String, u32> {
    items
        .iter()
        .map(|(path, mode)| ((*path).to_owned(), *mode))
        .collect()
}

fn paths(items: &[&str]) -> Vec<String> {
    items.iter().map(|path| (*path).to_owned()).collect()
}

#[test]
fn places_where_the_host_runs_what_it_finds_are_flagged() {
    for path in [
        "/etc/cron.d/backup",
        "/etc/cron.daily/x",
        "/etc/crontab",
        "/var/spool/cron/crontabs/root",
        "/etc/sudoers.d/90-me",
        "/etc/sudoers",
        "/etc/profile",
        "/etc/profile.d/x.sh",
        "/etc/systemd/system/x.service",
        "/usr/lib/systemd/system/x.service",
        "/etc/init.d/x",
        "/etc/rc.local",
        "/etc/pam.d/sshd",
        "/etc/ssh/sshd_config.d/x.conf",
        "/etc/ld.so.preload",
        "/etc/passwd",
        "/usr/local/bin/tool",
        "/usr/bin/tool",
        "/sbin/tool",
        "/root/.ssh/authorized_keys",
        "/home/me/.ssh",
        "/home/me/.bashrc",
        "/home/me/.zshenv",
        "/home/me/.profile",
        "/home/me/.config/autostart/x.desktop",
        "/home/me/.config/systemd/user/x.service",
        "/srv/deploy/.ssh/rc",
    ] {
        let risks = assess(&paths(&[path]), &BTreeMap::new());

        assert_eq!(risks.runs_code, paths(&[path]), "{path}");
    }
}

#[test]
fn ordinary_paths_and_look_alikes_are_not_flagged() {
    for path in [
        "/etc/nginx/nginx.conf",
        "/etc/cron.dx/y",
        "/etc/crontabs",
        "/etc/profiled",
        "/etc/sshd",
        "/usr/bin2/x",
        "/binx/tool",
        "/root/test-dir/a.conf",
        "/home/me/notes.txt",
        "/home/me/ssh/config",
        "/home/me/bashrc",
        "/home/me/.config/other/x",
        "/tmp/a",
    ] {
        let risks = assess(&paths(&[path]), &BTreeMap::new());

        assert!(risks.runs_code.is_empty(), "{path}");
    }
}

#[test]
fn a_new_entry_that_anyone_can_write_is_flagged_whatever_its_kind_or_place() {
    let new_files = paths(&["/srv/a", "/srv/b", "/srv/c", "/srv/d", "/srv/e", "/srv/f"]);
    let new_modes = modes(&[
        ("/srv/a", 0o666),
        ("/srv/b", 0o777),
        ("/srv/c", 0o644),
        ("/srv/d", 0o775),
        ("/srv/e", 0o602),
        ("/srv/f", 0o600),
    ]);

    let risks = assess(&new_files, &new_modes);

    assert_eq!(risks.world_writable, paths(&["/srv/a", "/srv/b", "/srv/e"]));
    assert!(risks.runs_code.is_empty());
}

#[test]
fn an_entry_can_be_both_and_the_order_of_the_upload_is_kept() {
    let new_files = paths(&["/etc/cron.d", "/etc/cron.d/job", "/srv/x"]);
    let new_modes = modes(&[("/etc/cron.d/job", 0o666), ("/srv/x", 0o666)]);

    let risks = assess(&new_files, &new_modes);

    assert_eq!(risks.runs_code, paths(&["/etc/cron.d", "/etc/cron.d/job"]));
    assert_eq!(risks.world_writable, paths(&["/etc/cron.d/job", "/srv/x"]));
}

#[test]
fn a_new_entry_without_a_known_mode_is_not_called_world_writable() {
    let risks = assess(&paths(&["/srv/a"]), &BTreeMap::new());

    assert_eq!(risks, UploadRisks::default());
}

#[test]
fn nothing_new_means_nothing_to_warn_about() {
    assert_eq!(assess(&[], &BTreeMap::new()), UploadRisks::default());
}
