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
   - Test module: `cargo nextest run -p warp --lib warp_sync` (đã cài trên máy này). Với `cargo test`
     thường, một số test terminal/workspace lỗi khi chạy chung một process (phụ thuộc thứ tự, không liên quan
     Warp Sync) — dùng nextest để đối chiếu.
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

### Phase 6 — Sửa mirror bằng VS Code (user yêu cầu 2026-09-25)

Mục tiêu: sửa file của server trong VS Code qua mirror, xem diff native trong VS Code, rồi vẫn
upload bằng một thao tác trong Warp. Không đổi transport, script remote hay định dạng manifest.

**Chọn editor.** Dùng lại setting có sẵn *Settings → Code → Editor and Code Review → "Choose an editor to open file links"*
(`EditorSettings::open_file_editor`). Chỉ các editor họ VS Code có CLI hỗ trợ `--diff`:
`VSCode` → `code`, `VSCodeInsiders` → `code-insiders`, `Cursor` → `cursor`, `Windsurf` → `windsurf`.
Setting khác → hành vi cũ (Open folder bằng file manager, diff mở trong editor của Warp); palette
"Open in editor" báo lỗi hướng dẫn chọn editor. Không thêm setting mới. CLI chạy bằng
`command::blocking::Command` trên background (không qua shell; path luôn tuyệt đối nên không bị hiểu
nhầm là option); lỗi → toast (`WarpSyncError::Editor`), `NotFound` → gợi ý cài lệnh `code`.
Workspace VS Code luôn là **thư mục host** `<mirror_root>/<host_key>` (để Source Control thấy repo ở 6.2).

**Task 6.1 — Mở mirror bằng VS Code.** Module `warp_sync/editor.rs`: `EditorCli::for_editor`
(thuần, có test), `EditorRequest { OpenMirror { workspace, file }, OpenDiffs { workspace, diffs, files } }`,
`invocations(&EditorRequest) -> Vec<Vec<OsString>>` (thuần, có test), `launch`. Mở thư mục:
`code <host_dir>`; mở file: `code <host_dir> <file>`; diff: `code -r --diff <server> <mirror>`.
UI: toast sau khi download có link "Open in <editor>" (thay "Open folder" khi có editor);
palette `workspace:warp_sync_open_mirror_in_editor`.

**Task 6.2 — Git baseline trong mirror.** Module `warp_sync/baseline.rs`. Thư mục host là một git repo
(`<host_dir>/.git`, tạo khi cần, commit rỗng ban đầu; `info/exclude` có `/.vscode/`). HEAD = "trạng thái
server ở lần sync cuối", nên Source Control của VS Code hiện đúng những gì đã sửa local chưa upload.
- Sau download `P`: `git add -A -f -- P` rồi `git commit -- P` (nếu có thay đổi) — gồm cả file bị xoá trên server.
- Sau upload `P`: chỉ commit **đúng các file trong archive** (`--pathspec-from-file` NUL). File đã xoá
  local vẫn còn trên server (không lan truyền xoá), nên phải còn trong HEAD và hiện "deleted" trong VS Code.
- Commit với pathspec dùng ngữ nghĩa `--only`: không cuốn theo thứ user đã stage trong VS Code.
- An toàn: mọi lệnh chạy `git --git-dir … --work-tree … --literal-pathspecs` với
  `-c core.hooksPath=/dev/null -c core.fsmonitor=false -c commit.gpgsign=false -c core.autocrlf=false`,
  user/email cố định, bỏ mọi biến môi trường `GIT_*`, `GIT_TERMINAL_PROMPT=0`; khoá `BASELINE_LOCK`.
- Thư mục `.git` lồng từ server (vd etckeeper `/etc/.git`) **không được mirror**: giải nén bỏ qua mọi
  entry có thành phần `.git` (`SkipReason::GitMetadata`, chỉ báo một lần cho thư mục gốc `.git`), duyệt
  mirror local cũng bỏ qua `.git`, `normalize_remote_path` từ chối path có thành phần `.git`.
  Lý do: repo lồng khiến git coi cả thư mục là gitlink, và `.git/config` do server quyết định (vd
  `core.fsmonitor`) có thể chạy lệnh trên máy local khi git/VS Code quét repo.
- Không có `git` → bỏ qua lặng lẽ (log info). Lỗi git khác **không** làm hỏng sync: toast thành công
  kèm câu "could not record the Git baseline: …".
- Mirror có sẵn trước Phase 6 chưa có baseline: file hiện "untracked" tới lần download kế tiếp.

**Task 6.3 — Compare mở bằng `code --diff`.** `transfer::compare` giữ bản server mới tải ở
`<mirror_root>/.warp-sync/compare/<host_key>/<path>` (thay bản cũ; file đặt 0400) thay vì xoá. Dialog
kết quả: nút "Open in <editor>" → mở workspace host, `--diff` cho tối đa `MAX_EDITOR_DIFFS` (10) file có
cả hai phía, và mở thêm file report `.diff` khi còn khác biệt không hiện được bằng `--diff` (chỉ một
phía, hoặc vượt giới hạn). Không có editor họ VS Code → giữ nút "Open diff" (editor của Warp). Không còn
khác biệt → xoá report và bản compare cũ của path đó (sửa nit "diff cũ còn lại").

**Task 6.4 — (CHỈ GHI CHÚ, chưa làm) Live sync: tự upload khi Save.** Watcher (`crates/watcher`,
debounce 1–2 s, bỏ `.swp`/`4913`/`*~`) trên path được bật riêng; upload qua đúng session + remote user đã
bật (kiểm `id -u` trước mỗi lần, session mất → dừng, không fallback). Ràng buộc cứng: in-band command bị
huỷ khi shell đang chạy lệnh của user (`pty_controller.rs:265-271`) → phải xếp hàng chờ shell rảnh. Giữ
kiểm tra xung đột + backup, chỉ bỏ dialog; mặc định tắt, có chỉ báo "Live sync" rõ ràng; chỉ upload file
đã đổi (hiện upload đóng gói cả path).

### Phase 7 — Điều khiển Warp Sync từ VS Code (user yêu cầu 2026-09-25)

Mục tiêu: Download / Upload / Compare ngay trong VS Code (chuột phải, nút trên Source Control), xác
nhận upload bằng modal của VS Code, **không** cần chuyển sang Warp. Lệnh vẫn chạy qua session Warpified
đang mở trong Warp (kênh duy nhất có quyền root sau `sudo -i`, xem D1).

**Điều kiện bắt đầu (⛔ CHECKPOINT C, user):** user đã test tay mục 13–19 của checklist (Phase 5–6).
Chưa test → agent **dừng và hỏi**, không bắt đầu Phase 7.

**Kênh = local control có sẵn (`warpctrl`)**, không dùng URI scheme (không xác thực, trang web nào cũng
gọi được). Đã kiểm chứng trong code:
- `warpctrl` là chính binary Warp chạy với `--warpctrl` (`app/src/lib.rs::run`,
  `warp_cli::local_control::ControlArgs::from_control_mode_env`). Build OSS cần Cargo feature
  `warp_control_cli` (`app/Cargo.toml`, `app/src/features.rs:466`) → chạy
  `./script/run --features warp_sync,warp_control_cli`; user phải bật **Settings → Scripting**.
- Bảo mật: discovery record 0600 → Unix socket broker (kiểm UID) → bearer ngắn hạn, gắn instance, chỉ
  cho **một** action → HTTP loopback từ chối `Origin` trình duyệt (`app/src/local_control/mod.rs`).
- Action khai báo trong `define_action_catalog!` (`crates/local_control/src/catalog.rs`), params là struct
  `#[serde(deny_unknown_fields)]` trong `crates/local_control/src/protocol.rs` + `ActionParameterSpec`
  + `resolver.rs::parse_params`; dispatch bằng `match` exhaustive trong
  `app/src/local_control/bridge.rs::handle_request` (hiện trả `ResponseEnvelope` **đồng bộ**).
  CLI: `crates/warp_cli/src/local_control/{mod.rs,commands.rs}` (mẫu: `FileCommand::Open` →
  `run_action_with_params`). Client: `crates/local_control/src/client.rs::send_request` dùng
  `reqwest::blocking::Client::new()` → **timeout 30 s**.
- Chọn session: `handlers/metadata.rs::session_list` / `select_pane_entries` / `select_session_entries`
  (có `target.session`).

**Task 7.1 — Bridge bất đồng bộ + client có timeout (hạ tầng dùng chung với Agent Bridge).** Làm đúng
thiết kế mục 3.5 và Task 2.4 của `../warp-agent-bridge/specs/agent-bridge/IMPLEMENTATION_PLAN.md`:
`BridgeResult { Ready(ResponseEnvelope), Pending { request_id, receiver: oneshot::Receiver<Result<Value, ControlError>> } }`;
`handle_request` trả `BridgeResult`, mọi arm cũ bọc `Ready`; `mod.rs::handle_control_request` await
`Pending` (`Err(Canceled)` → `BridgeUnavailable`); test cũ dùng helper `#[cfg(test)] expect_ready()`.
Client: `send_request_with_timeout(instance, request, timeout)`, `send_request` gọi lại nó với hành vi cũ.
Không thêm action nào ở task này. Verify: `cargo nextest run -p warp --lib -E 'test(/local_control::/)'`,
`cargo nextest run -p local_control`, clippy. Commit riêng (để Agent Bridge rebase dùng lại).

**Task 7.2 — Model: người yêu cầu là cửa sổ Warp hoặc client ngoài.** Trong `WarpSyncModel`, thay
`window_id: WindowId` ở các `start_*` bằng `Requester { Window(WindowId), External(ExternalReply) }`
(`ExternalReply` bọc `oneshot::Sender<Result<SyncReply, WarpSyncError>>`). Một helper `report(requester, …)`:
`Window` → `ctx.emit(WarpSyncEvent::…)` như cũ; `External` → gửi `SyncReply` có kiểu (không dựng dialog).
- Pending của client ngoài nằm ở map **riêng**, id là `Uuid` ngẫu nhiên (không phải `PendingId` tuần tự):
  client ngoài không xác nhận được dialog đang mở trong Warp và ngược lại.
- Pending ngoài hết hạn sau `EXTERNAL_PENDING_TTL` (10 phút) → tự huỷ, nhả khoá `(host, path)`.
- Vẫn hiện toast "Warp Sync (VS Code): Uploading …" ở cửa sổ chứa session được dùng — lệnh được gõ vào
  shell đó, user phải thấy.
- `SyncReply`: `Downloaded { local_path, files, dirs, bytes, remote_user, skipped, baseline_warning }`,
  `NeedsConfirmation { pending_id, kind: OverwriteLocalChanges { files } | Upload(UploadSummary) }`,
  `Uploaded { …, backup_path }`, `Compared { differences, identical_files, diff_path, host_dir, server_copy_dir, remote_user }`,
  `Unchanged { identical_files }`. Mọi chuỗi đến từ server đi qua `printable`.
- Test: model_tests cho định tuyến `Window`/`External`, TTL, id riêng.

**Task 7.3 — Action `sync.*` + handler `app/src/local_control/handlers/sync.rs`.** Catalog (group mới,
`TargetScope::File`), tất cả trả `Pending` (7.1), chỉ khi `FeatureFlag::WarpSync` bật (tắt → `UnsupportedAction`):

| Action | Params | Kết quả |
|---|---|---|
| `sync.status` | `{ path? }` | `mirror_root`, và nếu có `path`: `host_key`, `remote_path`, các session khớp (`session_id`, hostname, tab) |
| `sync.download` | `{ path }` | `Downloaded` hoặc `NeedsConfirmation(OverwriteLocalChanges)` |
| `sync.upload.prepare` | `{ path }` | `NeedsConfirmation(Upload)` (luôn — upload luôn phải xác nhận) |
| `sync.confirm` | `{ pending_id }` | `Downloaded` / `Uploaded` |
| `sync.cancel` | `{ pending_id }` | ack |
| `sync.compare` | `{ path }` | `Compared` hoặc `Unchanged` |

- `path` = **đường dẫn local tuyệt đối** trong mirror. Handler: `canonicalize` (từ chối symlink thoát
  ra ngoài), phải nằm dưới `SyncConfig.mirror_root`, không nằm trong `.warp-sync/`; thành phần đầu =
  thư mục host (`host_dir_name`), phần còn lại → remote path qua `normalize_remote_path` (nên `.git` bị
  từ chối). Path chưa tồn tại local (download lần đầu một path mới) → chấp nhận nếu cha nằm trong mirror.
- Chọn session: `target.session` nếu có; không thì các session remote có `host_key(hostname) == host_dir_name`
  hoặc `host_dir_name` bắt đầu bằng `host_key(hostname) + "-"` (mirror có hậu tố machine-id, D11). Đúng 1 →
  dùng; nhiều → ưu tiên session active của cửa sổ đang focus nếu nằm trong số đó, không thì lỗi
  `AmbiguousSession` kèm danh sách; 0 → lỗi "Mở session tới <host> trong Warp".
- An toàn máy: thêm `expected_host_key: Option<String>` vào `DownloadRequest`/`UploadRequest`/`CompareRequest`;
  sau `resolve_host_key` nếu khác → `WarpSyncError::Manifest("the mirror belongs to another machine")`.
  Không bao giờ upload file mirror của máy A qua session của máy B.
- `ErrorCode` mới cho lỗi Warp Sync (hoặc map vào code sẵn có) — message = `WarpSyncError` Display.
- Test: map path → (host, remote) (trong/ngoài mirror, symlink thoát, `.warp-sync`, `.git`), chọn session
  (0/1/nhiều/hậu tố machine-id), `expected_host_key` sai → lỗi.

**Task 7.4 — CLI `warpctrl sync`.** `status [PATH]`, `download PATH`, `upload PATH` (in tóm tắt + `pending_id`,
exit code 3 = cần xác nhận), `confirm ID`, `cancel ID`, `compare PATH`; `--output-format json` in nguyên
`data`. Dùng `send_request_with_timeout` với `SYNC_CLIENT_TIMEOUT` = 10 phút (mỗi lệnh remote tối đa
`COMMAND_TIMEOUT` 120 s, một thao tác chạy vài lệnh). Test parse trong `crates/warp_cli/src/local_control_tests.rs`.

**⛔ CHECKPOINT D (user):** qua CLI trong terminal local: `status` → `download` → sửa file → `upload` →
`confirm` → `compare`, với session `sudo -i` trên host thật; thử 2 tab cùng host (lỗi AmbiguousSession),
đóng tab giữa chừng, shell đang chạy `top` (lỗi rõ ràng).

**Task 7.5 — Extension VS Code `tools/vscode-warp-sync/`.** TypeScript, **không** dependency runtime (chỉ
`@types/vscode`, `typescript` dev), build `npm run compile`, đóng gói `npx @vscode/vsce package` → `.vsix`
cài tay. Gọi `warpctrl` bằng `child_process.execFile` (không shell), luôn `--output-format json`.
- Setting `warpSync.command`: mảng argv, mặc định `["warpctrl"]`; dev: `["<repo>/target/debug/warp-oss", "--warpctrl"]`.
- Kích hoạt khi workspace nằm dưới mirror root (hỏi `sync.status`).
- Lệnh: Download / Upload / Compare with server — ở `explorer/context`, `editor/title/context`,
  `scm/title` (Upload + Compare cho cả thư mục đang mở), Command Palette.
- Upload: `sync.upload.prepare` → `showWarningMessage(modal)` với `user@host`, số file, **cảnh báo xung
  đột** (changed/missing/already_exist), backup → Upload = `sync.confirm`, huỷ = `sync.cancel`.
  Download có thay đổi local → modal "Overwrite local changes?" tương tự.
- Compare: mở `vscode.diff(serverCopyUri, mirrorUri, "server ↔ mirror: <path>")` cho từng file hai phía
  (tối đa `MAX_EDITOR_DIFFS`), file một phía → mở report `.diff`.
- Status bar: `$(cloud) root@draff3` hoặc `$(warning) no Warp session`; bấm → `sync.status`.
- Tiến trình: `withProgress` (notification) trong khi chờ; lỗi → `showErrorMessage` với message từ Warp.
- Test: logic thuần (dựng argv, parse JSON, dựng nội dung modal) bằng test runner của Node (`node --test`),
  không cần chạy VS Code.

**Task 7.6 — Review + format.** `code-reviewer` + `security-reviewer` (trọng tâm: map path, chọn session,
tách pending ngoài/trong, extension không truyền chuỗi qua shell). Clippy, `./script/format`, commit.

**⛔ CHECKPOINT E (user):** checklist mục 20–25.

### Phase 7b — Upload path mới (user yêu cầu 2026-09-25)

Vấn đề (đã kiểm chứng trên `draff3`): file/thư mục **chưa có trên server** không upload thẳng được. `prepare_upload` probe path trước nên báo `Not found on the remote host`; chỉ upload được thư mục cha đã sync (khi đó file mới nằm trong "New files"), nhưng cha đóng gói cả cây → ghi đè lại mọi file khác dưới cha, không mong muốn. Cách vòng hiện tại: `mkdir` trên server, `sync download` thư mục rỗng, rồi upload riêng nó (đã thử, chạy được). Quyền hiện tại của mục mới: uid/gid/uname/gname của thư mục cha gần nhất trong manifest, mode cố định file `0644` / dir `0755` (`archive.rs`: `NEW_FILE_MODE`, `NEW_DIR_MODE`, `nearest_dir_ancestor`); bỏ qua umask của server và mode của file local.

**Task 7.7 — Upload thẳng path mới.** Điều kiện: một tổ tiên của path đã có mục `dir` trong manifest và **có thật trên server** (gọi là `A`, tổ tiên sâu nhất như vậy). Không có `A` → giữ lỗi hiện tại (`NotMirrored`/`NotFound`).
- `prepare_upload`: nếu probe path trả `not_found` và manifest không có mục nào dưới path → chế độ "path mới": probe `A` (phải tồn tại, là thư mục, đọc được), bỏ bước checksum (không có gì để so; `RemoteCheck::Checked` rỗng), `build_upload` lấy owner từ `A`. Lưu `probe_path` (= `A` trong chế độ mới) vào `PreparedUpload` để `ensure_same_target` (7.6) probe đúng chỗ.
- Archive: `-C A` với đường dẫn tương đối nhiều thành phần `./a/b/c`, có mục thư mục tường minh cho từng thư mục trung gian (owner của `A`, mode `0755`) — tương tự cách `tar` đã thay symlink thư mục bằng thư mục thật (rủi ro mục 6). Sinh script commit tổng quát hoá `parent`/`name` (`split_parent_name` hiện chỉ tách một cấp).
- Commit script: trong chế độ mới thêm bước **từ chối nếu path đã xuất hiện** (`[ ! -e "$P/$N" ] && [ ! -L "$P/$N" ]`, exit mã riêng) để không ghi đè thứ ai đó vừa tạo giữa lúc chuẩn bị và xác nhận; không backup (không có gì bị thay).
- Manifest sau upload: ghi mục cho path mới **và các thư mục trung gian**; `replace_subtree`/`record_upload` xử lý được path chưa có gốc.
- `UploadSummary`/dialog/CLI/extension: thêm dòng "Creates folder(s): …" (danh sách thư mục sẽ được tạo); không cần đổi extension ngoài README (nút SCM "Upload Changed Files" tự chạy được với file mới).
- **Quyết định cần user xác nhận trước khi code:** mode của mục mới — (a) giữ cố định `0644`/`0755` (an toàn, dễ đoán; `chmod 600` local bị bỏ qua) hay (b) lấy bit `rwx` của file local `& 0o777` (tôn trọng `chmod 600`, vẫn từ chối setuid/setgid/sticky như `SpecialMode`)? Khuyến nghị (b) cho mục mới, hiển thị mode trong dialog ("New files (mode)"); mục đã có trên server giữ mode của manifest như cũ.
- Bảo mật cần review riêng: path tương đối nhiều cấp không chứa `..`/`.`/tên rỗng, `A` không bị thay bằng symlink giữa probe và extract (script kiểm), guard `expected_host_key` và `resolve_host_key` (7.6, D20) vẫn chạy trước mọi thay đổi, giới hạn `MAX_ENTRIES`, dialog phải nói rõ sẽ **tạo** thư mục nào.
- Test: `transfer_tests` (LocalSh): file mới trong thư mục đã sync; cây thư mục mới nhiều cấp; path xuất hiện giữa prepare và confirm → từ chối, không ghi gì; `A` bị xoá trên server → `NotFound`; `A` không phải thư mục; không có tổ tiên trong manifest → lỗi; mode (a)/(b) và từ chối setuid; owner theo `A`; mirror có hậu tố machine-id (khoá `in_flight`, xem D20); `ensure_same_target` với path mới. Rồi chạy tay trên server test: `mkdir`-less upload của một thư mục mới, `stat` owner/mode trên server.

**⛔ CHECKPOINT F (user):** upload thư mục mới `~/.warp/mirrors/<host>/root/test-dir-2` (không `mkdir` trước) từ CLI và từ VS Code; kiểm `stat -c '%U:%G %a'` trên server; thử tạo file trùng tên trên server giữa `upload` và `confirm` → phải bị từ chối.

---

## 5. Checklist test tay (cho người dùng)

**Chạy bản có tính năng:** trên máy không có `warp-channel-config` (kênh OSS), `./script/run` **không** bật flag —
dùng `./script/run --features warp_sync`. Cách dùng: chuột phải vào text đang chọn (một dòng, trong một block của
session SSH) → "Warp Sync: Download to local mirror"; hoặc Command Palette → "Warp Sync: Download current directory
to local mirror" / "Warp Sync: Open local mirror". Mirror nằm ở `~/.warp/mirrors/<host>/…`.

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

Phase 6 (VS Code) — trước tiên đặt *Settings → Code → Editor and Code Review → "Choose an editor to open file links"* = VS Code (Linux: Warp nhận `code.desktop` hoặc `com.microsoft.VSCode.desktop`):

13. Download `/etc/nginx` → toast có link "Open in VS Code" → VS Code mở workspace `~/.warp/mirrors/<host>`
    (cả host, không chỉ thư mục vừa tải); Download một file → mở workspace + đúng file đó.
14. Source Control của VS Code: ngay sau download thì sạch; sửa `nginx.conf` → hiện "M", bấm vào thấy diff
    với bản server lần sync cuối. `git -C ~/.warp/mirrors/<host> log` có commit "Download /etc/nginx as root".
15. Upload từ Warp → Source Control sạch lại; xoá một file local rồi upload → file đó vẫn hiện "D" (server
    vẫn còn file, upload không xoá).
16. Sửa file trên server (`echo x >> /etc/nginx/nginx.conf`) → Compare → dialog có nút "Open in VS Code" →
    VS Code mở tab diff: trái = bản server (read-only), phải = mirror. Thêm/xoá file một phía → report `.diff`
    cũng được mở. Compare lại khi đã hết khác biệt → toast "No differences", file report cũ bị xoá.
17. Palette "Warp Sync: Open local mirror in external editor" mở workspace host. Đổi setting về "Default App"
    → toast lại là "Open folder", Compare lại là "Open diff" (editor của Warp); palette báo lỗi hướng dẫn.
18. Server có etckeeper (`/etc/.git`) → Download `/etc` → toast báo skipped, mirror **không** có `etc/.git`;
    palette nhập `/etc/.git` → lỗi "Git metadata (`.git`) is not synced".
19. Máy không có `git` (hoặc tạm đổi PATH) → download/upload vẫn thành công, không có baseline, không báo lỗi.

Phase 7 (VS Code điều khiển Warp) — chạy `./script/run --features warp_sync,warp_control_cli`, bật
Settings → Scripting, cài `.vsix` từ `tools/vscode-warp-sync/`, đặt `warpSync.command`:

20. Mở `~/.warp/mirrors/<host>` trong VS Code khi Warp có tab `sudo -i` tới host → status bar `root@<host>`.
    Đóng tab → status bar báo không có session.
21. Chuột phải file trong mirror → Upload → modal VS Code hiện `root@<host>`, số file, backup → Upload →
    file trên server đổi, Warp hiện toast ở đúng cửa sổ chứa session. Cancel → không lệnh nào chạy.
22. Sửa file trên server rồi Upload từ VS Code → modal có cảnh báo "changed on the server".
23. Compare từ nút trên Source Control → tab diff server ↔ mirror mở trong VS Code.
24. Hai tab Warp cùng host → lỗi nêu rõ phải chọn session; mirror của host khác tên (hậu tố machine-id)
    không bao giờ được upload qua session của máy kia.
25. Tắt Settings → Scripting → mọi lệnh trong VS Code báo lỗi local control bị tắt, không có gì chạy.

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
| Hostname do host từ xa tự báo → hai host trùng tên dùng chung mirror/manifest | `host_key` thêm hash khi tên bị sanitize (không thể "nhái" thư mục của host khác bằng ký tự lạ); hai host trùng hostname được tách bằng machine-id (xem D11). Upload từ chối file setuid/setgid để manifest độc hại không tạo được chương trình setuid root |
| Symlink thư mục bị cài giữa lúc download và upload (upload chạy bằng root) | Archive luôn có mục thư mục tường minh nên GNU tar thay symlink bằng thư mục thật (đã thử: `tar 1.35` không ghi xuyên). Còn lại cửa sổ TOCTOU rất hẹp giữa hai mục — chấp nhận ở v1 |
| Bất kỳ tiến trình nào cùng OS user (khi Settings > Scripting bật) có thể `sync.upload.prepare` rồi `sync.confirm` — không cần người bấm — và ghi file lên server với quyền của session (root sau `sudo -i`); nút xác nhận ở VS Code/CLI chỉ chống thao tác nhầm, **không** phải ranh giới bảo mật | Ghi trong README extension; id xác nhận là UUID v4 dùng một lần, hết hạn sau 10 phút, tối đa 8 cái chờ; tắt Scripting khi không dùng. **Quyết định còn mở (user):** thêm setting riêng, mặc định tắt, cho phép Warp Sync qua local control, hoặc buộc dialog phê duyệt trong Warp cho upload từ client ngoài |
| Thư mục mirror chứa file do server chọn (`.vscode/tasks.json`, `.code-workspace`…) → mở & tin cậy workspace trong VS Code có thể chạy code của server | Extension khai báo hoạt động trong Restricted Mode; README dặn giữ Restricted Mode. Chưa loại `.vscode`/`*.code-workspace` khỏi mirror (như D14 làm với `.git`) |
| Output/stdout bị host độc hại làm phình bộ nhớ | Kiểm `du` trước, kiểm kích thước byte nhận được sau, và chặn giải nén theo tổng stream (kể cả entry bị bỏ qua) |

---

## 7. Tiến độ, quyết định, nhật ký

### Tiến độ

- [x] Phase 0 — Làm sạch nền
- [x] 1.1 Dependencies · [x] 1.2 mod/error · [x] 1.3 paths · [x] 1.4 remote_script · [x] 1.5 manifest · [x] 1.6 archive
- [x] 2.1 Feature flag · [x] 2.2 transfer · [x] 2.3 model
- [x] 3.1 Toast · [x] 3.2 Confirm dialog · [x] 3.3 Context menu · [x] 3.4 Palette · [x] 3.5 Review
- [x] ⛔ CHECKPOINT A (user) — kết quả đo: _chưa ghi thời gian 1/5/20 MB; download chạy đúng trên host thật_
- [x] 4.1 Upload wiring · [x] 4.2 Upload dialog · [x] 4.3 Toast · [x] 4.4 Format
- [x] ⛔ CHECKPOINT B (user) — user xác nhận đã test upload (2026-09-25)
- [x] 5.1 Phát hiện xung đột remote · [x] 5.2 Modal nhập path · [x] 5.3 Trang Settings · [x] 5.4 Compare with remote (diff)
  (Phạm vi Phase 5 do user chọn: 4 mục trên; xong 2026-09-25, chờ user test tay. Các mục còn lại — symlink, lan truyền xoá, streaming qua daemon, hover — không làm.)
- [x] 6.1 Mở mirror bằng VS Code · [x] 6.2 Git baseline trong mirror · [x] 6.3 Compare bằng `code --diff` · 6.4 Live sync — chỉ ghi chú, chưa làm
  (Xong 2026-09-25, chờ user test tay mục 13–19 ở mục 5.)
- [x] ⛔ CHECKPOINT C (user) — user xác nhận đã test tay mục 13–19, không có lỗi (2026-09-25)
- [x] 7.1 Bridge async + client timeout · [x] 7.2 Model `Requester` · [x] 7.3 Action `sync.*` · [x] 7.4 CLI `warpctrl sync`
- [x] ⛔ CHECKPOINT D (user) — CLI trên host thật (user giao quyền; agent tự chạy phần không cần giao diện, xem Nhật ký; các tình huống 2 tab / đóng tab / `top` chưa kiểm)
- [x] 7.5 Extension VS Code · [x] 7.6 Review + format
- [ ] ⛔ CHECKPOINT E (user) — checklist mục 20–25
- [ ] 7.7 Upload path mới (Phase 7b) · [ ] ⛔ CHECKPOINT F (user)

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| D1 | 2026-09-24 | Transport = `Session::execute_command` của session active | Daemon chạy dưới user SSH, không phải root; subshell `sudo -i` dùng in-band executor chạy trong shell root |
| D2 | 2026-09-24 | Không đổi proto/daemon; bỏ `SyncDownload`/`SyncUpload` của POC | Không cần; POC trả Success giả khi lỗi |
| D3 | 2026-09-24 | Manifest là map phẳng theo path remote tuyệt đối | Tránh logic gộp/tách "root" phức tạp |
| D4 | 2026-09-24 | v1 bỏ qua symlink/special file; không lan truyền xoá | Giữ phạm vi nhỏ, tránh ghi đè nguy hiểm |
| D5 | 2026-09-24 | Backup remote dạng tgz trong `$HOME/.warp-sync/backups` | Backup cạnh file đích có thể bị glob config nạp nhầm |
| D6 | 2026-09-24 | Không dùng `.context/`; file này là nguồn duy nhất | Tránh nhiều nguồn ngữ cảnh lệch nhau giữa các agent |
| D7 | 2026-09-24 | Flag bật bằng Cargo feature `warp_sync` (+ `DOGFOOD_FLAGS`); `Workspace` chỉ subscribe `WarpSyncModel` khi flag bật | Kênh OSS không có flag nào ngoài DEBUG_FLAGS; subscription vô điều kiện làm vỡ ~90 test Workspace vì model chưa đăng ký trong test harness |
| D8 | 2026-09-24 | Khoá `(host, path)` giữ suốt lúc chờ xác nhận; hai path lồng nhau (tổ tiên/hậu duệ) cũng xung đột | Tránh download đè mirror khi dialog upload đang mở; tránh race swap/manifest |
| D9 | 2026-09-24 | Manifest ghi dưới `MANIFEST_LOCK` (đọc-sửa-ghi); `save` lỗi thì hoàn tác swap | Hai sync path không lồng nhau trên cùng host từng có thể mất entry của nhau; lỗi lưu manifest từng làm mất mirror cũ |
| D10 | 2026-09-24 | Mirror root tạo mode 0700; bản local bỏ bit group/other-write và setuid/setgid/sticky | Mirror chứa bản sao file của root; thư mục 1777 từng thành 0777 ở local |
| D11 | 2026-09-25 | Định danh host = hostname + `machine-id` của server (probe đọc `/etc/machine-id`, fallback dbus). Thư mục mirror giữ tên hostname cho máy đầu tiên dùng nó; máy khác trùng tên dùng `<host>-<hash machine-id>`. Manifest lưu `machine_id`; upload chỉ dùng mirror khớp machine-id (không thì `NotMirrored`); dialog upload hiện đuôi machine-id | Hostname do host tự báo nên hai server trùng tên từng dùng chung mirror → có thể upload nội dung của A lên B. Host không có machine-id vẫn dùng tên thường (không bảo vệ được). VM clone từ cùng image có thể trùng machine-id |
| D12 | 2026-09-25 | Editor ngoài = setting có sẵn "Choose an editor to open file links", chỉ họ VS Code (`code`/`code-insiders`/`cursor`/`windsurf`); không thêm setting | Cần CLI có `--diff`; tránh thêm setting + mục palette bật/tắt; người dùng editor khác giữ hành vi cũ |
| D13 | 2026-09-25 | Baseline = git repo ở thư mục host, HEAD = trạng thái server lần sync cuối; không có git → bỏ qua; lỗi git chỉ là cảnh báo trong toast | Source Control của VS Code cho diff native, nhiều file, không cần viết UI; baseline là phụ, không được làm hỏng sync |
| D14 | 2026-09-25 | Không mirror Git metadata: bỏ mọi thành phần mà một filesystem nào đó hiểu là `.git` (không phân biệt hoa/thường, NTFS `.git.`/`GIT~1`/`::$stream`, ký tự HFS+ bỏ qua); `info/attributes` vô hiệu hoá filter/diff/merge/text; repo `sharedRepository=0600` | Security review: `.GIT` trên macOS/Windows sẽ thành `.git` thật do server điều khiển → VS Code chạy hook/config của server; `.gitattributes` của server có thể gọi filter driver trong config của user |
| D15 | 2026-09-25 | VS Code điều khiển Warp qua local control (`warpctrl`, action `sync.*`), không qua URI scheme; API hai bước (prepare → confirm/cancel) với xác nhận trong VS Code; pending ngoài dùng id `Uuid` và map riêng; hạ tầng bridge async (Task 2.4 của Agent Bridge) làm ở 7.1 | URI scheme không xác thực; giữ HTTP request trong lúc user suy nghĩ ở dialog Warp sẽ vượt timeout; client ngoài không được xác nhận dialog của Warp; làm hạ tầng chung một lần ở nơi ít rủi ro rồi Agent Bridge rebase dùng lại |
| D16 | 2026-09-25 | 7.1: bỏ helper `#[cfg(test)] expect_ready()` của thiết kế Agent Bridge; test `resolve_bridge_result` (mod.rs) thay cho test gọi thẳng `handle_request`; `BridgeResult::Pending` có `#[allow(dead_code)]` tới 7.3; `send_request_with_timeout` tách phần POST ra `post_request` | Không test nào gọi `handle_request` trực tiếp nên helper sẽ là code chết (clippy fail); Pending chưa có action nào tạo ra trước 7.3; tách `post_request` để test timeout không cần dựng broker socket |
| D17 | 2026-09-25 | 7.2: `Requester::External { reply, window_id }` (không phải `External(ExternalReply)`); `start_*` nhận `impl Into<Requester>` (call site cũ truyền `WindowId` không đổi); code mới nằm ở `warp_sync/requester.rs` (`Requester`, `ExternalReply`, `SyncReply`, `ConfirmationKind`, `Finished`); toast của client ngoài ghi "Warp Sync (local control): …" (không phải "(VS Code)"); chỉ toast `Started`, kết quả/lỗi chỉ trả về client; thêm `WarpSyncError::PendingNotFound` | `sync.confirm` không biết session nên cần cửa sổ lưu trong pending; giảm churn ở ~10 call site UI; CLI `warpctrl` cũng là client ngoài nên nhãn "VS Code" sẽ sai; hai thông báo (toast + modal VS Code) cho cùng kết quả là thừa; id lạ/hết hạn cần lỗi riêng |
| D18 | 2026-09-25 | 7.3: mọi action `sync.*` trả một kiểu `SyncResult` có tag `status` (`crates/local_control/src/protocol.rs`, dùng chung app + CLI); lỗi `WarpSyncError` map sang `ErrorCode` sẵn có (`InvalidPath`→`invalid_params`, `NoSession`→`missing_target`, `AmbiguousSession`→`ambiguous_target`, `PendingNotFound`→`stale_target`, `AlreadyInProgress`→`target_state_conflict`) + một mã mới `sync_failed` cho phần còn lại; `canonicalize` path chạy trong future nền (không stat trên main thread), session được chọn lại trên main thread trong callback (không giữ session cũ qua await); path là **thư mục host** (không có remote path) bị từ chối cho download/upload/compare (`/` bị `normalize_remote_path` từ chối) nhưng `sync.status` chấp nhận; chọn session dùng `host_dir_matches` (tên host hoặc tên + `-` + 8 hex) chỉ để **lọc ứng viên**, ranh giới an toàn là `expected_host_key` kiểm sau `resolve_host_key` | Tránh sai lệch kiểu giữa hai phía; tránh chặn UI thread (nit review Phase 6); session có thể đóng giữa chừng; hostname do server tự báo nên có thể nhái tên thư mục — chỉ so khớp tên là chưa đủ |
| D19 | 2026-09-25 | 7.5: thêm devDependency `@types/node` (cần cho `child_process`/`node:test`); extension chỉ chạy `warpctrl` khi có thư mục workspace nằm dưới `~/.warp/mirrors` (hoặc `warpSync.mirrorRoot`); nút `scm/title` là "Upload/Compare Changed Files" — lấy file đổi từ API của extension Git có sẵn, cho user chọn (QuickPick), rồi chạy tuần tự từng file (một modal mỗi file); nút modal là "Upload Anyway" khi có cảnh báo; `--` đặt trước mọi path/id trong argv; kết quả của Warp được kiểm kiểu ở `protocol.ts` trước khi dùng | Không có gói kiểu nào khác cho `node:*`; tránh bật tiến trình cho mọi workspace; thư mục mở là thư mục host nên không thể upload/compare nguyên thư mục; ngăn tên file bị hiểu là option; JSON từ tiến trình ngoài là dữ liệu không tin cậy |
| D20 | 2026-09-25 | Sửa theo review 7.6: (a) `resolve_host_key` **từ chối** host không báo machine-id khi manifest của thư mục đó đã ghi machine-id (thay đổi D11: trước đây dùng tên thường); (b) `execute_upload` probe lại và so `user`/`uid`/`machine_id` với lúc chuẩn bị trước khi ghi gì; (c) tối đa `MAX_EXTERNAL_PENDING` = 8 thao tác chờ client ngoài (`TooManyPending`); (d) `PendingUpload` lưu khoá `in_flight` đã lấy lúc `begin` (không suy lại từ `prepared.host_key`); (e) `control_error` và `printable` thoát cả ký tự điều khiển lẫn ký tự ẩn/đổi chiều (U+200B–200F, 2028–202E, 2060–2064, 2066–206F, FEFF, 00AD, 061C); (f) `resolve_mirror_path` từ chối symlink treo và tên có khoảng trắng đầu/cuối hoặc ký tự điều khiển; (g) CLI coi kết quả không parse được là **lỗi** (exit 1), không phải thành công; (h) khách ngoài bỏ kết nối trong lúc chờ xác nhận → huỷ pending ngay | (a) server tự quyết việc có báo machine-id hay không nên bỏ trống = lách được guard; (b) xác nhận có thể đến sau tối đa 10 phút, session có thể đã trỏ nơi khác; (c) mỗi pending giữ một archive trong RAM; (d) khoá sai key làm path kẹt `AlreadyInProgress` tới khi khởi động lại Warp khi mirror có hậu tố machine-id; (e) escape terminal/OSC 52, bidi spoof; (f) `normalize_remote_path` trim path nên tên `foo ` thành `foo`; (g) app/CLI lệch phiên bản có thể làm `needs_confirmation` thành exit 0 |
| D21 | 2026-09-25 | 7.7: mode của mục **mới** (chưa có trong manifest) lấy từ bản local — user chọn phương án (b). File: `local_mode & 0o6777` (setuid/setgid vẫn bị `SpecialMode` từ chối, sticky bị bỏ); thư mục: `& 0o777` (setuid/setgid/sticky bị bỏ); nền tảng không có bit quyền → `0644`/`0755`. Áp dụng cho **mọi** mục mới, kể cả file mới nằm trong thư mục đã sync (trước đây cố định `0644`/`0755`); mục đã có trong manifest giữ mode manifest. Bit ghi cho nhóm/người khác được giữ nguyên (không tự bỏ `g+w`/`o+w`) — dialog phải hiện mode để user thấy | User yêu cầu tôn trọng `chmod 600`; tự ý bỏ bit sẽ làm mode trên server khác mode user thấy ở local; dialog là chốt chặn |

### Nhật ký

- 2026-09-24 — Plan v2 được viết sau khi review v1 + POC (Claude Opus). Chưa bắt đầu Phase 0.
- 2026-09-24 — Phase 0 xong: xoá 4 file `tmp_sync_*.rs`, stash POC (`stash@{0}` "warp-sync POC wip"), tạo `feature/warp-sync` từ master (f4f9b8838), đánh dấu spec v1 SUPERSEDED, `cargo check -p warp` pass.
- 2026-09-24 — 1.1 xong: thêm `flate2`, `tar` vào workspace deps + `app/Cargo.toml` (không đụng `node_runtime`); `cargo check -p warp` pass.
- 2026-09-24 — 1.2–1.4 xong (gộp một commit): `error.rs`, `paths.rs`, `remote_script.rs` + test (55 test pass, gồm test chạy `sh` thật). `mod warp_sync` tạm có `#[allow(dead_code)]` trong `lib.rs` — **gỡ ở Task 2.3** khi module đã được dùng. Bổ sung so với plan: `host_key` chặn `.`/`..`/`.warp-sync` (thay dấu `.` đầu bằng `_`); `validate_tmp_dir` trả newtype `RemoteTmpDir` và từ chối `..`; commit script chỉ in `backup=` khi thực sự có backup; `nextest` chưa cài → dùng `cargo test -p warp --lib warp_sync`.
- 2026-09-24 — 1.5 xong: `manifest.rs` + 11 test. `load_or_default` nhận thêm `host_key` (cần để tạo manifest rỗng); thêm `entry`, `record_sync`, `last_sync`.
- 2026-09-24 — 1.6 xong: `archive.rs` + 25 test (tổng Phase 1: 91 test pass, clippy `-D warnings` sạch). Lưu ý: clippy cấm `std::process::Command` → dùng `command::blocking::Command` (kể cả trong test). Khác plan: `locally_modified_files` nhận thêm tham số `root` và tính cả file local **chưa có trong manifest** (nếu không, swap sẽ xoá file mới của user); `UploadArchive` có `content_bytes` thay cho `total_len`; `verify_gzip_trailer` đọc hết stream để kiểm CRC (tar dừng trước trailer).
- 2026-09-24 — Phase 2 xong (117 test pass, clippy sạch). Flag: biến thể `FeatureFlag::WarpSync` (`crates/warp_features/src/lib.rs`, thêm vào `DOGFOOD_FLAGS`) + Cargo feature `warp_sync` (`app/Cargo.toml`) + dòng `#[cfg(feature = "warp_sync")]` trong `app/src/features.rs`. **Build `./script/run` trên máy này là kênh OSS (không có `warp-channel-config`) → chỉ có `DEBUG_FLAGS`, nên phải chạy `./script/run --features warp_sync`** (kênh local/dev thì tự bật qua DOGFOOD_FLAGS). `transfer.rs` chạy trên trait `RemoteShell` (`remote_shell.rs`; `SessionShell` bọc `Arc<Session>`), test end-to-end bằng `sh` cục bộ. `WarpSyncModel` đã đăng ký singleton ở `app/src/lib.rs`. `#[allow(dead_code)]` trên `mod warp_sync` **vẫn còn — gỡ ở Task 3.5** (UI mới là nơi dùng model).
- 2026-09-24 — 3.1–3.4 xong (126 test pass; `cargo clippy -p warp --all-targets --tests -- -D warnings` sạch). Chi tiết: toast + dialog nằm ở `Workspace` (`handle_warp_sync_event`, chỉ xử lý event có `window_id` của chính nó). Dialog xác nhận (`warp_sync/confirm_dialog.rs`) **không gán phím Enter** (chỉ Escape=Cancel) vì cả hai dialog bảo vệ khỏi mất dữ liệu; nút xác nhận dùng `DangerPrimaryTheme` nguyên bản. Context menu: item "Warp Sync: Download to local mirror" chỉ hiện khi flag bật và vùng chọn nằm gọn trong **một block** thuộc session không-local; block được xác định qua helper mới `BlockList::selected_block_index` (menu chuột phải trên text không mang block index). Palette: `workspace:warp_sync_download_cwd`, `workspace:warp_sync_open_mirror` (đăng ký trong `if FeatureFlag::WarpSync.is_enabled()`, không thuộc group Settings). Cả hai action mới thuộc nhánh `should_save_app_state_on_action() == false`. `#[allow(dead_code)]` trên `mod warp_sync` **vẫn còn** vì code upload (Phase 4) chưa có UI gọi tới — gỡ ở Phase 4.
- 2026-09-24 — 3.5 xong (review bằng `code-reviewer` + `security-reviewer`, sửa: rollback swap khi lưu manifest lỗi; thư mục `new`/`previous` tách nhau (trước đó tên file `previous` trùng); khoá manifest; kiểm tra lại sửa đổi local ngay trước swap; giữ khoá in-flight khi chờ xác nhận + chống path lồng nhau; dialog thứ hai huỷ pending của dialog bị thay + trả focus; đóng gói upload đọc file một lần (header size khớp); backup name có nonce; guard symlink cho thư mục backup; host_key có hash; manifest kiểm host_key; từ chối upload file setuid/setgid; mirror 0700 + mask quyền local; chặn giải nén theo tổng stream; log `safe:` không chứa lỗi từ xa). Kết quả: `cargo nextest run -p warp --lib -E 'test(/^(workspace::|terminal::model::blocks|terminal::view|warp_sync)/)'` → 768/768 pass; clippy `-D warnings` sạch.
- 2026-09-25 — CHECKPOINT A: người dùng test tay phần lớn mục 5 (download) trên host `draff3` (root qua sudo), chưa thấy lỗi; file local thuộc user thường, owner thật (root:root 0644) nằm trong manifest — đúng thiết kế. Chưa ghi thời gian tải 1/5/20 MB. Sửa thêm D11 (hostname trùng): 157 test warp_sync pass, clippy sạch.
- 2026-09-25 — Phase 4 xong, dừng ở CHECKPOINT B. Phần lõi upload (`prepare_upload`/`execute_upload`, `UploadSummary`, dialog xác nhận với Cancel là mặc định vì không gán Enter, backup path trong toast) đã có từ Phase 2–3 nên Phase 4 chỉ còn nối UI: context menu "Warp Sync: Upload from local mirror" (`ContextMenuAction::WarpSyncUpload`, dùng chung `warp_sync_selection_request` với download), palette `workspace:warp_sync_upload_cwd` (`WorkspaceAction::WarpSyncUploadCurrentDirectory`, không lưu app state), gỡ `#[allow(dead_code)]` trên `mod warp_sync` (xoá cảnh báo bằng cách đưa số thư mục/dung lượng vào toast; `Manifest::entry`/`last_sync` chỉ dùng trong test nên `#[cfg(test)]`). 159 test warp_sync + 779 test workspace/terminal pass; clippy `-D warnings` sạch; `./script/format` đã chạy.
- 2026-09-25 — 5.1 xong: `prepare_upload` chạy thêm `checksum_script` (`find -type f -exec sha256sum|shasum -a 256 {} +`, luôn `exit 0`) và so hash remote với `sha256` trong manifest (`remote_check.rs`: `find_remote_conflicts` → `changed` / `missing` / `already_exist`). Fingerprint chính là sha256 đã có trong manifest, không đổi định dạng manifest. Dialog upload hiện cảnh báo; host không có công cụ hash (hoặc lệnh hash lỗi `RemoteCommandFailed`) → `RemoteCheck::Unavailable` + dòng cảnh báo, không chặn upload. Kiểm tra chạy lúc chuẩn bị (trước dialog), không chạy lại sau khi user bấm Upload. 177 test warp_sync pass.
- 2026-09-25 — 5.2 xong: `warp_sync/path_prompt.rs` (`WarpSyncPathPrompt`: `Dialog` + `EditorView::single_line`, Enter=xác nhận, Escape=huỷ; Enter được phép ở đây vì mở prompt không thay đổi gì — Download vẫn hỏi khi ghi đè, Upload vẫn qua dialog xác nhận). Palette: `workspace:warp_sync_download_path` / `workspace:warp_sync_upload_path` (`WorkspaceAction::WarpSyncDownloadPath`/`WarpSyncUploadPath`, không lưu app state). Path lỗi (`InvalidPath`) hiện inline và giữ prompt mở; lỗi khác (không phải session remote…) đóng prompt + toast. Path tương đối resolve theo pwd của session active. Không có unit test cho view (cần harness `EditorView` nặng); logic chuẩn hoá path đã có test ở `paths_tests.rs`.
- 2026-09-25 — 5.3 xong: nhóm settings `WarpSyncSettings` (`app/src/settings/warp_sync.rs`, chỉ đăng ký khi `FeatureFlag::WarpSync` bật, `SyncToCloud::Never`): `warp_sync.mirror_root` (rỗng = `~/.warp/mirrors`; hỗ trợ `~/`; phải tuyệt đối), `warp_sync.max_download_mib` (mặc định 32, tối đa 128), `warp_sync.max_upload_mib` (mặc định 4, tối đa 16). Hằng `MAX_DOWNLOAD_KIB`/`MAX_UPLOAD_BYTES` được thay bằng `SyncLimits` (`warp_sync/config.rs`) truyền qua `DownloadRequest`/`UploadRequest`/`build_upload`; `SyncConfig::from_settings` đọc setting ở `WarpSyncModel::begin` và ở `warp_sync_open_mirror`. `paths::mirror_root()` bị xoá; `host_mirror_dir` nhận `mirror_root`. Trang Settings `SettingsSection::WarpSync` ("Warp Sync", ẩn khi flag tắt; nav chèn trước Shared blocks) có 3 ô nhập, commit khi Enter/blur, giá trị không hợp lệ thì hoàn về giá trị đang lưu. Đổi mirror folder **không** di chuyển mirror cũ (ghi trong mô tả). Không thêm mục Command Palette bật/tắt vì đây không phải setting dạng toggle. 288 test warp_sync/settings + 563 test gồm settings_view pass.
- 2026-09-25 — 5.4 xong: `transfer::compare` tải bản mới của path vào staging (không đụng mirror/manifest), rồi `diff::compare_trees` so với mirror theo sha256 và gán nhãn theo manifest (`ChangedLocally` / `ChangedOnServer` / `ChangedOnBoth` / `NewOnServer` / `DeletedLocally` / `NewLocally` / `DeletedOnServer`; không có baseline → `ChangedUnknown`). Diff unified (crate `similar`, timeout 2 s/file; file > 1 MiB hoặc nhị phân chỉ liệt kê; cắt ở 8 MiB) ghi ra `<mirror_root>/.warp-sync/diffs/<host_key>/<path>.diff` (0600, ngoài mirror để không bị nhầm là file đã sync). `-` là server, `+` là mirror local. UI: dialog kết quả dùng lại `WarpSyncConfirmDialog` (`ConfirmKind::CompareResult`, nút "Open diff" mở file bằng code editor của Warp, nút Đóng; nút không đỏ vì không phá dữ liệu); không có khác biệt → toast thành công. Vào từ context menu "Warp Sync: Compare with local mirror" và palette `workspace:warp_sync_compare_cwd` / `workspace:warp_sync_compare_path` (prompt nhập path). `ConfirmRequest.id` được thay bằng `ConfirmKind { OverwriteLocalChanges{id}, Upload{id}, CompareResult{diff_path} }`. Compare yêu cầu path đã được download (không thì `NotMirrored`). 1043 test (warp_sync + settings_view + workspace + terminal::view) pass; clippy `-D warnings` sạch.
- 2026-09-25 — Phase 5 review (`code-reviewer` + `security-reviewer`), đã sửa: mirror folder không được là `/`, `$HOME` hay thư mục cha của `$HOME` (host từ xa chọn tên thư mục con nên mirror root chung với dữ liệu user có thể bị thay); sentinel `no_hash_tool` chỉ nhận khi là **toàn bộ** output (tên file không giả được); tên file có ký tự điều khiển được escape (`printable`) trong dialog và file diff; `Timeout` của lệnh hash → `RemoteCheck::Unavailable` thay vì chặn upload; header report dùng hostname thật (`CompareRequest.hostname`); kết quả compare **không thay** dialog đang mở (tránh huỷ ngầm một upload đang chờ xác nhận) mà hiện toast kèm đường dẫn diff; lỗi trong path prompt tự xoá khi sửa; tham số `app`→`ctx`. Chưa sửa (ghi nhận): trang Settings hoàn giá trị không hợp lệ mà không báo lý do; file diff của các path khác nhau có thể trùng tên sau khi sanitize (ghi đè nhau); dòng hash của tên file có `\`/newline bị coi là "missing" (cảnh báo giả, hiếm); diff cũ còn lại khi lần compare sau không có khác biệt; mirror root có sẵn không bị kiểm quyền sở hữu/mode. 1152 test pass (một test `cloud_preferences_syncer` từng fail 1 lần do timing, chạy lại pass), clippy sạch, `./script/format` đã chạy.
- 2026-09-25 — Phase 6.1–6.3 xong (6.4 chỉ ghi chú). `warp_sync/editor.rs` (`EditorCli`, `EditorRequest`, `invocations`, `launch`; CLI chạy trên background qua `WarpSyncModel::open_in_editor`, lỗi → toast `Editor`/`NoEditor`); toast download có link "Open in <editor>" (`MirrorLocation { host_dir, local_path, is_file }`); palette `workspace:warp_sync_open_mirror_in_editor`. `warp_sync/baseline.rs`: repo git ở `<mirror_root>/<host_key>`; download commit `--only` cả path (gồm file server đã xoá); upload chỉ commit file đã upload và có đổi (`git status` ∩ file trong archive, vì `status` không nhận danh sách file từ stdin); pathspec qua `--pathspec-from-file` NUL + `--literal-pathspecs`; hook/fsmonitor/ký commit bị tắt bằng `-c` trên dòng lệnh; bỏ env `GIT_*`; `configure()` chạy lại mỗi lần sync (tự sửa khi setup bị ngắt); khoá `BASELINE_LOCK`. Cần git ≥ 2.26. Compare giữ bản server ở `.warp-sync/compare/<host_key>/…` (file 0400); dialog "Open in <editor>" → tối đa `MAX_EDITOR_DIFFS` tab `code -r --diff` + report khi còn khác biệt chưa hiện; không còn khác biệt → xoá report và bản copy cũ (sửa nit "diff cũ còn lại" của Phase 5). Review (`code-reviewer` + `security-reviewer`), đã sửa: tên kiểu `.GIT` lọt qua trên filesystem không phân biệt hoa/thường (CRITICAL, xem D14), `.gitattributes` gọi filter driver (HIGH), git objects chưa 0600, setup repo bị ngắt không tự sửa, `stat()` trên UI thread. Chưa sửa (ghi nhận): các lệnh `code -r --diff` chạy nối tiếp có thể mở cửa sổ mới thay vì cửa sổ workspace khi VS Code chưa chạy (chưa kiểm chứng — mục 16 checklist); manifest cũ có mục `.git` (tải trước Phase 6) sẽ hiện trong "Missing from the local mirror" tới lần download lại. Test: warp_sync + workspace pass (474), clippy `-D warnings` sạch, `./script/format` đã chạy.
- 2026-09-25 — Thêm Phase 7 (VS Code điều khiển Warp Sync qua `warpctrl`) vào plan theo yêu cầu user; chưa code. Thứ tự đã chốt với user: test tay Phase 5–6 (CHECKPOINT C) → Phase 7 → Agent Bridge (rebase lên `feature/warp-sync`, bỏ Task 2.4 và phần client timeout của 3.9 vì 7.1 đã làm).
- 2026-09-25 — CHECKPOINT C qua (user đã test tay 13–19, không lỗi). 7.1 xong (commit riêng, không action mới): `BridgeResult { Ready, Pending { request_id, receiver } }` trong `bridge.rs`, `handle_request` trả `BridgeResult`; `mod.rs::resolve_bridge_result` await `Pending` trên runtime HTTP (`Canceled` → `BridgeUnavailable`); `client::send_request_with_timeout` (+ `DEFAULT_REQUEST_TIMEOUT` 30 s = mặc định cũ của `reqwest::blocking::Client::new()`, `send_request` gọi lại). Test: 4 test `resolve_bridge_result` + 2 test timeout của `post_request` (server im lặng → lỗi; server chậm 400 ms vẫn nhận được). `nextest -p local_control` 42/42, `nextest -p warp --lib -E 'test(/local_control::/)'` 39/39, clippy `-D warnings` sạch cho `local_control` và `warp`. Chưa chạy `./script/format` (gộp một lần ở cuối Phase 7 theo yêu cầu user; diff format hiện tại chỉ là thứ tự import/wrap).
- 2026-09-25 — 7.2 xong. `WarpSyncModel` phân biệt `Requester::Window` (event → toast/dialog như cũ) và `Requester::External` (`SyncReply` qua oneshot, không dựng dialog). Pending của client ngoài ở map riêng `external_pending: HashMap<Uuid, _>` (id `Uuid::new_v4()`), tự huỷ sau `EXTERNAL_PENDING_TTL` (10 phút, `mod.rs`; model có `with_external_pending_ttl` cho test) và nhả khoá `(host, path)`; `confirm_external` / `cancel_external` chỉ nhìn map này, còn `confirm_*`/`cancel_pending` chỉ nhìn map cửa sổ nên hai bên không xác nhận nhầm của nhau. Mọi chuỗi từ server trong `SyncReply` đi qua `printable` (`Finished::into_reply`, `ConfirmationKind::upload/overwrite_local_changes`). Refactor gộp luồng: `report`/`announce`/`resume_download`/`resume_upload`/`await_*_confirmation` dùng chung cho hai loại requester. Tạm thời có `#[allow(dead_code)]` (đầu `requester.rs`, `confirm_external`, `cancel_external`, `PendingNotFound`) — **gỡ ở 7.3** khi handler dùng tới. 12 test mới trong `model_tests.rs` (định tuyến Window/External, TTL thật 20 ms, id riêng, không xác nhận chéo, path bị giữ/nhả) + 9 test `requester_tests.rs`; `nextest -E 'test(/^(warp_sync::|local_control::|workspace::)/)'` pass (516 + …), clippy `-D warnings` sạch.
- 2026-09-25 — 7.3 xong. Catalog: nhóm `sync` (6 action, target `File`, tổng 90 action); params `SyncPathParams`/`SyncStatusParams`/`SyncPendingParams` (`pending_id` là `Uuid`, sai định dạng bị chặn ở `validate_action_params`); `handlers/sync.rs` (luồng: kiểm cờ `WarpSync` → parse params → `SyncConfig` → `resolve_mirror_path` trong future nền → chọn session trên main thread → `WarpSyncModel::start_*` với `Requester::external(window_id, Some(host_dir_name))` → `forward` chờ `SyncReply` rồi trả `Pending` cho HTTP) và `handlers/sync_reply.rs` (SyncReply→SyncResult, WarpSyncError→ControlError, match exhaustive). `warp_sync/external.rs`: `resolve_mirror_path` (tuyệt đối, cấm `..`, `canonicalize` + phải nằm dưới mirror root đã canonicalize, cho path chưa tồn tại nếu thư mục host có sẵn, chặn `.warp-sync`, `.git`, symlink thoát ra) và `choose_session` (0 → `NoSession`; 1 → dùng; nhiều → session active của cửa sổ đang focus nếu duy nhất, không thì `AmbiguousSession` kèm danh sách tối đa 5). `expected_host_key: Option<String>` thêm vào `DownloadRequest`/`UploadRequest`/`CompareRequest`, kiểm ngay sau `resolve_host_key` → `Manifest("the mirror belongs to another machine")`; `Requester::External` mang `expected_host_key`. Gỡ hết `#[allow(dead_code)]` của 7.1/7.2. Test: 22 `external_tests`, 7 `sync_reply_tests`, 9 `sync_tests` (đi qua `handle_control_request` thật: cờ tắt → `UnsupportedAction`, status, path ngoài mirror/`..`/`.warp-sync`/`.git`/thư mục host → `invalid_params`, không có session → `missing_target` nêu tên host, session tường minh không tồn tại → `stale_target`, confirm/cancel id lạ → `stale_target`, id sai định dạng → `invalid_params`), 4 test `expected_host_key` trong `transfer_tests`, 2 test `host_dir_matches`, 6 test protocol; `nextest -p warp -E 'test(/^(warp_sync::|local_control::|workspace::|terminal::view)/)'` 914 pass, `-p local_control` 47 pass, clippy `-D warnings` sạch cho `warp`/`local_control`/`warp_cli`. **Không chạy được `cargo nextest run -p warp_cli`/`-p local_control -p warp_cli` trên máy này**: build script của `yeslogic-fontconfig-sys` cần `fontconfig.pc` (thiếu gói dev, không liên quan thay đổi) — test của `warp_cli` (7.4) phải chạy theo cách khác hoặc để CI. Ghi chú cho 7.5: nút `scm/title` "cho cả thư mục đang mở" không dùng được vì thư mục mở là thư mục host (bị từ chối) — extension nên thao tác trên các file thay đổi trong Source Control hoặc hỏi user chọn thư mục con.
- 2026-09-25 — 7.4 xong, dừng ở CHECKPOINT D. `crates/warp_cli/src/local_control/sync.rs`: `warpctrl sync status [PATH]`, `download PATH`, `upload PATH` (chỉ *chuẩn bị*: in tóm tắt + lệnh `confirm`/`cancel`), `confirm ID`, `cancel ID`, `compare PATH`; path tương đối được đổi thành tuyệt đối theo thư mục hiện tại (`std::path::absolute`, app vẫn kiểm lại); `confirm`/`cancel` chỉ nhận `--instance`/`--pid` (không có selector session); `--output-format json|ndjson` in nguyên `data`; **exit code 3** khi kết quả là `needs_confirmation` (mọi định dạng), 1 khi lỗi, 0 còn lại (`run_inner` giờ trả `Result<u8, _>`; `EXIT_SUCCESS/EXIT_FAILURE/EXIT_NEEDS_CONFIRMATION`). Client dùng `send_request_with_timeout` với `SYNC_CLIENT_TIMEOUT` = 10 phút; `commands.rs::send_action` tách phần gửi khỏi phần in (các lệnh cũ dùng `DEFAULT_REQUEST_TIMEOUT` 30 s, hành vi không đổi). Danh sách tối đa 10 path rồi "... and N more". Test: 14 test mới trong `local_control_tests.rs` (parse, ràng buộc selector, exit code, path tuyệt đối, hiển thị từng loại kết quả) + bảng phủ mọi `ActionKind` có lệnh CLI. **Cách chạy test `warp_cli` trên máy này** (build đơn lẻ hỏng vì `fontconfig.pc`, feature của `warp` bật `dlopen` nên gộp package thì được): `cargo nextest run -p warp_cli -p warp --lib --features warp/warp_sync,warp/warp_control_cli -E 'package(warp_cli)'` → 277 pass; clippy `cargo clippy -p warp -p local_control -p warp_cli --features warp/warp_sync,warp/warp_control_cli --all-targets --tests -- -D warnings` sạch. `./script/format` chưa chạy (một lần ở cuối Phase 7).
- 2026-09-25 — CHECKPOINT D: user giao toàn quyền (server test) nên agent tự chạy checklist CLI trên `draff3` qua instance Warp đang mở: status/download/upload/confirm/cancel/compare, exit code 3 và JSON, khoá path (trùng và lồng nhau), cảnh báo "changed on the server" (mô phỏng bằng sửa hash trong manifest rồi khôi phục), guard "another machine" trên mirror thật `draff3-85d628b5`, path xấu, chạy song song, không rò file tạm trên server. Đều đạt. Chưa kiểm: 2 tab cùng host (`AmbiguousSession`), đóng tab giữa chừng, shell đang chạy `top`, tắt Scripting (cần thao tác giao diện). Dấu vết trên server: `/root/test-dir/test.conf` và `/root/test-dir/new-from-cli.txt` (agent tạo), backup trong `/root/.warp-sync/backups/`.
- 2026-09-25 — 7.5 xong: `tools/vscode-warp-sync/` (TypeScript 5.9, không dependency runtime; devDeps `typescript`, `@types/vscode`, `@types/node`). Module thuần (không import `vscode`): `warpctrl.ts` (dựng argv `[...command, --output-format, json, sync, <sub>, --, <operand>]` không qua shell; exit 0/3 = kết quả, còn lại đọc `{ok:false,error}` hoặc stderr; gợi ý bật Settings > Scripting), `protocol.ts` (kiểm kiểu JSON của Warp), `prompts.ts` (nội dung modal upload/overwrite), `compare.ts` (tối đa 10 diff `server ↔ mirror`, báo cáo `.diff` khi còn phần một phía; path phải nằm trong thư mục gốc), `status.ts`, `changes.ts`, `mirror.ts`; `runner.ts` (`execFile`, timeout 11 phút); `extension.ts` + `git.ts` là phần gắn VS Code (menu explorer/editor tab/`scm/title`, palette, status bar `root@host` / `no Warp session`, poll 30 s khi cửa sổ focus, `withProgress`). Cài: `npm run package` → `.vsix` (16 KB). Test: `npm test` 33 pass (`node --test`); chạy thật `runProcess`+`WarpctrlClient` với Warp; chạy `extension.ts` với module `vscode` giả trên Warp thật: activate → status bar `root@draff3`; upload (chấp nhận / đóng modal = huỷ, khoá được nhả / upload lại), download đè bản sửa local (modal Overwrite), compare mở đúng `vscode.diff` (trái = bản server), upload thư mục host lỗi gọn, lệnh status. Chưa chạy trong VS Code thật (chưa bấm chuột phải/SCM) — thuộc CHECKPOINT E. Ghi chú: nút SCM cần repo Git ở thư mục host (có sau lần download đầu tiên).
- 2026-09-25 — 7.6 xong (review bằng `code-reviewer` + `security-reviewer`; không có CRITICAL, 2 HIGH đã sửa, xem D20). **Đã sửa:** HIGH khoá `in_flight` nhả sai key khi mirror có hậu tố machine-id (có từ Phase 4; path kẹt tới khi khởi động lại; 3 test mới); HIGH server giấu machine-id lách guard (test mới, đổi test cũ `a_host_that_reports_no_machine_id_uses_the_plain_name`); re-probe lúc confirm (test mới); giới hạn pending ngoài; `printable`/`control_error`; symlink treo + tên trim; huỷ pending khi client bỏ đi (`forward` trong `handlers/sync.rs`, **chưa có test riêng** vì cần dựng pending giả từ ngoài `warp_sync`); CLI exit code khi kết quả lạ + `send_action` trả đúng payload lỗi; extension: bỏ kết quả `status` cũ khi có câu hỏi mới hơn, `compare` nhiều file không dừng ở file lỗi đầu tiên, thông báo timeout đúng, `null` không hợp lệ cho field tuỳ chọn, `untrustedWorkspaces.supported`, mục "Security notes" trong README. **Chưa sửa (ghi nhận):** không có setting/dialog riêng cho upload từ client ngoài (M1 — câu hỏi mở cho user, xem mục 6); `.vscode`/`*.code-workspace` từ server vẫn được mirror; thông báo lỗi VS Code có thể tự tạo link Markdown từ chuỗi của server (L3); `diff_path`/`local_path` do Warp trả về mở mà không kiểm nằm trong mirror (L4); biến thể tên ngắn NTFS `git~2` (L5); catalog liệt kê `sync.*` là Implemented khi cờ tắt (L6); poll trạng thái ngừng khi workspace là chính `~/.warp/mirrors` (chỉ đúng lại sau sự kiện workspace/cấu hình) và đa workspace chỉ hiện mirror đầu tiên (L7); `extension.ts`/`git.ts`/`runner.ts` chưa có test tự động (đã chạy tay bằng `vscode` giả); thiếu test end-to-end `start_operation` với session giả. Kiểm tra: `nextest` (warp_cli + `warp_sync::|local_control::|workspace::|terminal::view`) 1200 pass, `-p local_control` 47 pass, extension `npm test` 34 pass, clippy `-D warnings` sạch cho `warp`/`local_control`/`warp_cli`. `./script/format` đã chạy (commit `style` riêng).
- 2026-09-25 — User hỏi về file/thư mục mới tạo trong mirror. Đã kiểm chứng trên `draff3`: upload thẳng path mới → `Not found on the remote host` (probe chạy trước); upload thư mục cha đã sync → mục mới hiện trong "New files" và lên server, owner theo cha (`root:root`), mode `0644`/`0755`; cách vòng `mkdir` trên server → `sync download` thư mục rỗng → upload riêng thư mục đó chạy được. Thêm Phase 7b / Task 7.7 vào plan (chưa code). Extension VS Code đã cài bản 7.6 (`281499f96`); CHECKPOINT E chưa có kết quả từ user.
- 2026-09-25 — 7.7 bước 1 (mode của mục mới, D21): user chọn phương án (b). `archive.rs`: `LocalItem.mode` (bit quyền của bản local), `new_entry_mode` thay cho hằng cố định; 4 test mới trong `archive_tests.rs` (giữ bit `rwx`, mục đã biết giữ mode manifest, từ chối setuid/setgid, thư mục không nhận bit đặc biệt), test cũ `upload_carries_edits_…` đặt mode tường minh để không phụ thuộc umask. `nextest -E 'test(/warp_sync::/)'` 323 pass. Chưa làm: dialog hiện mode (bước 5), đường "path mới" (bước 2–4).
- 2026-09-25 — 7.7 bước 2 (script remote): `remote_script.rs` — `CommitMode { Replace, CreateOnly }` trong `UploadCommit`; `CreateOnly` bỏ backup và **từ chối** khi `$P` là symlink/không phải thư mục (exit 87) hoặc `$P/$N` đã có (kể cả symlink treo, exit 86) — kiểm ngay trước `tar -x` vì `tar -x` sẽ đặt lại owner/mode của thư mục đã có và ghi xuyên symlink; `light_probe_script` (như `probe_script` nhưng không `du`, để probe thư mục tổ tiên có thể rất lớn như `/home` mà không tốn thời gian). 6 test mới chạy `sh` thật (tạo file, target tồn tại, symlink treo, cha thành symlink, cha mất/là file, probe nhẹ); `nextest -E 'test(/warp_sync::remote_script::tests::/)'` 44 pass. Đã viết trước (đỏ, chưa commit) 24 test `transfer_tests` cho đường path mới — làm xanh ở bước 3–4.
- 2026-09-25 — 7.7 bước 3 (đóng gói path mới): `paths::components_below(anchor, path)` (các cấp giữa tổ tiên và path; từ chối `.`/`..`/tên rỗng/ký tự điều khiển/`.git`/không nằm dưới anchor — phòng thủ thêm dù path đã qua `normalize_remote_path`), `Manifest::nearest_dir` (trả cả path của thư mục), `archive::build_new_upload(root, anchor, …)`: archive có mục thư mục tường minh cho từng cấp trung gian (owner của `anchor`, mode `0755`, tính vào `dirs` và `new_files`), yêu cầu `anchor` đúng là thư mục đã sync gần nhất và manifest chưa có gì dưới `root`. `build_upload` giờ và `build_new_upload` dùng chung `pack` (tách `entry_header`, `append_dir`); hành vi `build_upload` không đổi (toàn bộ test cũ vẫn xanh). 10 test mới; `nextest -E 'test(/warp_sync::(archive|paths|manifest)::tests::/)'` 100 pass.
- 2026-09-25 — 7.7 bước 4 (transfer): `prepare_upload` — khi probe path trả `not_found` thì rẽ sang `prepare_new_upload`: probe nhẹ `/` để biết machine-id (probe của path không tồn tại không trả machine-id; không dùng `du` nên rẻ) → `resolve_host_key` + `ensure_expected_host_key` **trước** mọi thứ khác → manifest không được có gì dưới path và phải có thư mục tổ tiên `anchor` (`Manifest::nearest_dir`), bản local phải có; nếu không → giữ lỗi cũ `NotFound(path)` → probe nhẹ `anchor` (mất → `NotFound(anchor)`, là file → `InvalidPath("… is not a directory …")`, không có `base64` → `MissingTool`) → nếu cấp ngoài cùng sẽ tạo (`anchor/<cấp đầu>`, khác path) đã có trên server mà chưa sync thì `NotMirrored(<cấp đó>)` (nếu không, `tar -x` sẽ đặt lại owner/mode của thư mục đang có; user phải download nó trước) → `build_new_upload`. `PreparedUpload.placement: UploadPlacement { Replace, Create { anchor } }` (thay cho `probe_path` trong plan: `probe_path()`/`extraction()`/`commit_mode()` suy ra từ đó); `ensure_same_target` probe lại `anchor` (nhẹ) và trả `NotFound` khi nó mất; `send_and_commit`/`record_upload` dùng `(anchor, cấp đầu)` làm `(-C, name)` nên manifest ghi cả các thư mục trung gian; không backup, `remote_check` = `Checked` rỗng. 25 test `transfer_tests` (LocalSh) + test baseline git; `nextest -E 'test(/warp_sync::/)'` 363 pass. Còn lại: dialog/summary/CLI/extension hiện "sẽ tạo" + mode (bước 5).
- 2026-09-25 — 7.7 bước 5a (dialog/CLI/protocol): `UploadArchive.new_modes` (mode của mọi mục mới, gồm các cấp trung gian) → `UploadSummary.new_modes` + `creates_under: Option<String>` (= `anchor` khi tạo path mới). Dialog upload: khi tạo mới hiện "Creates on the server, inside <anchor>:" liệt kê từng mục kèm `(mode 0644)`, bỏ câu "unchanged since the last sync" và thay câu backup bằng "Nothing on the server is replaced, so no backup is made."; khi upload thường, "New on the server:" cũng kèm mode. Protocol `SyncUploadSummary` thêm 2 trường **tuỳ chọn, tương thích ngược** (`new_file_modes`: path → `"0644"` dạng bát phân, bỏ khi rỗng; `creates_under`); CLI `warpctrl sync upload` in "Creates on the server, inside …" + mode; mọi chuỗi mới qua `printable`. Test: 2 dialog, 1 requester (escape), 2 protocol (gồm JSON từ app cũ), 1 sync_reply, 1 CLI, 2 archive; `nextest -p warp_cli -p warp --features … -E 'package(warp_cli) | test(/warp_sync::|local_control::/)'` 701 pass, `-p local_control` 49 pass. Extension VS Code: bước 5b.
- 2026-09-25 — 7.7 bước 5b (extension): `protocol.ts` nhận `new_file_modes` (Record<string, string>) và `creates_under` (tuỳ chọn, kiểm kiểu; `null`/số bị từ chối), `prompts.ts` — modal upload đề "Creates on the server, inside <anchor>:" + mode từng mục, "Nothing on the server is replaced, so no backup is made." thay cho câu backup khi tạo mới, "New files:" cũng kèm mode; README mô tả upload path mới. Khác plan ("không cần đổi extension ngoài README"): modal phải nói rõ **tạo** gì (mục "Bảo mật cần review riêng"), nên đổi 2 module thuần; nút SCM "Upload Changed Files" vốn đã liệt kê file untracked nên chạy được với file mới. `npm test` 37 pass (+3). Chưa đóng gói lại `.vsix`.
