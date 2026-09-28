# E — Sửa file từ xa ngay trong Warp — Implementation plan (v1)

> Viết ngày 2026-09-29 (Claude Opus 5.5), theo yêu cầu của người dùng và quyết định AO14/AO15 của
> `ROADMAP.md`. Phase E làm **trước G1**. Người dùng giao Claude quyết định thiết kế (AO13); người
> dùng chỉ làm các ⛔ CHECKPOINT test tay.

---

## 0. Quy tắc làm việc

1. Đọc `AGENTS.md` trước. Làm tuần tự theo task. Tới ⛔ CHECKPOINT thì **dừng**, ghi nhật ký, chờ
   người dùng test tay, không tự qua phase sau.
2. Validation theo mục "Implementation Validation Order" của `AGENTS.md`: `cargo check`/`cargo test`
   nhỏ nhất trong lúc làm; cuối mỗi phase: test liên quan →
   `cargo clippy -p warp -p languages --all-targets --tests -- -D warnings` → `./script/format` một lần.
3. Mọi quyết định mới ghi vào mục 7 (bảng `E#`), mọi task xong ghi nhật ký (ai làm, file đổi, lệnh
   test đã chạy, kết quả).
4. Không `.lock()` `TerminalModel` mới khi đang có lock ở call stack (mục "Terminal Model Locking" của
   `AGENTS.md`). E2 đụng `link_detection.rs`, nơi đang có `self.model.lock()`: dùng lại lock sẵn có.
5. Match exhaustive, không `unwrap`/`expect` trên dữ liệu từ server, file hay output terminal.
6. Không đổi hành vi Warp Sync hiện có (download/upload/compare, extension VS Code). E chỉ **thêm**
   một đường vào dùng lại chính model đó.

---

## 1. Mục tiêu và phạm vi

**Hiện nay:** download bằng Warp Sync → tìm thư mục mirror → mở VS Code → sửa → upload bằng
extension. **Sau E:** trong session SSH/`sudo -i` đã Warpify, thấy đường dẫn file ở đâu thì
Cmd/Ctrl-click (hoặc palette) → file mở trong editor code của Warp, đã tô màu → sửa → Save (Cmd/Ctrl-S)
→ hộp thoại "Upload to root@host?" có diff → Upload → server có file mới + backup, mirror có commit Git.

**Trong phạm vi:**
- E1: luồng Edit in Warp cho **một file** (palette + đường dẫn).
- E2: bấm vào đường dẫn trong output của block remote.
- E3: tô màu file cấu hình hệ thống + ánh xạ ngôn ngữ do người dùng khai báo.

**Ngoài phạm vi (để sau, ghi ở mục 6):** sửa cả thư mục, tự tải lại khi file trên server đổi,
chỉnh sửa đồng thời nhiều người, LSP cho file cấu hình, agent sửa file qua đường này (G4, cần O2).

---

## 2. Hiện trạng code (khảo sát 2026-09-29)

| Có sẵn | Dùng cho | Vị trí |
|---|---|---|
| `WarpSyncModel::start_download/start_upload(session, remote_path, requester)` — hỗ trợ **file đơn** (`is_file` trong probe), mirror theo máy (`host_key` + machine-id), baseline Git (`baseline::record_download/record_upload`, `commit_message`) | Toàn bộ phần chuyển file và lịch sử | `app/src/warp_sync/model.rs:259,291`, `transfer.rs:72,795`, `baseline.rs` |
| Hộp thoại xác nhận upload (`ConfirmRequest::upload`, kiểm server đã đổi qua `remote_check`, backup trên server) — **chỉ có tóm tắt**, chưa có diff | Xác nhận khi Save | `app/src/warp_sync/confirm_dialog.rs:103,168` |
| `resolve_mirror_path(mirror_root, local_path) -> MirrorPath` (mirror → host + remote path) | Biết file đang mở thuộc server nào | `app/src/warp_sync/external.rs:31` |
| Editor code của Warp: `open_file_notebook(path, session, layout)` qua `pane_group::Event::OpenFileInWarp`; `LocalCodeEditorEvent::FileSaved { auto_saved }` | Mở file mirror, bắt sự kiện Save | `app/src/workspace/view.rs:16497`, `app/src/code/local_code_editor.rs:109,1694` |
| Tô màu tree-sitter qua `arborium` (35 ngôn ngữ nhúng, query highlight lấy từ `arborium`), chọn ngôn ngữ theo tên file/đuôi; `language_by_name` để ép ngôn ngữ | E3 | `crates/languages/src/lib.rs:130-200`, `app/src/code/editor/model.rs:1318-1335`, `Cargo.toml:307` |
| `arborium` 2.13 có sẵn (chưa bật): `lang-nginx`, `lang-ini`, `lang-ssh-config`, `lang-diff`, `lang-perl`, `lang-awk`, `lang-jinja2`, `lang-caddy`, `lang-groovy`, `lang-vim`, `lang-cmake`… | E3 | feature của crate `arborium` |
| Nhận diện đường dẫn file trong output: **tắt cho block remote** (`is_block_considered_remote`), vì kiểm tồn tại bằng file system local | E2 | `app/src/terminal/view/link_detection.rs:566-660` |
| Mở link: `maybe_open_link` → `GridHighlightedLink::File` → `open_file_path` (local) | E2 | `app/src/terminal/view.rs:19189-19260` |
| `similar` 2.7 (diff) đã là dependency của workspace | Diff trong hộp thoại upload | `Cargo.toml:285` |
| Flag `FeatureFlag::WarpSync` | Gate chung | `crates/warp_features/src/lib.rs:1005` |

**Upstream Warp đã có cách sửa file remote khác** (`app/src/remote_server`, `crates/remote_server`,
`LocalOrRemotePath::Remote`): cài binary `remote-server` lên server qua ControlMaster SSH rồi mở buffer
từ xa. Không dùng cho E (quyết định E7): nó chạy dưới **user đăng nhập SSH**, nên không sửa được file
root trong mô hình `ssh` → `sudo -i` (có mật khẩu sudo) mà Bridge/Warp Sync nhắm tới; phải cài phần mềm
lên server; và gắn với hạ tầng xác thực của Warp. Warp Sync đi **in-band qua đúng PTY đã `sudo -i`**,
không cài gì. Xem lại khi có kênh exec trực tiếp của G3.

**Không có:** grammar cho apache, crontab, sudoers, haproxy → các file này vẫn mở được, chỉ không tô
màu (hoặc người dùng ánh xạ sang ngôn ngữ gần giống, E3.3).

---

## 3. Thiết kế

### 3.1 Trạng thái: `RemoteEditModel`

Singleton mới trong `app/src/warp_sync/remote_edit.rs`, chỉ giữ trong RAM:

```rust
pub struct RemoteEditModel {
    /// Theo đường dẫn file trong mirror (đã canonicalize).
    open: HashMap<PathBuf, RemoteEditFile>,
}
pub struct RemoteEditFile {
    session_id: SessionId,        // session đã dùng để download; upload đi qua đúng session này
    remote_path: String,
    remote_user: String,
    hostname: String,
    state: RemoteEditState,       // Clean | Unsynced | Uploading | Failed(String)
}
```

Không lưu file ra đĩa. Đóng Warp mất bảng này, file vẫn nằm trong mirror và vẫn upload được bằng
Warp Sync như cũ (E4). Lý do (quyết định E1): giống attach của Bridge, gắn file với **session đang
sống**; không muốn một Save sau khi mở lại Warp tự upload qua session khác.

### 3.2 Luồng E1

1. Người dùng chạy palette **"Warp Sync: Edit remote file…"** ở pane có session Warpify remote.
   Prompt đường dẫn (dùng lại `path_prompt.rs`, gợi ý = `pwd` của block cuối). Đường dẫn tương đối
   → nối với `pwd`.
2. `WarpSyncModel::start_download(session, remote_path, Requester::RemoteEdit { window_id })`.
   Probe cho biết là thư mục → dừng, toast "Edit in Warp opens files; use Warp Sync: Download for
   folders". Lớn hơn giới hạn download → lỗi như Warp Sync hiện nay.
3. Download xong: đăng ký vào `RemoteEditModel`, mở `mirror_path` bằng `OpenFileInWarp` với layout
   theo setting `open_file_layout` (split mặc định của Warp).
4. Editor hiện dòng thông tin ở footer: `root@draff3:/etc/nginx/nginx.conf · Save uploads to the
   server` (hoặc `· Unsynced changes` / `· Uploading…` / `· Upload failed: …`).
5. `FileSaved { auto_saved: false }` của file có trong bảng → `start_upload(session, remote_path,
   requester)` → hộp thoại upload có **diff** (3.4) → Upload → `record_upload` commit Git → toast
   "Uploaded to root@draff3:/etc/nginx/nginx.conf". Cancel → trạng thái `Unsynced`, file mirror giữ
   nguyên.
6. `FileSaved { auto_saved: true }` → chỉ ghi mirror, trạng thái `Unsynced`, **không** upload (quyết
   định E2: auto-save của Warp không được phép đẩy file lên server root).
7. Session không còn (đóng tab, thoát `sudo -i`, rớt SSH) → Save báo lỗi rõ ở footer + toast, file
   mirror giữ nguyên; mở lại session và chạy Edit lại cùng đường dẫn thì Warp Sync phát hiện thay đổi
   local chưa upload (luồng "Overwrite local changes?" đã có) — không mất dữ liệu.

### 3.3 Luồng E2: đường dẫn trong output remote

- Chỉ khi flag bật và block thuộc session `WarpifiedRemote`. Giữ nguyên nhánh local.
- Nhận diện **theo cú pháp**, không hỏi server: dùng lại `possible_file_paths_at_point`, nhận ứng
  viên là đường dẫn tuyệt đối (`/…`) hoặc tương đối nối với `block.pwd()` (là đường dẫn trên
  server). Loại: URL, chuỗi có ký tự điều khiển, `..` sau khi chuẩn hoá vượt quá `/`.
- Hover chỉ gạch chân + tooltip "Edit on root@draff3". **Không chạy lệnh nào lúc hover** (quyết
  định E3): mỗi lệnh chạy qua PTY của người dùng; hover mà chạy `stat` sẽ chen vào shell đang dùng.
- Cmd/Ctrl-click → luồng E1 từ bước 2. File không tồn tại/là thư mục → toast lỗi từ probe (bước 2).
- Menu chuột phải trên link: "Edit in Warp" và "Download with Warp Sync" (thư mục dùng mục sau).

### 3.4 Diff trong hộp thoại upload

Chỉ cho upload **một file** mà baseline Git có bản trước: unified diff (`similar`) giữa bản
baseline (commit download gần nhất) và bản mirror, tối đa 40 dòng như preview của O2 (P11), phần
dư ghi "… N more lines". File nhị phân/không phải UTF-8 → chỉ tóm tắt như cũ. Upload thư mục giữ
nguyên hộp thoại hiện có.

### 3.5 E3: tô màu

1. Bật feature `arborium`: `lang-nginx`, `lang-ini`, `lang-ssh-config`, `lang-diff`, `lang-perl`,
   `lang-awk`, `lang-jinja2`, `lang-caddy`. Mỗi ngôn ngữ thêm `crates/languages/grammars/<tên>/`
   `config.yaml` (+ `indents.scm` nếu có nguồn Helix/Zed), thêm vào `SUPPORTED_LANGUAGES`.
2. Nhận diện theo thứ tự (quyết định E4): **ánh xạ người dùng** → tên file (`nginx.conf`,
   `sshd_config`, `ssh_config`, `Caddyfile`, `.env` → shell) → đoạn đường dẫn (`/etc/nginx/`,
   `/etc/ssh/sshd_config.d/`, `/etc/systemd/`) → đuôi (`.service` `.timer` `.socket` `.mount`
   `.target` `.path` `.ini` `.cnf` → ini; `.j2` → jinja2; `.pl` `.pm` → perl; `.awk` → awk;
   `.diff` `.patch` → diff) → **shebang** dòng đầu (`#!/bin/bash`, `#!/usr/bin/env python3`…)
   cho file không đuôi. Không tự đoán `.conf` chung chung (nginx, apache, haproxy khác nhau) —
   `.conf` chỉ thành nginx khi nằm dưới `/etc/nginx/`.
3. Setting mới `code.language_overrides` (danh sách `{ glob, language }`), glob khớp với **đường
   dẫn trên server** cho file mở bằng E1/E2 và đường dẫn local cho file khác; `language` phải là
   một tên trong `SUPPORTED_LANGUAGES`, sai thì bỏ qua + cảnh báo một lần. Ví dụ:
   `{ glob = "/etc/haproxy/*.cfg", language = "ini" }`.
4. Màu lấy theo theme Warp như các ngôn ngữ hiện có — không thêm gì.

**Về "syntax server công khai" (AO15):** không làm. Tô màu tree-sitter chạy cục bộ trong binary,
không cần mạng; gửi nội dung `/etc/...` ra dịch vụ bên ngoài là lộ mật khẩu/khoá. LSP (Warp có
rust-analyzer, gopls, pyright, typescript, clangd) là chẩn đoán/gợi ý, chạy local, không phục vụ
file cấu hình. Thứ người dùng cần "đổi được trong cấu hình" là ánh xạ ngôn ngữ (bước 3).

---

## 4. Các phase và task

Thứ tự: 1 → ⛔ CHECKPOINT EA → 2 → ⛔ CHECKPOINT EB → 3 → ⛔ CHECKPOINT EC.

### Phase 1 — Luồng Edit in Warp (E1)

- **1.1** Flag `FeatureFlag::WarpSyncRemoteEdit` (runtime, bật trong `DOGFOOD_FLAGS` giống `WarpSync`,
  kiểm cả `WarpSync` lẫn flag mới). Đọc trước: `specs/warp-sync/IMPLEMENTATION_PLAN.md` (mục
  requester, pending, confirm), `app/src/warp_sync/model.rs` toàn bộ.
- **1.2** `remote_edit.rs`: `RemoteEditModel` + `RemoteEditFile` + `RemoteEditState`; hàm thuần
  `resolve_remote_path(input, pwd) -> Result<String, _>` (tuyệt đối/tương đối, cấm `\0`, chuẩn hoá
  `.`/`..`). Test: `remote_edit_tests.rs`.
- **1.3** `Requester::RemoteEdit { window_id }` (hoặc biến thể tương đương theo cách `Requester` đang
  tổ chức): khi download xong trả `mirror_path` về Workspace thay vì chỉ announce. Probe là thư mục →
  lỗi riêng `WarpSyncError::NotAFile` (thông điệp ở 3.2 bước 2).
- **1.4** Workspace: action `WarpSyncEditRemoteFile`, palette "Warp Sync: Edit remote file…", prompt
  đường dẫn; nhận kết quả → đăng ký model →
  `open_file_notebook(LocalOrRemotePath::Local(mirror_path), Some(session), layout, None, ctx)`
  (`workspace/view.rs:8945`).
- **1.5** Bắt `LocalCodeEditorEvent::FileSaved` (tìm chỗ Workspace/pane đã subscribe editor; nếu không
  có chỗ chung thì subscribe khi mở ở 1.4): `auto_saved == false` và file trong bảng → `start_upload`;
  `auto_saved == true` → `Unsynced`. Upload xong/lỗi/cancel → cập nhật state.
- **1.6** Footer editor hiện `user@host:path · <trạng thái>` cho file trong bảng
  (`app/src/code/footer.rs`).
- **1.7** Diff trong hộp thoại upload (3.4): hàm thuần `single_file_diff(old, new, max_lines)` +
  test; nối vào `upload_body` khi upload là một file có baseline.
- **1.8** Review theo mục 0 (lock, unwrap, match), test, clippy, format. Ghi nhật ký.

**⛔ CHECKPOINT EA** — checklist 5.EA.

### Phase 2 — Bấm đường dẫn trong output SSH (E2)

- **2.1** Đọc toàn bộ `link_detection.rs`, `grid_handler::PossiblePath`, nhánh `local_fs`. Ghi lại
  (nhật ký) cách link local được highlight/invalidate để nhánh remote đi đúng vòng đời đó.
- **2.2** Nhánh remote trong `scan_for_file_path`: khi flag bật và block là `WarpifiedRemote`, dùng
  `block.pwd()` và hàm thuần `remote_link_candidate(possible_path, pwd) -> Option<String>` (không I/O,
  test kỹ: `ls` output, `nginx: [emerg] ... in /etc/nginx/conf.d/a.conf:12`, đường dẫn có `:line`,
  URL, đường dẫn có dấu câu cuối). Kết quả là biến thể mới `GridHighlightedLink::RemoteFile` (không
  trộn vào `File` vốn chứa `PathBuf` local).
- **2.3** Tooltip "Edit on user@host"; Cmd/Ctrl-click → luồng 1.4 với đường dẫn đó (có số dòng →
  mở editor tại dòng đó nếu `open_file_notebook` hỗ trợ, không thì bỏ qua số dòng).
- **2.4** Menu chuột phải: "Edit in Warp" / "Download with Warp Sync".
- **2.5** Review, test, clippy, format, nhật ký.

**⛔ CHECKPOINT EB** — checklist 5.EB.

### Phase 3 — Tô màu file cấu hình (E3)

- **3.1** Bật 8 feature `arborium` (3.5 bước 1). Với từng ngôn ngữ: xác nhận crate có query highlight
  (README grammars: dùng query của `arborium`, không tự viết `highlights.scm`); ngôn ngữ nào crate
  thiếu query → bỏ, ghi quyết định. Thêm `config.yaml` (comment prefix, indent), `indents.scm` khi
  có nguồn Helix/Zed. Test `crates/languages/src/lib_tests.rs`: mỗi ngôn ngữ mới parse được một mẫu.
- **3.2** Nhận diện theo tên file/đoạn đường dẫn/đuôi/shebang (3.5 bước 2) — hàm thuần, test bảng.
  Shebang cần nội dung dòng đầu: thêm đường vào dùng buffer đã load, không đọc file lần hai.
- **3.3** Setting `code.language_overrides` + đọc khi chọn ngôn ngữ; file từ E1/E2 dùng
  `remote_path` của `RemoteEditModel` để khớp glob. Glob tự viết `*`/`?`/`**` hoặc dùng lại glob của
  O2 (`agent_bridge/policy.rs`) nếu tách được thành hàm dùng chung.
- **3.4** Review, test, clippy (`-p languages` nữa), format, nhật ký.

**⛔ CHECKPOINT EC** — checklist 5.EC. Sau đó tick E trong roadmap, chuyển sang viết
`G_GATEWAY_PLAN.md` (G1).

---

## 5. Checklist test tay (người dùng)

Môi trường: VM lab có sshd như các checkpoint trước; tab SSH → `sudo -i` → Warpify. Build lại có
flag mới.

**5.EA — Edit in Warp**

1. Palette "Warp Sync: Edit remote file…" → `/etc/nginx/nginx.conf` → file mở trong editor Warp,
   footer `root@<host>:/etc/nginx/nginx.conf · Save uploads to the server`.
2. Sửa một comment → Cmd/Ctrl-S → hộp thoại "Upload to root@<host>?" có diff đúng dòng vừa sửa →
   Upload → toast thành công; trên server `cat` thấy thay đổi, có file backup; `git log` trong thư
   mục mirror có commit mới.
3. Sửa tiếp → Save → Cancel → footer "Unsynced changes"; Save lại → hộp thoại hiện lại.
4. Nhập đường dẫn tương đối (`nginx.conf` khi `pwd` là `/etc/nginx`) → mở đúng file.
5. Nhập một thư mục → toast "Edit in Warp opens files…", không mở gì.
6. Để auto-save chạy (đổi focus sau khi sửa) → không có hộp thoại upload, footer "Unsynced changes".
7. Mở file, sửa trên server bằng `echo '# x' >> /etc/nginx/nginx.conf`, rồi Save trong Warp → hộp
   thoại cảnh báo server đã đổi (luồng `remote_check` sẵn có), Cancel được.
8. Đóng tab SSH rồi Save → lỗi rõ ở footer + toast, file mirror còn nguyên.

**5.EB — Bấm đường dẫn**

1. `ls /etc/nginx` rồi Cmd/Ctrl-click `nginx.conf` → mở như 5.EA.1.
2. `nginx -t` với lỗi cố ý → Cmd/Ctrl-click đường dẫn trong thông báo lỗi `... in /etc/nginx/...:12`
   → mở đúng file (tại dòng 12 nếu hỗ trợ).
3. Hover đường dẫn → **không** có lệnh lạ nào xuất hiện trong block/history của shell.
4. Cmd/Ctrl-click một đường dẫn không tồn tại → toast lỗi, không mở gì.
5. Session local (không SSH) → nhận diện đường dẫn như trước, không đổi hành vi.

**5.EC — Tô màu**

1. `nginx.conf`, một `*.service`, `sshd_config`, `/etc/php/*/php.ini`, một script không đuôi có
   `#!/bin/bash` → đều có màu.
2. `/etc/haproxy/haproxy.cfg` không màu; thêm `language_overrides` `{ glob = "/etc/haproxy/*.cfg",
   language = "ini" }` → mở lại có màu.
3. Ghi `language = "khong-co"` → không crash, có một cảnh báo.

---

## 6. Rủi ro và để sau

| Rủi ro | Giảm thiểu / chấp nhận |
|---|---|
| Save nhầm lên server root | Luôn có hộp thoại + diff (không auto-upload); backup trên server; commit Git để rollback |
| Auto-save đẩy file dở dang | Auto-save không bao giờ upload (E2) |
| Upload qua sai server khi hostname trùng | Warp Sync đã kiểm `host_key` + machine-id; E không đổi chỗ đó |
| Nhận diện link sai ở remote (không kiểm tồn tại) | Chỉ kiểm lúc click; lỗi thì toast, không có tác dụng phụ |
| Mirror giữ bản sao file nhạy cảm (`/etc/shadow`…) | Như Warp Sync hiện nay (thư mục 0700); G4 sẽ thêm loại trừ path |
| File lớn/nhị phân | Giới hạn download của Warp Sync; nhị phân mở theo `FileTarget` hiện có |

**Để sau:** "Reload from server" khi server đổi; mở nhiều file/thư mục cùng lúc; danh sách "file
đang sửa từ xa"; bỏ hộp thoại cho host lab (theo policy O2); LSP; G4 dùng lại `RemoteEditModel` cho
agent (đi qua duyệt O2).

---

## 7. Tiến độ, quyết định, nhật ký

### Tiến độ

- [x] Plan v1 (2026-09-29)
- [ ] 1.1 flag · [ ] 1.2 model · [ ] 1.3 requester · [ ] 1.4 palette/mở editor · [ ] 1.5 Save → upload · [ ] 1.6 footer · [ ] 1.7 diff · [ ] 1.8 review + clippy + format
- [ ] ⛔ CHECKPOINT EA
- [ ] 2.1 đọc · [ ] 2.2 nhận diện remote · [ ] 2.3 click · [ ] 2.4 menu · [ ] 2.5 review + clippy + format
- [ ] ⛔ CHECKPOINT EB
- [ ] 3.1 grammar · [ ] 3.2 nhận diện · [ ] 3.3 `language_overrides` · [ ] 3.4 review + clippy + format
- [ ] ⛔ CHECKPOINT EC · [ ] tick E trong roadmap

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| E1 | 2026-09-29 | Bảng file đang sửa từ xa chỉ nằm trong RAM, gắn với session đã download | Không để Save sau khi mở lại Warp tự upload qua session khác; file vẫn ở mirror, upload tay được |
| E2 | 2026-09-29 | Chỉ Save chủ động (Cmd/Ctrl-S) mới upload; auto-save chỉ ghi mirror | Không đẩy file dở dang lên server |
| E3 | 2026-09-29 | Link remote nhận diện theo cú pháp, không chạy lệnh lúc hover; kiểm tồn tại lúc click (probe của download) | Lệnh đi qua PTY của người dùng; hover chạy lệnh sẽ chen vào shell |
| E4 | 2026-09-29 | Thứ tự chọn ngôn ngữ: ánh xạ người dùng → tên file → đoạn đường dẫn → đuôi → shebang; `.conf` chung chung không tự đoán | `.conf` dùng cho nhiều định dạng khác nhau; đoán sai tệ hơn không màu |
| E5 | 2026-09-29 | Hộp thoại upload luôn hiện (kèm diff cho file đơn), kể cả khi mở từ E1/E2 | Ghi lên server root; diff giúp duyệt nhanh nên một cú bấm là chấp nhận được |
| E6 | 2026-09-29 | Không dùng dịch vụ tô màu/"syntax server" bên ngoài (AO15) | Tô màu đã cục bộ; gửi file `/etc` ra ngoài là lộ bí mật |
| E7 | 2026-09-29 | E dựa trên Warp Sync (in-band qua PTY đã `sudo -i`), không dùng `remote_server` của upstream | `remote_server` chạy dưới user đăng nhập SSH (không sửa được file root sau `sudo -i` có mật khẩu), phải cài binary lên server, gắn xác thực Warp |

### Nhật ký

- 2026-09-29 — Plan v1 (Claude Opus 5.5) sau khảo sát code: Warp Sync hỗ trợ file đơn + baseline
  Git; editor code của Warp có `FileSaved { auto_saved }`; tô màu tree-sitter qua `arborium` (35
  ngôn ngữ, crate có sẵn nginx/ini/ssh-config…); nhận diện đường dẫn đang tắt cho block remote
  (`link_detection.rs:581`); upstream có `remote_server` (cài binary, chạy dưới user SSH) — không
  dùng (E7). Chưa code.
