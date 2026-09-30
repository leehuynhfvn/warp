---
name: warp-remote-ops
description: Operate on a remote server through a Warp terminal session with the warp-bridge MCP tools (list_sessions, exec, exec_visible, read_file, edit_file, write_file, recent_output). Use when the user asks to inspect, diagnose, configure or fix something "on the server", "on prod-1", "in my Warp session", or names a host that is open in Warp.
---

# Remote operations through Warp

The `mcp__warp-bridge__*` tools act on a **remote server** through a terminal session the user
opened in Warp (`ssh`, often followed by `sudo -i`). Commands usually run **as root**. They are
not your local shell: Bash, Read and Edit still act on this machine.

## Before anything else

1. Call `list_sessions`. Use only sessions marked `attached`. If none is attached, ask the user to
   run **"Agent Bridge: Allow agents to control this session"** from the Warp command palette in
   the pane of that server, then stop.
2. Say which `user@host` you are about to work on. If several sessions are attached, pass
   `session_id` on every call.
3. Read-only attachments allow `read_file` and `recent_output` only.

## Workflow

1. **Diagnose**. When the user mentions something they just ran or an error they just saw, call
   `recent_output` first: it returns their latest commands with exit code and output, without
   running anything. Then use read-only commands: `systemctl status --no-pager <unit>`,
   `journalctl -u <unit> -n 100 --no-pager`, `ss -tlnp`, `df -h`, `free -m`, `read_file` on the
   config. Narrow long output with `grep`, `head`, `tail`; output is cut in the middle past a few
   dozen KiB.
2. **Propose** the change: which file, which lines, why, and how you will check it. Wait for the
   user's go-ahead before changing anything that is not trivially reversible.
3. **Change** config files with `edit_file` (small, exact `old_string`; `read_file` first). Use
   `write_file` only for new files. Every overwrite is backed up on the server under
   `~/.warp-agent/backups/`; mention the backup path to the user.
4. **Check syntax** before any reload:

   | Service | Check |
   |---|---|
   | nginx | `nginx -t` |
   | Apache | `apachectl configtest` |
   | OpenSSH | `sshd -t` |
   | sudoers | `visudo -c` (never edit `/etc/sudoers` without it) |
   | systemd unit | `systemd-analyze verify <unit>` then `systemctl daemon-reload` |
   | HAProxy | `haproxy -c -f /etc/haproxy/haproxy.cfg` |
   | Postfix | `postfix check` |
   | BIND | `named-checkconf`, `named-checkzone <zone> <file>` |
   | Prometheus | `promtool check config <file>` |

5. **Apply** with the gentlest action: `reload` before `restart`.
6. **Verify**: service status, a request against it (`curl -sS -o /dev/null -w '%{http_code}'
   …`), recent logs. If it got worse, restore the backup and reload again, then report.

## Running a command the user can see

`exec` runs out of sight, in a subshell. `exec_visible` types the command into the user's own
shell instead, where it runs as a block they watch. Use it only when:

- the user asks to see the command run, or to follow along;
- a `cd` or `export` has to stay in effect for the user afterwards;
- the command may ask something the user should answer in the terminal (a confirmation, a
  passphrase).

The command must be a single line (join steps with `;` or `&&`). It enters the shell history on the server, and its output is what the terminal shows (stdout and
stderr together, secrets hidden per the user's settings). Never send `exit`, `logout`, `exec …`,
`su` or `sudo -i` with it: leaving the shell ends your access to the session. If the result says
the command is **still running**, it keeps running in the terminal: tell the user, and read the
result later with `recent_output` instead of running it again.

## Ask again before running

Confirm with the user in plain words, even if they approved the overall task:

- Restarting or reconfiguring `sshd`, networking (`ip`, `nmcli`, `netplan apply`,
  `/etc/network/*`), the firewall (`iptables`, `nft`, `ufw`, `firewalld`): a mistake can lock
  everyone out, including this session.
- `reboot`, `shutdown`, `poweroff`, `systemctl isolate`, `kill -9` of system services.
- Deleting data: `rm -r`, `find … -delete`, `truncate`, `dd`, `mkfs`, `wipefs`, `fdisk`/`parted`,
  `lvremove`, `zfs destroy`, `DROP DATABASE`.
- Package operations that remove or upgrade: `apt-get remove/purge/dist-upgrade`, `dnf remove`,
  `yum update`.
- Changes to users and access: `passwd`, `usermod`, `userdel`, `authorized_keys`, sudoers, PAM.
- Anything with `curl | sh` or that downloads and runs code.

## Limits of the tools

- No terminal and no stdin: pass `-y`, `--no-pager`, `-n`; interactive programs (`vim`, `top`,
  `less`, password prompts) fail or hang until the timeout.
- `exec` times out after 120 s by default (`timeout_secs` up to 600). The user's shell must be
  idle; if the user is running something, you get `session_busy` — wait and retry.
- `exec` commands run in a subshell: `cd` and `export` do not persist between calls; use `cwd`
  (or `exec_visible`, see above).
- Text that looks like a secret is shown as `****`. `edit_file` cannot match it and `write_file`
  refuses to overwrite a file that contains it: edit around the secret lines.
- Files larger than 512 KiB cannot be read or written; use `exec` with `grep`, `sed -n`, `tail`.
- Warp may ask the user to approve a write before it runs; the call then waits up to 5 minutes.
  If the result says "Denied by Warp's agent policy", do not retry the same command or rephrase
  it to get around the rule — tell the user why it was denied. Keep commands short and
  single-purpose so the user can review them.

## Opening a session yourself (`open_session`, `close_session`)

When no session of the server you need is attached, do not ask the user to set one up by hand if
`list_hosts` shows the server: call `open_session` with its alias.

- `purpose` is read by the person who approves it. Say the real reason in a few words.
- Ask for `access: "read_only"` unless you must change something. `full` still does not run a
  command by itself: every `exec` and write goes through the user's policy one by one.
- Ask for `root: true` only when the task needs root. It works where the server's entry says the
  user has `sudo` without a password; otherwise the result says it was not reached.
- The user may be asked to approve, and the call waits for it. If it is denied, do not open the
  session again or find another way in; tell the user.
- A result that says the session is not ready means the user may have to answer something in
  its tab (a password, a passphrase, a host key). Do not open another one; check `list_sessions`
  after a while.
- Open only as many sessions as you need (each server has a limit) and call `close_session` on
  each when you are done. You can close only sessions you opened yourself; the ones the user
  opened stay open.
- Opening needs this agent to be paired with Warp. If the result says it is not paired, tell the
  user; do not retry.
