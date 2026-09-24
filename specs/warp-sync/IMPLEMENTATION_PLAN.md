# Warp Sync — Implementation Plan (v2)

> Thay thế `WarpSync_Design_Spec.md` (v1). Người thực thi: một coding agent (Claude Sonnet hoặc
> Gemini 3.1 Pro qua Antigravity CLI). Làm **tuần tự từng Phase**, dừng ở mọi **CHECKPOINT** để
> người dùng xác nhận. Không tự ý mở rộng phạm vi.

---

## 0. Quy tắc bắt buộc cho agent thực thi

1. Đọc `AGENTS.md` trước. Các skill liên quan nằm trong `.agents/skills/`:
   `add-feature-flag`, `gui-ui-guidelines`, `rust-unit-tests`, `logging-and-error-reporting`.
   Đọc `SKILL.md` của skill tương ứng **trước** khi làm task dùng tới nó.
2. **Sau MỖI task** chạy `cargo check -p warp` và sửa hết lỗi trước khi sang task sau.
   (POC trước đó đã commit code chèn vào giữa hàm khác → không compile. Không lặp lại.)
3. Không dùng `unwrap()`/`expect()` trên dữ liệu đến từ remote, filesystem hoặc input của user.
   Không dùng `let _ =` để nuốt lỗi filesystem. Không trả `Success` khi thao tác thất bại.
4. Match exhaustive, không dùng `_` nếu tránh được. `ctx` là tham số cuối. Không prefix `_` cho
   tham số thừa — xoá hẳn. Comment chỉ giải thích "why". Format args inline (`{e}`).
5. Unit test đặt trong file `<name>_tests.rs`, include cuối module bằng
   `#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;`.
6. Lệnh kiểm tra:
   - Test module: `cargo nextest run -p warp warp_sync` (nếu chưa cài nextest:
     `cargo test -p warp --lib warp_sync`).
   - Cuối mỗi Phase: `cargo clippy -p warp --all-targets --tests -- -D warnings`.
   - Chỉ chạy `./script/format` **một lần** ở cuối Phase 4 (hoặc cuối Phase đang làm nếu dừng).
   - **Không** chạy `./script/presubmit`.
7. Commit theo conventional commits (`feat(warp-sync): ...`), mỗi task một commit nhỏ.
8. Gặp tình huống plan không lường trước (API không tồn tại, chữ ký khác) → tìm pattern tương tự
   trong code hiện có; nếu vẫn mơ hồ thì **dừng và hỏi**, không bịa API.
9. File này là **nguồn sự thật duy nhất** cho Warp Sync. Bắt đầu phiên: đọc mục 7 để biết đang ở
   đâu. Sau mỗi task đã commit: tick checkbox + thêm 1 dòng vào "Nhật ký" ở mục 7 (commit cùng task).
   Quyết định lệch khỏi plan → ghi vào "Quyết định" kèm lý do. Không tạo thêm file memory/context khác.

---

## 1. Kết quả review plan v1 và POC hiện có

### 1.1 Giả định cốt lõi của v1 là sai

| Giả định trong v1 | Thực tế trong code | Bằng chứng |
|---|---|---|
| Sau `sudo -i`, daemon remote-server chạy dưới quyền root | Daemon được khởi chạy bằng `ssh -o ControlPath=<sock> … <bin> remote-server-proxy`, tức một exec channel mới trên kết nối SSH master → chạy dưới **user đăng nhập SSH**. `sudo -i` trong PTY không ảnh hưởng tới nó. Daemon còn dùng chung theo identity key. | `app/src/remote_server/ssh_transport.rs:89-94`, `crates/remote_server/src/ssh.rs:39-50` |
| RPC daemon là kênh có quyền root | Sau `sudo -i` (đã Warpify), subshell là **một session riêng** (`IsSSHWrapperSession::No`) dùng `InBandCommandExecutor`: lệnh được gõ vào chính shell root đó, output trả về qua OSC mã hoá hex (an toàn với binary). **Đây mới là kênh có quyền root.** | `app/src/terminal/model/session/command_executor.rs:178-190, 316-335`; `app/assets/bundled/bootstrap/bash_body.sh:182-202` |
| Chuột phải vào path trên terminal đã có sẵn | Link detection **bị tắt cho session remote**. | `app/src/terminal/view/link_detection.rs:581` |
| Giữ nguyên uid/gid ở local | Warp local chạy dưới user thường, không `chown` sang root được. Phải lưu metadata (mode/uid/gid) vào **manifest** rồi áp lại khi upload. | — |
| Thêm message streaming vào proto | Không cần: `Session::execute_command` đã đi đúng kênh cho mọi loại session. | `app/src/terminal/model/session.rs:1592` |

### 1.2 Lỗi trong POC (nhánh `feature/warp-sync-poc`)

- 2 commit `e95736423`, `a0753a177` **không compile**: hàm mới được chèn vào giữa chữ ký
  `read_file_context`, giữa arm `RenameActivePane`, giữa lời gọi `register_editable_bindings`.
  Thay đổi chưa commit đã sửa lại; hiện tree compile được nhưng còn 6 warning unused → clippy fail.
- `handle_sync_download`: `std::fs::read(...).unwrap_or_default()` → đọc file bị `EACCES` (đúng
  trường hợp sudo) vẫn trả **Success với dữ liệu rỗng** → client ghi đè mirror bằng file rỗng.
  Đọc file đồng bộ trên main thread của daemon (chặn mọi request khác); không giới hạn kích thước
  (frame tối đa 64 MiB, `crates/remote_server/src/protocol.rs:13`); bỏ qua cờ `archive`.
- `handle_sync_upload`: ghi lỗi vẫn trả Success; ghi path tuỳ ý không xác nhận.
- Client: path cứng `/tmp/warp_sync_test.txt`; `unwrap()` trên `file_name()`; `let _ = create_dir_all`;
  lấy `host_id` từ `ActiveSession::working_directory` → **luôn `None` trong subshell `sudo -i`**
  (host_id chỉ gắn với session SSH wrapper, xem `app/src/terminal/view.rs:24371-24375`) → POC không
  bao giờ chạy được trong kịch bản mục tiêu.
- `action_tests.rs` khẳng định `SyncRemoteFile.should_save_app_state_on_action()` — sai ngữ nghĩa.
- Binding đặt trong group `Settings`; không có feature flag.
- ~30 dòng đổi `super::proto` → `crate::remote_server::proto` trong `server_model.rs` là churn không cần.
- `tmp_sync_*.rs` là file nháp ở root repo. `GEMINI.md` ghi Phase 1–2 "Completed" — không đúng.
- `GEMINI.md`/`AGENTS.md` nói enum `FeatureFlag` ở `warp_core/src/features.rs`; thực tế định nghĩa ở
  `crates/warp_features/src/lib.rs` (được re-export qua `warp_core::features`).

---

## 2. Kiến trúc đã chọn

### 2.1 Ý tưởng chính

**Transport = `Session::execute_command` của session đang active.** Sync chạy với đúng quyền
của shell mà người dùng đang nhìn thấy:

```
Terminal đang ở…                     Executor của session            Chạy dưới quyền
──────────────────────────────────  ──────────────────────────────  ─────────────────
ssh user@host (session SSH gốc)     RemoteServerCommandExecutor      user đăng nhập
                                    hoặc RemoteCommandExecutor
ssh user@host → sudo -i (Warpified) InBandCommandExecutor            root  ✅ mục tiêu
Local                               (không hỗ trợ — báo lỗi)         —
```

Không thay đổi proto, không thay đổi daemon, không cần `NOPASSWD`.

### 2.2 Luồng dữ liệu

```
Download:
  UI → WarpSyncModel.start_download(session, path)
     → probe script  (tồn tại? quyền? user? kích thước? GNU tar?)
     → download script: tar -czf - -C PARENT ./NAME   (stdout = bytes .tgz)
     → [local] giải nén vào staging, kiểm tra an toàn, ghi manifest, swap vào mirror
Upload:
  UI → WarpSyncModel.prepare_upload(session, path)
     → [local] build .tgz từ mirror, header lấy mode/uid/gid từ manifest
     → probe → CONFIRM DIALOG (user@host, path, số file, dung lượng)
     → begin (mktemp -d) → N× chunk (base64 → append) → commit
       (kiểm tra size + gzip -t → backup tgz vào $HOME/.warp-sync/backups → tar -x → dọn tmp)
```

### 2.3 Ngoài phạm vi v1 (Phase 5 / v2)

Modal nhập path, "Compare with local mirror" (diff UI), trang Settings, phát hiện xung đột phía
remote, symlink, lan truyền thao tác xoá file, streaming qua daemon cho file lớn, PowerShell remote.

---

## 3. Quyết định thiết kế chi tiết

### 3.1 Bố cục mirror

- `mirror_root()` = `dirs::home_dir()/.warp/mirrors`.
- `host_key` = `Session::hostname()` đã sanitize (chỉ giữ `[A-Za-z0-9._-]`, ký tự khác → `_`,
  rỗng → `unknown-host`).
- File remote `/etc/nginx/nginx.conf` ↔ local `<mirror_root>/<host_key>/etc/nginx/nginx.conf`.
- Manifest: `<mirror_root>/.warp-sync/<host_key>.json`.
- Staging: `<mirror_root>/.warp-sync/staging/<uuid>/` (cùng filesystem → `rename` được).

### 3.2 Chuẩn hoá path remote (`normalize_remote_path(input, pwd)`)

- Trim khoảng trắng; từ chối rỗng, chứa `\0` hoặc `\n`, dài > 4096.
- Bắt đầu bằng `~` → từ chối ("Use an absolute path").
- Tương đối → nối với `pwd` của block (pwd phải tuyệt đối, nếu không → lỗi).
- Gộp `//`, bỏ `.`; **có `..` → từ chối** (v1).
- Bỏ `/` cuối. Kết quả `/` → từ chối. Nằm dưới `/proc`, `/sys`, `/dev`, `/run` → từ chối.

### 3.3 Manifest (serde JSON, `version: 1`)

Map phẳng theo **đường dẫn remote tuyệt đối** — đơn giản hơn nhiều so với lưu theo "root":

```json
{
  "version": 1,
  "host_key": "prod-1",
  "entries": {
    "/etc/nginx":            { "kind": "dir",  "mode": 493, "uid": 0, "gid": 0, "uname": "root", "gname": "root", "mtime": 1727000000 },
    "/etc/nginx/nginx.conf": { "kind": "file", "mode": 420, "uid": 0, "gid": 0, "uname": "root", "gname": "root", "mtime": 1727000000, "size": 1234, "sha256": "…" }
  },
  "last_sync": { "/etc/nginx": { "remote_user": "root", "at_unix": 1727000000 } }
}
```

- `mode` lưu đủ `& 0o7777` (kể cả setuid/sticky).
- `replace_subtree(P, new_entries)`: xoá mọi key `== P` hoặc bắt đầu bằng `P + "/"` rồi chèn mới.
  **Chú ý**: `/etc/nginx2` không thuộc `/etc/nginx` — phải so `P + "/"`, không so prefix trần.
- `entries_under(P)`: lấy các key `== P` hoặc bắt đầu bằng `P + "/"`.
- Ghi atomically: ghi file tạm cùng thư mục rồi `rename`.
- Version khác 1 hoặc JSON hỏng → lỗi rõ ràng, không xoá file.

### 3.4 Script remote

Mọi script là **POSIX sh**, được bọc để chạy được bất kể shell của user (bash/zsh/fish):

```
wrap_for_any_shell(script) = "printf %s <BASE64(script)> | base64 -d | sh"
```

Chuỗi base64 chỉ gồm `[A-Za-z0-9+/=]` nên an toàn tuyệt đối với mọi shell và với lớp escape
single-quote của `InBandCommandExecutor`. Mọi path bên trong script phải đi qua
`posix_quote(s)` = `'` + `s.replace('\'', "'\\''")` + `'`. **Không bao giờ** nội suy path chưa quote.

Shell `PowerShell` → trả lỗi `UnsupportedShell` (v1).

Vì `sh` đọc script từ stdin, script **không được** chứa lệnh nào đọc stdin (nó sẽ nuốt phần còn lại
của script). Mọi input phải lấy từ file/đối số.

**Probe** (`probe_script(path)`), output dạng `key=value` từng dòng, luôn `exit 0`:

```sh
P=<quoted path>
if [ ! -e "$P" ]; then echo status=not_found; exit 0; fi
if [ ! -r "$P" ]; then echo status=permission_denied; else echo status=ok; fi
echo "user=$(id -un)"; echo "uid=$(id -u)"
if [ -d "$P" ]; then echo kind=dir; else echo kind=file; fi
echo "size_kib=$(du -sk "$P" 2>/dev/null | cut -f1)"
if tar --version 2>/dev/null | grep -q GNU; then echo tar=gnu; else echo tar=other; fi
if command -v base64 >/dev/null 2>&1; then echo base64=yes; else echo base64=no; fi
```

(Cho upload: nếu target chưa tồn tại thì probe thêm parent; v1 chỉ cho upload path đã có trong manifest.)

**Download** (`download_script(parent, name)`): stdout chỉ chứa bytes tgz; lỗi của tar được gom
vào file tạm và chỉ in ra khi thất bại:

```sh
E=$(mktemp) || exit 90
tar -czf - -C <quoted parent> ./<name đã quote an toàn> 2>"$E"; rc=$?
if [ $rc -ne 0 ]; then cat "$E"; fi; rm -f "$E"; exit $rc
```

Tiền tố `./` ngăn tên file bắt đầu bằng `-` bị hiểu thành option (không cần `--`, tương thích BusyBox).
Quote an toàn cho `./NAME`: `posix_quote(&format!("./{name}"))`.

**Upload begin** (bọc base64 — dùng `$(...)`, fish < 3.4 không hiểu):
`T=$(mktemp -d "${TMPDIR:-/tmp}/warp-sync.XXXXXX") && chmod 700 "$T" && echo "$T"`.
Rust validate output bằng regex `^/[A-Za-z0-9._/-]+/warp-sync\.[A-Za-z0-9]+$` (sau khi trim).

**Upload chunk** (không bọc base64 vì mọi phần đã được validate charset):
`printf %s <b64chunk> | base64 -d >> <T>/payload.tgz`
- Cắt chuỗi base64 tại **bội số của 4** (mỗi chunk tự decode độc lập). Kích thước mặc định
  `UPLOAD_CHUNK_B64_LEN = 16 * 1024`.

**Upload commit** (bọc base64), tham số: `T`, `parent`, `name`, `expected_len`, `backup_name`, cờ tar:

```sh
T=<q>; P=<q parent>; N=<q ./name>
[ "$(wc -c < "$T/payload.tgz" | tr -d ' ')" = "<expected_len>" ] || { echo "size mismatch"; exit 81; }
gzip -t "$T/payload.tgz" || { echo "corrupt payload"; exit 82; }
if [ -e "$P/$N" ]; then
  B="$HOME/.warp-sync/backups"; mkdir -p "$B" && chmod 700 "$B" || exit 83
  tar -czf "$B/<backup_name>.tgz" -C "$P" "$N" || { echo "backup failed"; exit 84; }
fi
tar -xzf "$T/payload.tgz" -C "$P" <EXTRACT_FLAGS> || { echo "extract failed"; exit 85; }
rm -rf "$T"; echo "backup=$B/<backup_name>.tgz"
```

`EXTRACT_FLAGS`: `uid == 0 && tar == gnu` → `-p --same-owner --numeric-owner`;
`tar == gnu` và không phải root → `-p --no-same-owner`; `tar == other` → `-p` (và cảnh báo trong
dialog rằng giữ owner có thể không đầy đủ).
Backup đặt ngoài thư mục đích vì các glob kiểu `/etc/nginx/conf.d/*.conf`, `/etc/cron.d/*` có thể
vô tình nạp file backup nằm cạnh.

**Cleanup** (khi lỗi sau `begin`): `rm -rf <T>` — chỉ chạy khi `T` đã qua regex ở trên.

### 3.5 Giới hạn (constant trong `warp_sync/mod.rs`, sẽ tinh chỉnh sau CHECKPOINT)

| Constant | Giá trị | Lý do |
|---|---|---|
| `MAX_DOWNLOAD_KIB` | `32 * 1024` (theo `du -sk`) | Output in-band đi qua PTY dạng hex (×2) |
| `MAX_EXTRACTED_BYTES` | `256 MiB` | Chống decompression bomb |
| `MAX_ENTRIES` | `20_000` | Chống archive bất thường |
| `MAX_UPLOAD_BYTES` | `4 MiB` (tgz) | Upload phải gõ qua line editor của shell |
| `UPLOAD_CHUNK_B64_LEN` | `16 * 1024` | Dòng lệnh vừa phải cho readline/zle |
| `COMMAND_TIMEOUT` | `120 s` mỗi lệnh | In-band chỉ chạy khi shell rảnh |

### 3.6 Giải nén an toàn phía local (`archive::extract_download`)

- Dùng crate `tar` + `flate2` (đã có trong `crates/node_runtime`, thêm vào workspace deps).
- Mỗi entry: path phải tương đối, chỉ gồm `CurDir`/`Normal`; bỏ `./`; component đầu **phải** bằng
  `name` → nếu không, trả lỗi `UnexpectedArchiveEntry`.
- `EntryType::Regular` / `Directory` → tạo trong staging. Mọi loại khác (symlink, hardlink, device,
  fifo…) → **bỏ qua** và ghi vào `skipped` kèm lý do. Không tạo symlink, không follow symlink.
- Quyền local: file `(mode & 0o777) | 0o600`, dir `(mode & 0o777) | 0o700` (để user sửa được);
  mode gốc giữ trong manifest. Chỉ set quyền dưới `#[cfg(unix)]`.
- Đếm tổng bytes/entries, vượt giới hạn → dừng với lỗi.
- Tính `sha256` từng file (crate `sha2` đã có trong app).
- Lỗi CRC của gzip (flate2 tự kiểm) → lỗi `CorruptArchive`.

### 3.7 Build archive upload (`archive::build_upload`)

- Duyệt `local_path_for(P)` (file hoặc cây thư mục), **không follow symlink** local.
- Path trong archive: `./<name>/…`. Mode/uid/gid/uname/gname lấy từ manifest theo path remote
  tương ứng. File mới (không có trong manifest): uid/gid/uname/gname của **thư mục cha gần nhất
  có trong manifest**, mode `0o644` (dir `0o755`).
- Trả về `UploadArchive { bytes, files, dirs, total_len, new_files: Vec<String>, missing_locally: Vec<String> }`
  (`missing_locally` = có trong manifest nhưng không còn ở local → chỉ thông báo, v1 không xoá remote).
- Nếu `bytes.len() > MAX_UPLOAD_BYTES` → lỗi `TooLarge`.

### 3.8 Bảo vệ thay đổi local khi download lại

Trước khi swap staging vào mirror: so sha256 các file local hiện có dưới `P` với manifest. File
khác sha (đã sửa mà chưa upload) → trả `NeedsConfirmation(OverwriteLocalChanges { files })`, UI
hỏi lại; chỉ ghi đè khi user đồng ý.

### 3.9 Lỗi (`WarpSyncError`, dùng `thiserror`, Display là thông điệp hiển thị cho user)

`NotRemoteSession`, `UnsupportedShell`, `InvalidPath(String)`, `NotFound(String)`,
`PermissionDenied { user: String }`, `TooLarge { limit_desc: String }`, `MissingTool(&'static str)`,
`Timeout`, `Executor(String)`, `RemoteCommandFailed { exit_code: Option<i32>, message: String }`,
`CorruptArchive(String)`, `UnexpectedArchiveEntry(String)`, `NotMirrored(String)`,
`Manifest(String)`, `LocalIo(String)`, `AlreadyInProgress`.

`RemoteCommandFailed.message`: lấy tối đa 1 KiB cuối của output (in-band đặt output vào `stderr`
khi exit ≠ 0; executor khác có thể để ở `stdout`) → `from_utf8_lossy` → dòng không rỗng cuối cùng.
Với `PermissionDenied` thông điệp gợi ý: "Running as <user>. Run `sudo -i` and Warpify the subshell, then retry."

---

## 4. Các Phase

### Phase 0 — Làm sạch nền (≈ 15 phút)

Mục tiêu: bắt đầu từ `master` sạch; giữ POC làm tham khảo, không viết lại lịch sử.

1. Xoá file nháp: `rm tmp_sync_action.rs tmp_sync_binding.rs tmp_sync_handlers.rs tmp_sync_manager.rs`.
2. Lưu thay đổi chưa commit của POC vào stash để tham khảo:
   `git stash push -m "warp-sync POC wip" -- app/ crates/`.
3. Tạo nhánh mới từ master: `git switch -c feature/warp-sync master`.
   (Nhánh `feature/warp-sync-poc` giữ nguyên; **không** merge/cherry-pick commit nào từ đó —
   phần proto/daemon/binding bị bỏ hoàn toàn.)
4. Thêm dòng đầu vào `WarpSync_Design_Spec.md`:
   `> SUPERSEDED — xem specs/warp-sync/IMPLEMENTATION_PLAN.md`.
5. `GEMINI.md` đã được cập nhật (trỏ về file này, đánh dấu POC bị bỏ) — chỉ cần xác nhận.
   (`GEMINI.md` và spec cũ là file untracked của user — không commit trừ khi user yêu cầu.)
6. Commit file plan này trên nhánh mới (`docs(warp-sync): add implementation plan`) để các agent
   sau đọc được cùng một nguồn.
7. Verify: `git status` sạch (ngoài các file untracked của user), `cargo check -p warp` pass.

### Phase 1 — Core thuần (không UI, không async) + unit test

Mọi file dưới `app/src/warp_sync/`. Khai báo `mod warp_sync;` trong `app/src/lib.rs` (cạnh các
`mod` khác). Các hàm ở Phase này **thuần** hoặc chỉ đụng filesystem local → test được hoàn toàn.

**Task 1.1 — Dependencies.**
- Root `Cargo.toml` `[workspace.dependencies]`: thêm `tar = "0.4"`, `flate2 = "1.0"`
  (khớp version đang dùng ở `crates/node_runtime/Cargo.toml`).
- `app/Cargo.toml`: `tar.workspace = true`, `flate2.workspace = true`. Xác nhận `sha2`, `base64`,
  `serde_json`, `uuid`, `tempfile` (dev) đã có.
- Verify: `cargo check -p warp`; `Cargo.lock` không kéo thêm crate mới ngoài `tar`/`flate2` sẵn có.

**Task 1.2 — `warp_sync/mod.rs` + `error.rs`.** Constants mục 3.5; `WarpSyncError` mục 3.9.

**Task 1.3 — `paths.rs` + `paths_tests.rs`.**
API: `mirror_root() -> Option<PathBuf>`, `host_key(hostname: &str) -> String`,
`normalize_remote_path(input: &str, pwd: Option<&str>) -> Result<String, WarpSyncError>`,
`split_parent_name(remote_abs: &str) -> (String, String)`,
`local_path_for(mirror_root: &Path, host_key: &str, remote_abs: &str) -> PathBuf`,
`manifest_path(mirror_root, host_key)`, `staging_dir(mirror_root) -> PathBuf` (uuid mới).
Test tối thiểu: path tuyệt đối/tương đối; `a//b/./c`; `..`; `~`; `/`; `/proc/x`; newline;
tên có space/quote/`$`/backtick; `host_key("my host!")`; `split_parent_name("/etc")` → (`/`, `etc`).

**Task 1.4 — `remote_script.rs` + `remote_script_tests.rs`.**
API: `posix_quote`, `wrap_for_any_shell`, `probe_script`, `download_script`, `upload_begin_command`,
`validate_tmp_dir`, `upload_chunk_commands(tmp_dir, tgz: &[u8]) -> Vec<String>`,
`upload_commit_script(...)`, `cleanup_command(tmp_dir)`, `parse_probe_output(&str) -> Result<ProbeResult, WarpSyncError>`,
`remote_failure_message(output: &[u8]) -> String`.
Kiểu: `ProbeResult { status: ProbeStatus, user, uid: u32, kind: RemoteKind, size_kib: Option<u64>, tar: TarFlavor, has_base64: bool }`.
Test tối thiểu:
- `wrap_for_any_shell` khớp regex `^printf %s [A-Za-z0-9+/=]+ \| base64 -d \| sh$` và decode ra đúng script.
- `posix_quote("it's")` == `'it'\''s'`; script chứa path độc (`$(rm -rf /)`, `` `id` ``, `'`) — assert
  path chỉ xuất hiện dưới dạng đã quote.
- Chunk: nối các chunk decode lại == input; mỗi chunk dài ≤ giới hạn và chia hết cho 4.
- `validate_tmp_dir` chấp nhận `/tmp/warp-sync.AbC123`, từ chối `/tmp/x; rm -rf /`, `/tmp/warp-sync.`, path tương đối.
- `parse_probe_output`: đủ trường; thiếu trường → lỗi; `not_found`; `permission_denied`; dòng rác bị bỏ qua;
  `size_kib=` rỗng (du lỗi quyền) → `size_kib: None`, download vẫn tiếp tục và dựa vào `MAX_EXTRACTED_BYTES`.
- (Nếu có `sh` trên máy test) một test chạy thật `sh -c` với `probe_script` trên thư mục tạm — đánh
  dấu `#[cfg(unix)]`.

**Task 1.5 — `manifest.rs` + `manifest_tests.rs`.**
API: `Manifest::load_or_default(path)`, `save_atomic(&self, path)`, `replace_subtree(&mut self, root, entries)`,
`entries_under(&self, root) -> BTreeMap<String, EntryMeta>`, `nearest_dir_ancestor(&self, path) -> Option<&EntryMeta>`.
Test: prefix `/etc/nginx` vs `/etc/nginx2`; replace xoá entry cũ; round-trip JSON; version lạ → lỗi;
save atomic không để lại file tạm.

**Task 1.6 — `archive.rs` + `archive_tests.rs`.**
API: `extract_download(tgz: &[u8], name: &str, remote_parent: &str, staging: &Path) -> Result<ExtractReport, WarpSyncError>`
(`ExtractReport { entries: BTreeMap<String /*remote abs*/, EntryMeta>, skipped: Vec<(String, SkipReason)>, files, dirs, total_bytes }`),
`build_upload(...) -> Result<UploadArchive, WarpSyncError>` (mục 3.7),
`locally_modified_files(manifest_entries, mirror_root, host_key) -> Vec<String>` (mục 3.8).
Test (tạo archive bằng `tar::Builder` trong test):
- file + dir: mode/uid/gid/uname/gname được ghi đúng vào entries; quyền local có `0o600`/`0o700`.
- entry `../evil`, `/etc/passwd`, top-level khác `name` → lỗi, **không** ghi gì ra ngoài staging.
- symlink/hardlink/fifo → nằm trong `skipped`.
- vượt `MAX_ENTRIES`/`MAX_EXTRACTED_BYTES` (dùng constant nhỏ qua tham số hoặc `cfg(test)`) → lỗi.
- gzip hỏng → `CorruptArchive`.
- `build_upload` rồi `extract_download` lại → metadata khớp manifest; file mới thừa uid/gid của dir cha;
  file bị xoá local nằm trong `missing_locally`.

**Kết thúc Phase 1:** test pass, clippy pass. Commit.

### Phase 2 — Transport + model + feature flag

**Task 2.1 — Feature flag `WarpSync`.** Theo skill `add-feature-flag`: thêm variant vào
`crates/warp_features/src/lib.rs` và bật theo hướng dẫn của skill sao cho build chạy bằng
`./script/run` có flag này (kiểm tra thực tế channel local dùng danh sách flag nào, đừng đoán).

**Task 2.2 — `transfer.rs` (async, không đụng UI).**
- `async fn run(session: &Session, command: &str) -> Result<Vec<u8>, WarpSyncError>`:
  `session.execute_command(command, None, None, ExecuteCommandOptions::default())` bọc timeout
  `COMMAND_TIMEOUT` (xem cách `crates/remote_server/src/ssh.rs` dùng
  `warpui_core::r#async::FutureExt::with_timeout`). Map `CommandExitStatus::Failure` →
  `RemoteCommandFailed` qua `remote_failure_message`.
- `pub async fn download(session: Arc<Session>, remote_path: String, mirror_root: PathBuf, allow_overwrite_local_changes: bool) -> Result<DownloadResult, WarpSyncError>`
  theo 2.2: probe → kiểm tra (`status`, `size_kib`, `base64` không cần cho download) →
  `download_script` → `extract_download` vào staging → kiểm tra 3.8 (trả
  `DownloadResult::NeedsConfirmation` nếu có file sửa local và `!allow_overwrite_local_changes`) →
  xoá subtree cũ trong mirror → `rename` staging → `replace_subtree` + `save_atomic` → dọn staging
  (kể cả khi lỗi). `DownloadResult::Done(DownloadOutcome { local_path, files, dirs, total_bytes, skipped, remote_user })`.
- `pub async fn prepare_upload(...) -> Result<PreparedUpload, WarpSyncError>`: kiểm tra manifest
  (`NotMirrored` nếu trống) → `build_upload` → probe (`has_base64` bắt buộc, `MissingTool("base64")`)
  → trả `PreparedUpload { archive, probe, remote_path, host_key }` (chưa gửi gì).
- `pub async fn execute_upload(session, prepared) -> Result<UploadOutcome, WarpSyncError>`:
  begin → chunks (tuần tự) → commit → cập nhật manifest (sha256/size mới, entry mới) → trả
  `UploadOutcome { files, total_len, backup_path: Option<String>, remote_user }`. Lỗi sau begin →
  chạy `cleanup_command` best-effort rồi trả lỗi gốc.
- IO nặng (giải nén, sha256, build archive) phải chạy **bên trong future** (background), không
  chạy trong callback trên main thread.
- Kiểm tra session: `session.session_type()` phải là `SessionType::WarpifiedRemote { .. }`
  (host_id có thể `None` — **không** được yêu cầu host_id); `session.shell().shell_type()` là
  PowerShell → `UnsupportedShell`.
- Logging: theo skill `logging-and-error-reporting` — path remote dùng `safe_info!` (safe không có
  path, full có path); không log nội dung file.

**Task 2.3 — `model.rs`: `WarpSyncModel` (SingletonEntity).**
- Đăng ký ở `app/src/lib.rs` cạnh `ctx.add_singleton_model(remote_server::manager::RemoteServerManager::new)` (~dòng 1885).
- State: `in_flight: HashSet<(String /*host_key*/, String /*remote path*/)>`,
  `pending_uploads: HashMap<PendingUploadId, (Arc<Session>, PreparedUpload, WindowId)>`,
  tương tự cho download chờ xác nhận.
- API (mỗi hàm nhận `window_id` để chỉ workspace khởi tạo hiển thị toast):
  `start_download(session, remote_path, window_id, ctx)`,
  `confirm_download_overwrite(id, ctx)`, `start_upload(session, remote_path, window_id, ctx)`,
  `confirm_upload(id, ctx)`, `cancel_pending(id, ctx)`.
  Trùng `(host, path)` đang chạy → emit `Failed(AlreadyInProgress)`.
- Event: `WarpSyncEvent::{Started{window_id, description}, DownloadNeedsConfirmation{window_id, id, files}, UploadNeedsConfirmation{window_id, id, summary}, Succeeded{window_id, message, open_path: Option<PathBuf>}, Failed{window_id, error}}`.
- Dùng `ctx.spawn(future, callback)`; callback cập nhật state và emit event.
- Test cho model nếu khả thi theo skill `rust-unit-tests`; tối thiểu test `in_flight` chống trùng.

**Kết thúc Phase 2:** `cargo check`, test, clippy pass. Commit.

### Phase 3 — UI Download (+ dialog xác nhận dùng chung)

Đọc skill `gui-ui-guidelines` trước. Mọi UI ẩn khi `!FeatureFlag::WarpSync.is_enabled()` hoặc
session không phải `WarpifiedRemote`.

**Task 3.1 — Toast trong Workspace.** Trong `Workspace::new` (`app/src/workspace/view.rs`),
subscribe `WarpSyncModel`; chỉ xử lý event có `window_id == ctx.window_id()`. Dùng
`self.toast_stack.update(ctx, |s, ctx| s.add_ephemeral_toast(DismissibleToast::success(..)/error(..), ctx))`
(pattern ở `view.rs:~2255`). Toast thành công có `.with_link(ToastLink …)` "Open folder" mở
`open_path` bằng helper mở thư mục sẵn có (tìm theo `ShowInFileExplorer`).

**Task 3.2 — Dialog xác nhận dùng chung** `app/src/warp_sync/confirm_dialog.rs`, mô phỏng
`app/src/workspace/delete_conversation_confirmation_dialog.rs` (176 dòng). Tham số: tiêu đề,
nội dung (danh sách tối đa 10 path + "and N more"), nhãn nút xác nhận, action confirm/cancel.
Dùng cho `DownloadNeedsConfirmation` (Phase 3) và `UploadNeedsConfirmation` (Phase 4).

**Task 3.3 — Context menu khi có text được chọn trong block của session remote.**
Trong `TerminalView::context_menu_items` (`app/src/terminal/view.rs:17275`), arm
`(RegularTextRightClick | RichContentTextRightClick, None, true)` (~dòng 17383): thêm separator +
"Warp Sync: Download to local mirror" (và ở Phase 4: "Warp Sync: Upload from local mirror").
- Thêm variant vào `ContextMenuAction` (không dùng `_` khi match).
- Handler: lấy text đã chọn (tái dùng logic của `ContextMenuAction::CopySelectedText`); nhiều dòng
  → toast lỗi; lấy block được right-click → `block.session_id()` → `self.sessions.as_ref(ctx).get(id)`
  → `Arc<Session>`; `block.pwd()` để resolve path tương đối; gọi
  `WarpSyncModel::handle(ctx).update(ctx, |m, ctx| m.start_download(..))`.
- Chỉ hiện item khi block thuộc session remote (xem `is_block_considered_remote`).

**Task 3.4 — Command Palette.** Thêm `WorkspaceAction::{WarpSyncDownloadCurrentDirectory, WarpSyncOpenMirror}`
(Phase 4 thêm `WarpSyncUploadCurrentDirectory`). Đăng ký binding trong `app/src/workspace/mod.rs`
bọc `if FeatureFlag::WarpSync.is_enabled() { app.register_editable_bindings([...]) }` (pattern
`UIZoom` ~dòng 362), tên `workspace:warp_sync_download_cwd`, mô tả "Warp Sync: Download current
directory to local mirror", context `id!("Workspace")`. **Không** đặt vào group `Settings`,
**không** thêm vào `should_save_app_state_on_action`.
Handler: lấy terminal view đang active, đọc `terminal.pwd()` và session như pattern ở
`view.rs:17709-17716`; session local → toast `NotRemoteSession`.
`WarpSyncOpenMirror`: mở `<mirror_root>/<host_key>` (tạo nếu chưa có).

**Task 3.5 — Tự review + chạy test/clippy. Commit.**

**⛔ CHECKPOINT A (người dùng test tay — agent dừng lại, bàn giao checklist mục 5).**
Mục tiêu: xác nhận kênh in-band chạy được và đo tốc độ với 1 MB / 5 MB / 20 MB. Nếu 20 MB quá
chậm (> ~60 s) hoặc làm treo terminal → hạ `MAX_DOWNLOAD_KIB`; nếu in-band không hoạt động → dừng,
xem lại kiến trúc với user trước khi làm Phase 4.

### Phase 4 — Upload

**Task 4.1 — Wiring upload** trong model + context menu item "Upload from local mirror" +
palette `WarpSyncUploadCurrentDirectory`.
**Task 4.2 — Dialog xác nhận upload**: hiển thị `user@host:path` (user lấy từ **probe**, không từ
giả định), số file / dung lượng, danh sách `new_files`, `missing_locally` ("sẽ không bị xoá trên
server"), vị trí backup, cảnh báo nếu `tar == other`. Nút mặc định là **Cancel**.
**Task 4.3 — Toast kết quả** kèm đường dẫn backup remote.
**Task 4.4 — `./script/format`** (một lần, cuối cùng), commit.

**⛔ CHECKPOINT B** — người dùng chạy checklist upload ở mục 5.

### Phase 5 — Tuỳ chọn / v2 (chỉ làm khi user yêu cầu)

Modal nhập path từ palette; "Compare with local mirror" (diff UI); trang Settings (mirror root,
giới hạn); phát hiện xung đột remote (fingerprint lúc download vs trước upload); symlink; lan
truyền xoá; kênh streaming qua daemon cho file lớn khi SSH trực tiếp bằng root; nhận diện path
remote khi hover.

---

## 5. Checklist test tay (cho người dùng)

**Môi trường:** 1 VM/container có sshd, **hostname khác máy local**, user thường có sudo **cần mật
khẩu**. (Không dùng `ssh localhost`: subshell `sudo -i` có cùng hostname sẽ bị coi là local và chạy
bằng `LocalCommandExecutor` dưới user của bạn.) Build Warp local với flag `WarpSync` bật.

Download:
1. `ssh user@vm` → `cd /etc` → chọn text `hostname` → chuột phải → Download → file có trong
   `~/.warp/mirrors/<host>/etc/hostname`, manifest có uid 0.
2. `cat /etc/shadow` bị từ chối khi chưa sudo → Download `/etc/shadow` → toast PermissionDenied
   có gợi ý `sudo -i`, **không** tạo file rỗng.
3. `sudo -i` → Warpify subshell → Download `/etc/shadow`, `/root`, `/etc/nginx` (thư mục có symlink)
   → thành công; symlink báo skipped.
4. Path có space/quote: `touch "/tmp/a b'c"` → download OK.
5. Thư mục > giới hạn → toast TooLarge. Đo thời gian 1/5/20 MB.
6. Sửa file trong mirror rồi Download lại → dialog hỏi ghi đè.
7. Đang download thì gõ một lệnh khác và Enter → lỗi rõ ràng, terminal không hỏng.
8. Session local → palette/menu báo NotRemoteSession hoặc không hiển thị.

Upload:
9. Sửa `~/.warp/mirrors/<host>/etc/nginx/nginx.conf` → Upload (trong `sudo -i`) → dialog hiện
   `root@<host>` → xác nhận → file trên server đổi nội dung, **owner/mode giữ nguyên**
   (`stat -c '%U:%G %a'`), có backup trong `/root/.warp-sync/backups/`.
10. Thêm file mới local → upload → file mới trên server thuộc owner của thư mục cha, mode 644.
11. Upload không có `sudo -i` vào file của root → lỗi extract, `/tmp/warp-sync.*` được dọn.
12. Cancel ở dialog → không có lệnh nào chạy trên server.

---

## 6. Rủi ro đã biết

| Rủi ro | Giảm thiểu |
|---|---|
| Throughput in-band thấp / output lớn làm chậm terminal | Giới hạn kích thước; CHECKPOINT A đo thực tế |
| User chạy lệnh khác khi đang truyền → `warp_preexec` kill generator | Báo lỗi rõ; cleanup best-effort |
| Subshell `sudo -i` chưa Warpify → lệnh chạy dưới user SSH | Luôn hiển thị user thật (từ probe) trong toast/dialog |
| BusyBox/non-GNU tar | Flag tối thiểu + cảnh báo trong dialog |
| Hostname trùng giữa các server khác nhau | Ghi chú giới hạn v1; có thể thêm alias ở v2 |
| Ghi đè file hệ thống quan trọng | Confirm bắt buộc, backup tgz ngoài thư mục đích, `gzip -t` + kiểm size trước khi giải nén |

---

## 7. Tiến độ, quyết định, nhật ký

### Tiến độ

- [x] Phase 0 — Làm sạch nền
- [ ] 1.1 Dependencies · [ ] 1.2 mod/error · [ ] 1.3 paths · [ ] 1.4 remote_script · [ ] 1.5 manifest · [ ] 1.6 archive
- [ ] 2.1 Feature flag · [ ] 2.2 transfer · [ ] 2.3 model
- [ ] 3.1 Toast · [ ] 3.2 Confirm dialog · [ ] 3.3 Context menu · [ ] 3.4 Palette · [ ] 3.5 Review
- [ ] ⛔ CHECKPOINT A (user) — kết quả đo: _chưa có_
- [ ] 4.1 Upload wiring · [ ] 4.2 Upload dialog · [ ] 4.3 Toast · [ ] 4.4 Format
- [ ] ⛔ CHECKPOINT B (user)

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| D1 | 2026-09-24 | Transport = `Session::execute_command` của session active | Daemon chạy dưới user SSH, không phải root; subshell `sudo -i` dùng in-band executor chạy trong shell root |
| D2 | 2026-09-24 | Không đổi proto/daemon; bỏ `SyncDownload`/`SyncUpload` của POC | Không cần; POC trả Success giả khi lỗi |
| D3 | 2026-09-24 | Manifest là map phẳng theo path remote tuyệt đối | Tránh logic gộp/tách "root" phức tạp |
| D4 | 2026-09-24 | v1 bỏ qua symlink/special file; không lan truyền xoá | Giữ phạm vi nhỏ, tránh ghi đè nguy hiểm |
| D5 | 2026-09-24 | Backup remote dạng tgz trong `$HOME/.warp-sync/backups` | Backup cạnh file đích có thể bị glob config nạp nhầm |
| D6 | 2026-09-24 | Không dùng `.context/`; file này là nguồn duy nhất | Tránh nhiều nguồn ngữ cảnh lệch nhau giữa các agent |

### Nhật ký

- 2026-09-24 — Plan v2 được viết sau khi review v1 + POC (Claude Opus). Chưa bắt đầu Phase 0.
- 2026-09-24 — Phase 0 xong: xoá 4 file `tmp_sync_*.rs`, stash POC (`stash@{0}` "warp-sync POC wip"), tạo `feature/warp-sync` từ master (f4f9b8838), đánh dấu spec v1 SUPERSEDED, `cargo check -p warp` pass.
