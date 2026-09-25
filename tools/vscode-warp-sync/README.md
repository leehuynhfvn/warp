# Warp Sync for VS Code

Download, compare and upload the files of a [Warp Sync](../../specs/warp-sync/IMPLEMENTATION_PLAN.md)
mirror without leaving VS Code. The commands run through the Warp terminal that is connected to the
server, so they have the privileges of that shell (for example root after `sudo -i`). The extension
only calls `warpctrl`; it never talks to the server itself.

## Set up

1. Run Warp with local control: `./script/run --features warp_sync,warp_control_cli`, and turn on
   **Settings > Scripting**.
2. In Warp, connect to the server, run `sudo -i` if you need root, and let Warp warpify the shell.
3. Build and install the extension:

   ```sh
   cd tools/vscode-warp-sync
   npm install
   npm run package          # writes vscode-warp-sync-<version>.vsix
   code --install-extension vscode-warp-sync-0.1.0.vsix
   ```
4. Set `warpSync.command` to the command that runs `warpctrl`, for example
   `["/path/to/warp/target/debug/warp-oss", "--warpctrl"]` (the default is `["warpctrl"]`).
5. Open the host's folder of the mirror (`~/.warp/mirrors/<host>`) in VS Code. If you moved the
   mirror in Warp's settings, also set `warpSync.mirrorRoot`.

## Use

- **Explorer / editor tab, right click:** *Download from Server*, *Upload to Server*,
  *Compare with Server*.
- **Source Control title bar:** *Upload Changed Files* and *Compare Changed Files* offer the files
  that differ from the last sync (the mirror is a Git repository whose HEAD is the server's state).
- **Upload** always asks first. The dialog names `user@host`, the number of files, files that would
  be created, files missing from the mirror (never deleted on the server), and warns when the
  server changed since the last sync. Cancelling sends nothing.

## Security notes

- The confirmation is a safeguard against mistakes, not a security boundary. Warp's local control
  is available to every process that runs as your OS user while **Settings > Scripting** is on, and
  such a process can prepare and confirm an upload itself, with the privileges of the Warp session
  (root after `sudo -i`). Turn Scripting off when you do not use it.
- A mirror contains files chosen by the server, for example `.vscode/tasks.json`. Keep mirror
  folders in VS Code's **Restricted Mode**: this extension works there, and it needs no trust.
  Never trust a mirror folder you did not review.
- `warpSync.command` and `warpSync.mirrorRoot` are machine-scoped, so a workspace cannot change
  which program the extension starts.
- **Status bar:** `root@<host>` when a Warp session serves the mirror, a warning otherwise. Click to
  check again.

The extension stays dormant, and starts no process, unless a workspace folder is inside the mirror
folder.

## Develop

```sh
npm test    # compiles, then runs the unit tests with node --test
```

The pure logic (building the command line, reading Warp's answers, the dialog texts, the diff
plan) is in modules that do not import `vscode`; `extension.ts` only connects them to the editor.
