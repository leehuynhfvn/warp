---
name: warp-remote-ops
description: Operate on a remote server through a Warp terminal session with the warp-bridge MCP tools (list_sessions, exec, read_file, edit_file, write_file, recent_output). Use when the user asks to inspect, diagnose, configure or fix something "on the server", "on prod-1", "in my Warp session", or names a host that is open in Warp.
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
- Commands run in a subshell: `cd` and `export` do not persist between calls; use `cwd`.
- Text that looks like a secret is shown as `****`. `edit_file` cannot match it and `write_file`
  refuses to overwrite a file that contains it: edit around the secret lines.
- Files larger than 512 KiB cannot be read or written; use `exec` with `grep`, `sed -n`, `tail`.
