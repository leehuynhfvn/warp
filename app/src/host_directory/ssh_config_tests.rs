use std::fs;

use super::*;

/// A fake home directory whose `.ssh` holds the given files (paths relative to `.ssh`).
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let path = dir.path().join(".ssh").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        Self { dir }
    }

    fn scan(&self) -> Discovery {
        let ssh_dir = self.dir.path().join(".ssh");
        discover_aliases(&ssh_dir.join("config"), &ssh_dir, self.dir.path())
    }

    fn ssh(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(".ssh").join(name)
    }
}

fn aliases(discovery: &Discovery) -> Vec<&str> {
    discovery
        .aliases
        .iter()
        .map(|host| host.alias.as_str())
        .collect()
}

fn line(keyword: &str, args: &[&str]) -> Option<Line> {
    Some(Line {
        keyword: keyword.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    })
}

#[test]
fn tokenizes_a_keyword_and_its_arguments() {
    assert_eq!(
        tokenize("Host web01 web02"),
        line("host", &["web01", "web02"])
    );
    assert_eq!(tokenize("   Host\tweb01"), line("host", &["web01"]));
    assert_eq!(tokenize("HOST web01"), line("host", &["web01"]));
}

#[test]
fn a_keyword_and_its_arguments_may_be_separated_by_an_equals_sign() {
    assert_eq!(tokenize("Host=web01"), line("host", &["web01"]));
    assert_eq!(tokenize("Host = web01"), line("host", &["web01"]));
    assert_eq!(
        tokenize("Host= web01 web02"),
        line("host", &["web01", "web02"])
    );
}

#[test]
fn an_equals_sign_inside_an_argument_is_kept() {
    assert_eq!(tokenize("SetEnv A=b"), line("setenv", &["A=b"]));
}

#[test]
fn a_quoted_argument_keeps_its_white_space() {
    assert_eq!(
        tokenize(r#"Host "my host" web"#),
        line("host", &["my host", "web"])
    );
    assert_eq!(
        tokenize(r#"Include "a \"b\" c""#),
        line("include", &[r#"a "b" c"#])
    );
}

#[test]
fn a_comment_ends_the_line() {
    assert_eq!(tokenize("Host web01 # the first"), line("host", &["web01"]));
    assert_eq!(tokenize("Host web01 #the first"), line("host", &["web01"]));
    assert_eq!(tokenize("# Host nope"), None);
    assert_eq!(tokenize("   # Host nope"), None);
    assert_eq!(tokenize(""), None);
    assert_eq!(tokenize("   \t"), None);
}

#[test]
fn a_hash_inside_a_word_is_not_a_comment() {
    assert_eq!(tokenize("Host web#1"), line("host", &["web#1"]));
}

#[test]
fn finds_the_aliases_of_host_lines() {
    let home = Home::new(&[(
        "config",
        "Host web01\n    HostName 10.0.0.1\nHost web02 web03\n",
    )]);
    let discovery = home.scan();
    assert_eq!(aliases(&discovery), ["web01", "web02", "web03"]);
    assert_eq!(discovery.aliases[0].file, home.ssh("config"));
    assert_eq!(discovery.aliases[0].line, 1);
    assert_eq!(discovery.aliases[1].line, 3);
}

#[test]
fn skips_wildcards_negations_and_the_catch_all() {
    let home = Home::new(&[(
        "config",
        "Host web01 *.tyo web0? !bad *\nHost *\n    ServerAliveInterval 30\n",
    )]);
    assert_eq!(aliases(&home.scan()), ["web01"]);
}

#[test]
fn a_match_block_gives_no_alias() {
    let home = Home::new(&[(
        "config",
        "Match host web01 user root\n    Port 22\nHost web02\n",
    )]);
    assert_eq!(aliases(&home.scan()), ["web02"]);
}

#[test]
fn skips_an_alias_that_could_not_be_typed_safely_and_says_so() {
    let home = Home::new(&[("config", "Host ok -oProxyCommand=x a;b máy\n")]);
    let discovery = home.scan();
    assert_eq!(aliases(&discovery), ["ok"]);
    assert_eq!(discovery.warnings.len(), 3);
}

#[test]
fn an_alias_defined_twice_is_found_once_at_its_first_place() {
    let home = Home::new(&[("config", "Host web01\nHost web01\n")]);
    let discovery = home.scan();
    assert_eq!(aliases(&discovery), ["web01"]);
    assert_eq!(discovery.aliases[0].line, 1);
}

#[test]
fn handles_windows_line_endings_and_a_missing_final_newline() {
    let home = Home::new(&[("config", "Host web01\r\n  User root\r\nHost web02")]);
    assert_eq!(aliases(&home.scan()), ["web01", "web02"]);
}

#[test]
fn a_missing_config_is_an_empty_result_without_a_warning() {
    let home = Home::new(&[]);
    let discovery = home.scan();
    assert_eq!(discovery, Discovery::default());
}

#[test]
fn follows_a_relative_include_from_the_ssh_directory() {
    let home = Home::new(&[
        ("config", "Include config.work\nHost first\n"),
        ("config.work", "Host work01\n"),
    ]);
    let discovery = home.scan();
    assert_eq!(aliases(&discovery), ["work01", "first"]);
    assert_eq!(
        discovery.files,
        [home.ssh("config"), home.ssh("config.work")]
    );
}

#[test]
fn follows_a_tilde_and_an_absolute_include() {
    let home = Home::new(&[("config.a", "Host a\n"), ("config.b", "Host b\n")]);
    let absolute = home.ssh("config.b");
    fs::write(
        home.ssh("config"),
        format!("Include ~/.ssh/config.a\nInclude {}\n", absolute.display()),
    )
    .unwrap();
    assert_eq!(aliases(&home.scan()), ["a", "b"]);
}

#[test]
fn an_include_may_name_several_files() {
    let home = Home::new(&[
        ("config", "Include one two\n"),
        ("one", "Host a\n"),
        ("two", "Host b\n"),
    ]);
    assert_eq!(aliases(&home.scan()), ["a", "b"]);
}

#[test]
fn an_include_glob_reads_matching_files_in_sorted_order() {
    let home = Home::new(&[
        ("config", "Include conf.d/*.conf\n"),
        ("conf.d/20-b.conf", "Host b\n"),
        ("conf.d/10-a.conf", "Host a\n"),
        ("conf.d/notes.txt", "Host ignored\n"),
        ("conf.d/.hidden.conf", "Host hidden\n"),
    ]);
    assert_eq!(aliases(&home.scan()), ["a", "b"]);
}

#[test]
fn a_glob_may_have_a_wildcard_in_a_directory_name() {
    let home = Home::new(&[
        ("config", "Include sites/*/ssh\n"),
        ("sites/x/ssh", "Host x1\n"),
        ("sites/y/ssh", "Host y1\n"),
    ]);
    assert_eq!(aliases(&home.scan()), ["x1", "y1"]);
}

#[test]
fn an_include_that_matches_nothing_is_silent() {
    let home = Home::new(&[("config", "Include nothing/*\nInclude nowhere\nHost a\n")]);
    let discovery = home.scan();
    assert_eq!(aliases(&discovery), ["a"]);
    assert!(discovery.warnings.is_empty());
}

#[test]
fn an_include_that_needs_expansion_is_skipped_with_a_warning() {
    let home = Home::new(&[("config", "Include %d/x ${HOME}/y ~other/z\nHost a\n")]);
    let discovery = home.scan();
    assert_eq!(aliases(&discovery), ["a"]);
    assert_eq!(discovery.warnings.len(), 3);
}

#[test]
fn an_include_that_leads_back_to_itself_stops() {
    let home = Home::new(&[
        ("config", "Host a\nInclude other\n"),
        ("other", "Host b\nInclude config\n"),
    ]);
    let discovery = home.scan();
    assert_eq!(aliases(&discovery), ["a", "b"]);
    assert_eq!(discovery.warnings.len(), 1);
}

#[test]
fn a_file_included_twice_without_a_cycle_is_read_twice_but_its_hosts_are_found_once() {
    let home = Home::new(&[
        ("config", "Include shared\nInclude shared\n"),
        ("shared", "Host a\n"),
    ]);
    assert_eq!(aliases(&home.scan()), ["a"]);
}

#[test]
fn stops_following_includes_nested_too_deeply() {
    let mut files = Vec::new();
    let names: Vec<String> = (0..20).map(|n| format!("level{n}")).collect();
    for (index, name) in names.iter().enumerate() {
        let next = names
            .get(index + 1)
            .map(|next| format!("Include {next}\n"))
            .unwrap_or_default();
        files.push((name.clone(), format!("Host h{index}\n{next}")));
    }
    let mut refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    refs.push(("config", "Include level0\n"));
    let home = Home::new(&refs);
    let discovery = home.scan();
    assert!(discovery.aliases.len() <= 17, "{}", discovery.aliases.len());
    assert!(
        discovery
            .warnings
            .iter()
            .any(|w| w.contains("nested too deeply"))
    );
}

#[test]
fn a_file_larger_than_the_limit_is_skipped_with_a_warning() {
    let big = format!("Host a\n{}", "# padding\n".repeat(120_000));
    let home = Home::new(&[("config", &big)]);
    let discovery = home.scan();
    assert!(discovery.aliases.is_empty());
    assert_eq!(discovery.warnings.len(), 1);
}

#[test]
fn an_include_of_a_directory_is_ignored() {
    let home = Home::new(&[
        ("config", "Include dir\nHost a\n"),
        ("dir/inner", "Host b\n"),
    ]);
    assert_eq!(aliases(&home.scan()), ["a"]);
}

/// The shape of a real configuration: a main file that only includes others, hosts written
/// several to a line together with wildcards, `Match` blocks and options.
#[test]
fn reads_a_configuration_shaped_like_a_real_one() {
    let mut fleet = String::from("Host");
    for n in 1..=60 {
        fleet.push_str(&format!(" node{n:02}"));
    }
    fleet.push_str(" *.ty8 *.tyo\n    User ops\n    ProxyJump gw01\n");
    let home = Home::new(&[
        (
            "config",
            "Include config.fleet\nInclude config.home\n\nHost gw01\n    HostName 203.0.113.1\n\nHost *\n    ServerAliveInterval 30\n",
        ),
        ("config.fleet", &fleet),
        (
            "config.home",
            "# home lab\nHost pve pve2 # hypervisors\n    User root\nMatch host nas exec \"true\"\n    Port 2222\nHost nas\n",
        ),
    ]);
    let discovery = home.scan();
    let found = aliases(&discovery);
    assert_eq!(found.len(), 60 + 4);
    assert_eq!(found[0], "node01");
    assert_eq!(found[59], "node60");
    assert_eq!(&found[60..], ["pve", "pve2", "nas", "gw01"]);
    assert!(discovery.warnings.is_empty(), "{:?}", discovery.warnings);
}

#[test]
fn expand_glob_returns_only_files() {
    let home = Home::new(&[("a/file", ""), ("a/sub/file", "")]);
    let found = expand_glob(&home.ssh("a/*"));
    assert_eq!(found, [home.ssh("a/file")]);
    assert!(expand_glob(&home.ssh("a/sub")).is_empty());
}
