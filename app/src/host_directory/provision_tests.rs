use std::cell::RefCell;

use tempfile::TempDir;

use super::*;
use crate::host_directory::WARP_CONF_FILE;

/// Answers like `ssh -G` would for a `warp.conf` that says what `NewHost` says, unless told to
/// answer something else or to fail.
#[derive(Default)]
struct FakeSsh {
    answer: RefCell<Option<Result<Resolved, String>>>,
}

impl FakeSsh {
    fn answering(resolved: Resolved) -> Self {
        Self {
            answer: RefCell::new(Some(Ok(resolved))),
        }
    }

    fn failing(reason: &str) -> Self {
        Self {
            answer: RefCell::new(Some(Err(reason.to_owned()))),
        }
    }
}

impl SshResolver for FakeSsh {
    fn resolve(&self, config: Option<&Path>, _alias: &str) -> Result<Resolved, String> {
        if let Some(answer) = self.answer.borrow().clone() {
            return answer;
        }
        // Read the block back the way OpenSSH would: the options of the one host in the file.
        let text = fs::read_to_string(config.expect("warp.conf is resolved on its own"))
            .map_err(|err| err.to_string())?;
        let value = |keyword: &str| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix(keyword)?.strip_prefix(' '))
                .map(str::to_owned)
        };
        Ok(Resolved {
            hostname: value("HostName").unwrap_or_default(),
            user: value("User").unwrap_or_else(|| "me".to_owned()),
            port: value("Port").map_or(22, |port| port.parse().unwrap_or(0)),
            proxy_jump: value("ProxyJump"),
            identity_files: value("IdentityFile").into_iter().collect(),
        })
    }
}

fn resolved(hostname: &str, user: &str, port: u16) -> Resolved {
    Resolved {
        hostname: hostname.to_owned(),
        user: user.to_owned(),
        port,
        proxy_jump: None,
        identity_files: Vec::new(),
    }
}

fn new_host(alias: &str) -> NewHost {
    NewHost {
        alias: alias.to_owned(),
        hostname: "203.0.113.9".to_owned(),
        user: Some("ops".to_owned()),
        port: Some(2222),
        tags: vec!["prod".to_owned()],
        ..NewHost::default()
    }
}

fn home() -> TempDir {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".ssh")).unwrap();
    home
}

fn read(home: &TempDir, relative: &str) -> String {
    fs::read_to_string(home.path().join(relative)).unwrap()
}

fn write(home: &TempDir, relative: &str, text: &str) {
    let path = home.path().join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

const USER_CONFIG: &str = "Host prod-db\n    HostName db.example.com\n";

#[test]
fn creates_a_host_in_warp_conf_and_the_directory() {
    let home = home();
    write(&home, ".ssh/config", USER_CONFIG);

    let hosts = create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::SudoNopasswd,
        &FakeSsh::default(),
    )
    .unwrap();

    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].source, HostSource::Warp);
    assert_eq!(hosts[0].tags, ["prod"]);
    assert_eq!(hosts[0].root_login, RootLogin::SudoNopasswd);
    let conf = read(&home, WARP_CONF_FILE);
    assert!(conf.contains("# warp:tags=prod\nHost lab\n    HostName 203.0.113.9\n"));
    assert_eq!(store::load(home.path()).unwrap(), hosts);
    assert_eq!(read(&home, ".ssh/config"), USER_CONFIG);
}

#[cfg(unix)]
#[test]
fn warp_conf_is_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = home();
    create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap();

    let mode = fs::metadata(home.path().join(WARP_CONF_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn refuses_an_alias_the_user_already_defined() {
    let home = home();
    write(&home, ".ssh/config", USER_CONFIG);

    let error = create_host(
        home.path(),
        &new_host("prod-db"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap_err();

    assert!(
        matches!(error, ProvisionError::AlreadyDefined { .. }),
        "{error}"
    );
    assert!(!home.path().join(WARP_CONF_FILE).exists());
    assert!(!home.path().join(".warp/agent-ops/hosts.toml").exists());
}

#[test]
fn refuses_an_alias_that_is_already_in_the_directory() {
    let home = home();
    create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap();
    let before = read(&home, WARP_CONF_FILE);

    let error = create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap_err();

    assert!(
        matches!(error, ProvisionError::AlreadyDefined { .. }),
        "{error}"
    );
    assert_eq!(read(&home, WARP_CONF_FILE), before);
}

#[test]
fn takes_over_a_host_that_left_the_ssh_config_keeping_what_warp_knew() {
    let home = home();
    let mut gone = Host::new("lab", HostSource::SshConfig);
    gone.missing = true;
    gone.mirror_key = Some("mirror".to_owned());
    gone.machine_id = Some("abc".to_owned());
    store::save(home.path(), &[gone]).unwrap();

    let hosts = create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::Root,
        &FakeSsh::default(),
    )
    .unwrap();

    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].source, HostSource::Warp);
    assert!(!hosts[0].missing);
    assert_eq!(hosts[0].mirror_key.as_deref(), Some("mirror"));
    assert_eq!(hosts[0].machine_id.as_deref(), Some("abc"));
}

#[test]
fn refuses_a_host_that_cannot_be_written_as_one_block() {
    let home = home();
    let mut injected = new_host("lab");
    injected.hostname = "x\n    ProxyCommand touch /tmp/pwned".to_owned();

    let error =
        create_host(home.path(), &injected, RootLogin::None, &FakeSsh::default()).unwrap_err();

    assert!(matches!(error, ProvisionError::Invalid(_)), "{error}");
    assert!(!home.path().join(WARP_CONF_FILE).exists());
}

#[test]
fn undoes_the_write_when_ssh_reads_the_host_differently() {
    let home = home();
    write(&home, WARP_CONF_FILE, "# old\n");

    let error = create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::None,
        &FakeSsh::answering(resolved("198.51.100.1", "ops", 2222)),
    )
    .unwrap_err();

    assert!(matches!(error, ProvisionError::Verify(_)), "{error}");
    assert_eq!(read(&home, WARP_CONF_FILE), "# old\n");
    assert!(!home.path().join(".warp/agent-ops/hosts.toml").exists());
}

#[test]
fn removes_a_new_warp_conf_when_ssh_fails() {
    let home = home();

    let error = create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::None,
        &FakeSsh::failing("Bad configuration option"),
    )
    .unwrap_err();

    assert!(matches!(error, ProvisionError::Verify(_)), "{error}");
    assert!(!home.path().join(WARP_CONF_FILE).exists());
}

#[test]
fn keeps_the_previous_warp_conf_beside_the_new_one() {
    let home = home();
    create_host(
        home.path(),
        &new_host("one"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap();
    let before = read(&home, WARP_CONF_FILE);

    create_host(
        home.path(),
        &new_host("two"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap();

    assert_eq!(read(&home, ".ssh/config.d/warp.conf.bak"), before);
}

#[test]
fn removing_a_warp_host_removes_its_block_and_leaves_the_others() {
    let home = home();
    create_host(
        home.path(),
        &new_host("one"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap();
    create_host(
        home.path(),
        &new_host("two"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap();

    let hosts = remove_host(home.path(), "one").unwrap();

    assert_eq!(
        hosts.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(),
        ["two"]
    );
    let conf = read(&home, WARP_CONF_FILE);
    assert!(!conf.contains("Host one"));
    assert!(conf.contains("Host two"));
}

#[test]
fn forgetting_a_host_from_the_ssh_config_leaves_the_ssh_files_alone() {
    let home = home();
    write(&home, ".ssh/config", USER_CONFIG);
    write(&home, WARP_CONF_FILE, "Host other\n    HostName o\n");
    store::save(home.path(), &[Host::new("prod-db", HostSource::SshConfig)]).unwrap();

    let hosts = remove_host(home.path(), "prod-db").unwrap();

    assert!(hosts.is_empty());
    assert_eq!(read(&home, ".ssh/config"), USER_CONFIG);
    assert_eq!(read(&home, WARP_CONF_FILE), "Host other\n    HostName o\n");
}

#[test]
fn removing_an_unknown_host_is_an_error() {
    let home = home();

    let error = remove_host(home.path(), "nope").unwrap_err();

    assert!(matches!(error, ProvisionError::Invalid(_)), "{error}");
}

#[test]
fn editing_a_warp_host_rewrites_its_tags_comment() {
    let home = home();
    create_host(
        home.path(),
        &new_host("lab"),
        RootLogin::None,
        &FakeSsh::default(),
    )
    .unwrap();
    let edit = HostEdit {
        tags: vec!["db".to_owned(), "eu".to_owned()],
        root_login: RootLogin::SudoPassword,
        transport: Transport::Direct,
    };

    let hosts = update_host(home.path(), "lab", &edit).unwrap();

    assert_eq!(hosts[0].tags, ["db", "eu"]);
    assert_eq!(hosts[0].root_login, RootLogin::SudoPassword);
    assert_eq!(hosts[0].transport, Transport::Direct);
    let conf = read(&home, WARP_CONF_FILE);
    assert!(conf.contains("# warp:tags=db,eu\nHost lab\n"));
    assert!(!conf.contains("prod"));
}

#[test]
fn editing_a_host_from_the_ssh_config_only_changes_the_directory() {
    let home = home();
    write(&home, ".ssh/config", USER_CONFIG);
    write(&home, WARP_CONF_FILE, "Host other\n    HostName o\n");
    store::save(home.path(), &[Host::new("prod-db", HostSource::SshConfig)]).unwrap();
    let edit = HostEdit {
        tags: vec!["prod".to_owned()],
        root_login: RootLogin::Root,
        transport: Transport::InBand,
    };

    let hosts = update_host(home.path(), "prod-db", &edit).unwrap();

    assert_eq!(hosts[0].tags, ["prod"]);
    assert_eq!(read(&home, ".ssh/config"), USER_CONFIG);
    assert_eq!(read(&home, WARP_CONF_FILE), "Host other\n    HostName o\n");
}

#[test]
fn installs_the_include_at_the_top_and_backs_up_the_old_file() {
    let home = home();
    write(&home, ".ssh/config", USER_CONFIG);
    assert!(!include_installed(home.path()));

    let backup = install_include(home.path()).unwrap().unwrap();

    assert_eq!(
        read(&home, ".ssh/config"),
        format!("{}\n\n{USER_CONFIG}", warp_conf::INCLUDE_LINE)
    );
    assert_eq!(fs::read_to_string(backup).unwrap(), USER_CONFIG);
    assert!(include_installed(home.path()));
}

#[test]
fn installing_the_include_twice_changes_nothing() {
    let home = home();
    write(&home, ".ssh/config", USER_CONFIG);
    install_include(home.path()).unwrap();
    let after_first = read(&home, ".ssh/config");

    let second = install_include(home.path()).unwrap();

    assert_eq!(second, None);
    assert_eq!(read(&home, ".ssh/config"), after_first);
}

#[test]
fn installing_the_include_keeps_every_other_byte() {
    let home = home();
    let crlf = "Host a\r\n    HostName a.example\r\n\r\nHost b\r\n    HostName b.example";
    write(&home, ".ssh/config", crlf);

    install_include(home.path()).unwrap();

    assert_eq!(
        read(&home, ".ssh/config"),
        format!("{}\n\n{crlf}", warp_conf::INCLUDE_LINE)
    );
}

#[test]
fn installs_the_include_when_there_is_no_ssh_config() {
    let home = home();

    let backup = install_include(home.path()).unwrap();

    assert_eq!(backup, None);
    assert_eq!(
        read(&home, ".ssh/config"),
        format!("{}\n", warp_conf::INCLUDE_LINE)
    );
}

#[cfg(unix)]
#[test]
fn installing_the_include_keeps_the_permissions_of_the_ssh_config() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = home();
    write(&home, ".ssh/config", USER_CONFIG);
    let path = home.path().join(".ssh/config");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

    install_include(home.path()).unwrap();

    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[cfg(unix)]
#[test]
fn installing_the_include_writes_through_a_symlinked_ssh_config() {
    let home = home();
    write(&home, "dotfiles/ssh_config", USER_CONFIG);
    let link = home.path().join(".ssh/config");
    std::os::unix::fs::symlink(home.path().join("dotfiles/ssh_config"), &link).unwrap();

    install_include(home.path()).unwrap();

    assert!(link.is_symlink());
    assert!(read(&home, "dotfiles/ssh_config").starts_with(warp_conf::INCLUDE_LINE));
}
