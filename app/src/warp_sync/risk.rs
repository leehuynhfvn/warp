//! Things worth a second look before an upload creates them on the remote host. The user is
//! always asked first; these are what the question should draw attention to.

use std::collections::BTreeMap;

/// Permission bit that lets any user on the host write.
const OTHERS_WRITE: u32 = 0o002;

/// Places where the host runs, or trusts without asking, what it finds. A new file there is
/// code execution or a change of who may log in, not just a file.
const CODE_PATH_PREFIXES: &[&str] = &[
    "/bin",
    "/boot",
    "/etc/anacrontab",
    "/etc/apt/apt.conf.d",
    "/etc/bash.bashrc",
    "/etc/bashrc",
    "/etc/cron.d",
    "/etc/cron.daily",
    "/etc/cron.hourly",
    "/etc/cron.monthly",
    "/etc/cron.weekly",
    "/etc/crontab",
    "/etc/environment",
    "/etc/group",
    "/etc/gshadow",
    "/etc/init",
    "/etc/init.d",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    "/etc/ld.so.preload",
    "/etc/logrotate.d",
    "/etc/modprobe.d",
    "/etc/modules-load.d",
    "/etc/network",
    "/etc/pam.d",
    "/etc/passwd",
    "/etc/profile",
    "/etc/profile.d",
    "/etc/rc.d",
    "/etc/rc.local",
    "/etc/security",
    "/etc/shadow",
    "/etc/ssh",
    "/etc/sudoers",
    "/etc/sudoers.d",
    "/etc/sysctl.d",
    "/etc/systemd",
    "/etc/udev/rules.d",
    "/etc/update-motd.d",
    "/etc/xdg/autostart",
    "/etc/zsh",
    "/lib/systemd",
    "/lib/udev",
    "/sbin",
    "/usr/bin",
    "/usr/lib/systemd",
    "/usr/lib/udev",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/sbin",
    "/var/spool/cron",
];

/// File names in a user's home that a shell or a login session runs.
const STARTUP_FILE_NAMES: &[&str] = &[
    ".bash_login",
    ".bash_logout",
    ".bash_profile",
    ".bashrc",
    ".forward",
    ".pam_environment",
    ".profile",
    ".xinitrc",
    ".xprofile",
    ".xsession",
    ".zlogin",
    ".zlogout",
    ".zprofile",
    ".zshenv",
    ".zshrc",
];

/// A directory whose contents are trusted for logging in, wherever in the tree it is.
const SSH_DIR_NAME: &str = ".ssh";

/// `.config` subdirectories that start programs.
const AUTOSTART_CONFIG_DIRS: &[&str] = &["autostart", "systemd"];

/// New entries of an upload that deserve a warning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UploadRisks {
    /// Anyone on the host could change these.
    pub world_writable: Vec<String>,
    /// Places where the host runs or trusts what it finds.
    pub runs_code: Vec<String>,
}

/// Looks at the entries the upload creates (`new_files`, with the mode each gets in
/// `new_modes`), in upload order. Entries that already exist are left alone: the user synced them
/// and sees what changed.
pub fn assess(new_files: &[String], new_modes: &BTreeMap<String, u32>) -> UploadRisks {
    let mut risks = UploadRisks::default();
    for path in new_files {
        if new_modes
            .get(path)
            .is_some_and(|mode| mode & OTHERS_WRITE != 0)
        {
            risks.world_writable.push(path.clone());
        }
        if runs_code(path) {
            risks.runs_code.push(path.clone());
        }
    }
    risks
}

fn runs_code(path: &str) -> bool {
    let in_system_place = CODE_PATH_PREFIXES.iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    in_system_place || in_user_startup_place(path)
}

fn in_user_startup_place(path: &str) -> bool {
    let components: Vec<&str> = path
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    let is_startup_file = components
        .last()
        .is_some_and(|name| STARTUP_FILE_NAMES.contains(name));
    let is_in_ssh_dir = components.contains(&SSH_DIR_NAME);
    let is_in_autostart_dir = components
        .windows(2)
        .any(|pair| pair[0] == ".config" && AUTOSTART_CONFIG_DIRS.contains(&pair[1]));
    is_startup_file || is_in_ssh_dir || is_in_autostart_dir
}

#[cfg(test)]
#[path = "risk_tests.rs"]
mod tests;
