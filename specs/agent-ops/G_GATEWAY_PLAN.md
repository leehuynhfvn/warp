# G1 — Danh bạ server — Implementation plan (v1)

> Viết ngày 2026-09-29 (Claude Sonnet 5.5), theo `ROADMAP.md` mục G và quyết định AO9–AO13. Các mục 0–6
> chi tiết **G1** (xong); mục **G2** (trước mục 7) chi tiết G2a/G2b, viết 2026-09-30; G3–G5 giữ ở mức
> phác trong roadmap, sẽ viết thêm vào file này khi tới lượt. Người dùng giao Claude quyết định thiết
> kế (AO13); người dùng chỉ làm các ⛔ CHECKPOINT test tay.

---

## 0. Quy tắc làm việc

1. Đọc `AGENTS.md` trước. Làm tuần tự theo task. Tới ⛔ CHECKPOINT thì **dừng**, ghi nhật ký, chờ
   người dùng test tay, không tự qua phase sau.
2. Validation theo "Implementation Validation Order" của `AGENTS.md`: `cargo check`/`cargo nextest`
   nhỏ nhất trong lúc làm; cuối mỗi phase: test liên quan → `cargo clippy -p warp --features … --all-targets
   --tests -- -D warnings` → `./script/format` một lần.
3. Mọi quyết định mới ghi vào mục 7 (bảng `GD#`), mọi task xong ghi nhật ký (file đổi, lệnh test đã
   chạy, kết quả).
4. Match exhaustive, không `unwrap`/`expect` trên dữ liệu từ file, output `ssh` hay người dùng nhập.
5. **Không bao giờ sửa byte nào của `~/.ssh/config` do người dùng viết**, trừ đúng một dòng `Include`
   (GD4) và chỉ sau khi hỏi + backup. Không bao giờ chạy lệnh trên server trong G1 (chỉ `ssh` mở tab).
6. Không đổi hành vi Warp Sync / Agent Bridge hiện có; G1 chỉ **thêm** (một hook ghi `mirror_key`, một
   MCP tool đọc).

---

## 1. Mục tiêu và phạm vi

**Sau G1:** người dùng khai báo server một lần trong Warp (hoặc để Warp tự nhập từ `~/.ssh/config`),
gắn tag, mở session bằng "Connect to server…", và mỗi host biết mirror Warp Sync của nó nằm đâu. Agent
gọi được `list_hosts` để biết server nào có, tag gì, mirror ở đâu (chưa mở được session — đó là G2).

**Trong phạm vi:** G1a kho `hosts.toml`; G1b nhập từ ssh config; G1c ghi ngược `warp.conf` + tag;
quick connect; G1d nối Warp Sync + `list_hosts`; Settings page; palette.

**Ngoài phạm vi (để sau):** mật khẩu SSH/sudo trong secure storage + `SSH_ASKPASS` + tự điền `sudo`
(GD2 — làm ở phase 5 tùy chọn hoặc cùng G2); agent tự mở session (G2); kênh exec trực tiếp (G3);
sửa file qua mirror bởi agent (G4); dòng thời gian (G5); đồng bộ danh bạ giữa nhiều máy.

---

## 2. Hiện trạng code (khảo sát 2026-09-29)

| Có sẵn | Dùng cho | Vị trí |
|---|---|---|
| Mẫu kho TOML nhỏ có `parse`/`serialize` thuần + `load`/`add` ghi đĩa dưới `~/.warp/agent-ops/` | Khuôn cho `hosts.toml` | `app/src/agent_bridge/pairing.rs`, hằng `POLICY_FILE`/`AGENTS_FILE` ở `agent_bridge/mod.rs` |
| `create_private_dir_all` (thư mục 0700) | Tạo `~/.warp/agent-ops`, `~/.ssh/config.d` | `app/src/warp_sync/paths.rs` |
| Tab config: `PaneTemplateType::PaneTemplate { cwd, commands: Vec<CommandTemplate{exec}>, … }`, `Workspace::add_tab_with_pane_layout(PanesLayout::Template(..))` | Quick connect mở tab chạy `ssh <alias>` | `app/src/launch_configs/launch_config.rs:365`, `app/src/workspace/view.rs:13104` |
| Warpify SSH tự bắt lệnh `ssh …` gõ trong shell; `subshell_info().ssh_connection_info: InteractiveSshCommand { host, port }` giữ **alias người dùng gõ** | Nối session ↔ alias (G1d) | `app/src/terminal/ssh/util.rs`, `terminal/view.rs:27077` |
| Manifest Warp Sync lưu `host_key` + `machine_id` (`resolve_host_key`, `machine_host_key`) | `mirror_key`/`machine_id` của host (G1d) | `app/src/warp_sync/manifest.rs`, `transfer.rs:715`, `paths.rs:271` |
| `WarpSyncSettingsPageView`, `SettingsSection::WarpSync`, gate bằng `FeatureFlag` | Khuôn cho trang Settings "Servers" | `app/src/settings_view/warp_sync_page.rs`, `mod.rs:329,1335,1476` |
| Palette: `EditableBinding` + `WorkspaceAction::*` sau flag | "Agent Ops: Connect to server…", "Add server…" | `app/src/workspace/mod.rs:422,474`, `action.rs:501` |
| MCP tool + `ActionKind` catalog (`remote.session.list` …) | `list_hosts` | `crates/local_control/src/catalog.rs:320`, `crates/warp_cli/src/local_control/mcp/tools.rs` |
| OpenSSH 10.2 local: `ssh -G <alias>` in cấu hình đã giải `Include`/`Match`/wildcard | Giá trị kết nối luôn lấy từ đây (AO10) | máy dev |

**`~/.ssh/config` thật của người dùng** (mẫu để test parser): 179 dòng `Host`, 10+ dòng `Include ~/.ssh/config.*`,
dòng `Host` gộp nhiều alias lẫn wildcard (`Host web01 web02 … *.ty8 *.tyo`). Chưa có `~/.ssh/config.d/`.

**Không có:** kho host; parser ssh config; chỗ chứa metadata (tag, cách lên root) theo host; picker
danh sách tìm được (cần tìm khuôn dùng lại ở 2.1).

---

## 3. Thiết kế

### 3.1 Module

`app/src/host_directory/` (tách khỏi `agent_bridge/` vì dùng cho cả người và agent):

| File | Nội dung |
|---|---|
| `mod.rs` | hằng (`HOSTS_FILE = ".warp/agent-ops/hosts.toml"`, `WARP_CONF = ".ssh/config.d/warp.conf"`, giới hạn), re-export |
| `model.rs` | kiểu `Host`, `HostSource`, `AuthMethod`, `RootLogin`, `Transport`; `validate_alias`; hàm thuần `merge_discovered` |
| `store.rs` | `parse`/`serialize` thuần; `load`/`save` (atomic, `0600`) |
| `ssh_config.rs` | parser **khám phá alias** (không giải giá trị): `Host`, `Include` |
| `ssh_resolve.rs` | `parse_ssh_g(output)` thuần + chạy `ssh -G` nền có timeout |
| `warp_conf.rs` | dựng/ghi khối host vào `warp.conf` + tag comment + `Include` (phase 3) |
| `directory.rs` | `HostDirectoryModel` (singleton entity): giữ danh sách trong RAM, làm mới, sự kiện |

Flag `FeatureFlag::AgentOpsHosts` (runtime, `DOGFOOD_FLAGS`) + cargo feature `agent_ops_hosts`, cùng
mẫu `WarpSyncRemoteEdit` (`crates/warp_features/src/lib.rs:1011`, `app/Cargo.toml:882-885`).

### 3.2 `hosts.toml` (G1a)

```toml
version = 1

[[hosts]]
alias = "web01"
source = "ssh_config"        # ssh_config | warp
tags = ["prod", "web"]
auth = "key"                 # key | agent | password | unknown   (chỉ là ghi chú ở G1, GD2)
root_login = "sudo_nopasswd" # root_login | sudo_nopasswd | sudo_password | none
requiretty = false
transport = "in_band"        # in_band | direct   (G3 dùng; G1 chỉ lưu)
missing = false              # alias biến mất khỏi ssh config
mirror_key = "draff3"        # G1d
machine_id = "…"             # G1d
```

- `alias` khớp `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$` (`validate_alias`). Lý do: alias đi vào lệnh
  `ssh <alias>` gõ trong shell của người dùng; cấm khoảng trắng, `-` đầu (option injection), metachar,
  nên **không cần quote** và không thể chèn lệnh (GD5). Host trong ssh config có alias không hợp lệ bị bỏ qua
  (log một lần).
- Không chứa: HostName/User/Port/ProxyJump/IdentityFile (AO10), không chứa bí mật (GD2).
- `deny_unknown_fields`; file hỏng → lỗi rõ + **không ghi đè** (khác `agents.toml`, vì đây là dữ liệu
  người dùng nhập tay: tag, root_login). File hỏng: giữ nguyên, toast, danh bạ rỗng trong RAM.
- Ghi atomic (tmp cùng thư mục + rename), quyền `0600`, thư mục `0700`.

### 3.3 Nhập từ ssh config (G1b)

- `discover_aliases(ssh_config_path, home) -> Discovery { aliases: Vec<Discovered>, warnings }`:
  1. Đọc `~/.ssh/config`; token hoá từng dòng theo cú pháp OpenSSH (khoảng trắng hoặc `=`, `"…"`, comment `#`).
  2. `Host a b c`: mỗi token là một alias; **bỏ** token có `*`, `?`, `!` (pattern/phủ định). `Host *` không cho gì.
  3. `Match …`: không sinh alias (chỉ chứa option).
  4. `Include <pattern>…`: mở rộng `~`, đường dẫn tương đối tính từ `~/.ssh` (đúng OpenSSH), glob, đọc từng
     file đã sắp xếp; tối đa độ sâu 16, tối đa 256 file, mỗi file ≤ 1 MiB; chống vòng (tập đường dẫn đã canonicalize).
  5. Ghi nhận `(alias, file, line)` để hiển thị nguồn.
- `merge_discovered(existing, discovered) -> (Vec<Host>, MergeReport { added, missing, restored })`:
  alias mới → thêm `source = ssh_config`, tag rỗng; alias đã có mà vắng mặt → `missing = true`, giữ metadata;
  xuất hiện lại → `missing = false`. Host `source = warp` không bao giờ bị đánh `missing` bởi bước này.
- Khi nào chạy: lúc khởi động (nền), và khi mở picker/Settings. **Không** dùng file watcher ở G1 (GD6): mtime
  của `~/.ssh/config` + các file `Include` đã đọc được so lại khi mở picker, đủ rẻ.
- Toast "Found N new SSH hosts" chỉ khi N > 0 **và** danh bạ đã từng có dữ liệu (lần import đầu với 179 host
  chỉ hiện một toast "Imported 179 SSH hosts", không spam).

### 3.4 Giá trị kết nối (AO10)

`resolve(alias) -> ResolvedHost { hostname, user, port, proxy_jump, identity_files, … }` bằng
`ssh -G -- <alias>`, chạy nền, timeout 5 s, chỉ khi hiển thị chi tiết/Settings (không chạy hàng loạt lúc
import). Cảnh báo (ghi ở GD7): `ssh -G` **thực thi** `Match exec` trong config của chính người dùng; G1 chỉ
gọi cho alias lấy từ config của họ hoặc tạo trong Warp và đã qua `validate_alias`.

### 3.5 Ghi ngược `warp.conf` (G1c)

Chỉ cho host `source = warp`. `~/.ssh/config.d/warp.conf` (`0600`), mỗi host một khối:

```
# warp:tags=prod,project-x
Host web-new
    HostName 203.0.113.10
    User ops
    Port 22
    IdentityFile ~/.ssh/id_ed25519
```

- Ghi atomic; backup bản trước thành `warp.conf.bak`; sau khi ghi chạy `ssh -G -- <alias>` và so `hostname`,
  `user`, `port` với ý định; lệch/lỗi → khôi phục backup.
- `Include config.d/warp.conf` thêm **một lần** vào **đầu** `~/.ssh/config` (Include phải đứng trước mọi
  `Host`/`Match` để không bị nuốt vào khối), sau khi hiện hộp thoại nói rõ file nào sẽ đổi, có backup
  `config.warp-backup-<timestamp>`. Đã có dòng (so khớp chuẩn hoá) thì không thêm. Người dùng từ chối →
  host vẫn lưu trong `warp.conf` nhưng `ssh <alias>` ngoài Warp chưa chạy (cảnh báo hiển thị).
- Không sửa khối người dùng tự viết. "Move to Warp": chuyển khối (nguyên văn các dòng của khối) sang
  `warp.conf`, xoá khỏi file gốc — **chỉ** khi người dùng xác nhận, có backup; ngoài phạm vi phase 3 nếu
  chưa cần (GD8: làm sau, nếu người dùng yêu cầu).
- Tag host `ssh_config` nằm trong `hosts.toml`; tag host `warp` nằm cả trong comment `# warp:tags=` (nguồn
  sự thật là comment; `hosts.toml` chỉ cache) để `grep` được. Sửa tag = ghi lại comment.

### 3.6 Quick connect

Palette "Agent Ops: Connect to server…" → picker tìm theo alias/tag → mở tab bằng
`PaneTemplateType::PaneTemplate { commands: [CommandTemplate { exec: "ssh <alias>" }] }`. Warpify SSH của
Warp tự bắt lệnh. Người dùng vẫn tự gõ passphrase/mật khẩu trong terminal (G1 không lưu bí mật).
"Reconnect": chạy lại `ssh <alias>` trong tab đã rớt. Lên root (`sudo_nopasswd`): sau khi session Warpify xong
gửi `sudo -i` — **chỉ** nếu tìm được sự kiện "session remote đã bootstrap" đáng tin (task 2.4); nếu không thì
G1 không tự lên root, chỉ hiện gợi ý.

### 3.7 Nối Warp Sync + `list_hosts` (G1d)

- Nối session ↔ alias: `ssh_connection_info.host` của session (alias người dùng gõ; quick connect cũng qua
  đường này). Khi một download/upload Warp Sync xong trên session đó và alias có trong kho → ghi
  `mirror_key` (host_key đã giải) + `machine_id` vào `hosts.toml`. Lần sau `machine_id` của probe khác bản đã
  lưu → toast cảnh báo (server cài lại / alias trỏ máy khác); Warp Sync đã tự tách thư mục theo machine-id
  (`resolve_host_key`), G1 chỉ **báo**, không sửa Warp Sync.
- `remote.host.list` (`ActionKind::RemoteHostList`, target Instance, không cần attach, cùng quyền với
  `remote.session.list`) + MCP tool `list_hosts`: alias, tags, `source`, `user@hostname:port` (từ `ssh -G`
  đã cache), `root_login`, `transport`, session đang mở/đã attach (khớp bằng `ssh_connection_info.host`),
  `mirror_dir`, danh sách path đã sync (từ manifest, tối đa 200). **Không** trả bí mật (G1 chưa có).
  Cập nhật `INSTRUCTIONS` của MCP một dòng: agent sửa file của server qua mirror là việc của G4, hiện tại
  chỉ *đọc* mirror.

---

## 4. Các phase và task

Thứ tự: 1 → ⛔ CHECKPOINT HA → 2 → ⛔ CHECKPOINT HB → 3 → ⛔ CHECKPOINT HC → 4 → ⛔ CHECKPOINT HD →
(5 tùy chọn).

### Phase 1 — Kho host + nhập ssh config (G1a, G1b)

- **1.1** Flag `FeatureFlag::AgentOpsHosts` + cargo feature `agent_ops_hosts` + module rỗng
  `host_directory` gate bằng flag runtime (mẫu `remote_edit::is_enabled()`).
- **1.2** `model.rs`: kiểu dữ liệu, `validate_alias`, `merge_discovered`. Test `model_tests.rs` (alias hợp lệ/
  không hợp lệ: `-oProxyCommand=x`, `a b`, `a;b`, rỗng, 65 ký tự, unicode; merge: thêm/missing/restored/host
  `warp` không bị missing).
- **1.3** `store.rs`: `parse`/`serialize`/`load`/`save`. Test: round-trip, `deny_unknown_fields`, version lạ,
  file hỏng không bị ghi đè, quyền `0600` (Unix), atomic (không để lại file tạm khi lỗi).
- **1.4** `ssh_config.rs`: `discover_aliases`. Test bảng: `Host a b c`, dấu `=`, nháy kép, comment cuối dòng,
  wildcard/`!`, `Match`, `Include` tương đối/`~`/glob/không tồn tại, vòng Include, độ sâu, file lớn, dòng CRLF,
  `Host` không alias hợp lệ. Thêm test dùng **bản sao ẩn danh** của cấu trúc config thật (Include chồng nhau,
  dòng Host gộp 60 alias).
- **1.5** `directory.rs`: `HostDirectoryModel` singleton; khởi động nền: load → discover → merge → save (chỉ khi
  đổi) → sự kiện `Changed`; toast theo 3.3; palette "Agent Ops: Import hosts from SSH config" chạy lại tay và
  toast báo `added/missing`.
- **1.6** Review theo mục 0, test, clippy, format, nhật ký.

**⛔ CHECKPOINT HA** — checklist 5.HA.

### Phase 2 — Quick connect (G1 quick connect)

- **2.1** Tìm khuôn picker tìm-được có sẵn (`tab_configs/repo_picker.rs`, palette, `path_prompt.rs`); ghi vào nhật
  ký cái nào dùng lại được. Nếu không có picker phù hợp: một modal nhỏ (editor tìm + danh sách) theo khuôn
  `path_prompt`/`confirm_dialog`.
- **2.2** `WorkspaceAction::AgentOpsConnectToServer`, palette "Agent Ops: Connect to server…", lọc theo alias/tag
  (khớp mờ, tag `tag:prod`), phím Enter mở tab.
- **2.3** Mở tab `ssh <alias>` (3.6); tên tab = alias; alias chỉ đến từ kho đã `validate_alias` (kiểm lại lúc mở).
  "Reconnect" hoãn (GD13).
- **2.4** Lên root: khảo sát sự kiện bootstrap session remote; nếu có → `sudo -i` cho `sudo_nopasswd`/
  `root_login = login` (theo bảng 3.2); nếu không → bỏ, ghi lý do vào nhật ký. Test bằng hàm thuần quyết định
  "có gõ gì tiếp không".
- **2.5** Review, test, clippy, format, nhật ký.

**⛔ CHECKPOINT HB** — checklist 5.HB.

### Phase 3 — Settings "Servers" + tag + tạo host (G1c)

- **3.1** `SettingsSection::Servers` + trang (khuôn `warp_sync_page.rs`): bảng host (alias, nguồn, `user@host`
  từ `ssh -G` nền, tag, cách lên root, mirror, trạng thái `missing`); ô tìm; sửa tag/`root_login`/`transport` ghi
  `hosts.toml`. Chỉ hiện khi `AgentOpsHosts` bật.
- **3.2** `warp_conf.rs`: `render_block(host_input) -> String`, `parse_blocks(text)` (chỉ đọc file của Warp),
  `set_tags`, `remove_block`, validate từng trường (HostName không chứa khoảng trắng/`;`/quote, Port 1–65535,
  IdentityFile không chứa `\n`; **không** cho ProxyCommand/LocalCommand/Match exec — chỉ ProxyJump, GD9).
  Test dày, kể cả injection (`HostName x\n ProxyCommand …`).
- **3.3** Ghi atomic + backup + xác minh bằng `ssh -G` + khôi phục khi lệch (3.5). Test trong thư mục tạm với
  `$HOME` giả (mẫu `pairing_tests.rs`); phần chạy `ssh` đi qua trait để test dùng bản giả.
- **3.4** Thêm `Include config.d/warp.conf` vào đầu `~/.ssh/config`: hộp thoại xác nhận (`confirm_dialog`), backup,
  idempotent, không đổi byte nào khác (test so sánh nội dung trước/sau, kể cả file không kết thúc bằng newline
  và file CRLF).
- **3.5** Form "Add server" (Settings + palette "Agent Ops: Add server…"): alias, HostName, User, Port,
  ProxyJump, IdentityFile, tag, `root_login`. Tạo → `warp.conf` + `hosts.toml`.
- **3.6** Xoá host `warp` (xoá khối + mục kho, xác nhận); host `ssh_config` chỉ có "Forget" (xoá metadata Warp,
  không đụng file ssh).
- **3.7** Review, test, clippy, format, nhật ký.

**⛔ CHECKPOINT HC** — checklist 5.HC.

### Phase 4 — Nối Warp Sync + `list_hosts` (G1d)

- **4.1** Hook: sau download/upload Warp Sync thành công, nếu session có `ssh_connection_info.host` thuộc kho →
  `set_mirror(alias, mirror_key, machine_id)`. Kiểm khác `machine_id` → toast (3.7). Hàm thuần
  `mirror_link_update(existing, observed) -> Update {Set, Warn, None}` + test.
- **4.2** `ActionKind::RemoteHostList` + handler `local_control/handlers/hosts.rs` + kết quả
  `RemoteHostListResult`; cùng chính sách quyền với `remote.session.list`. Test handler.
- **4.3** MCP tool `list_hosts` (`warp_cli/.../tools.rs`, `format.rs`) + dòng trong `INSTRUCTIONS`; test
  `tools_tests.rs`. Chú ý protocol: thêm action mới → xem cách `remote.*` đã được version (D12 của Bridge).
- **4.4** Ghi vào `E`/Warp Sync: nút/palette "Open mirror of this server" cho host trong Settings (dùng lại
  `WarpSyncOpenMirror`).
- **4.5** Review, test, clippy (`-p warp -p local_control -p warp_cli`), format, nhật ký.

**⛔ CHECKPOINT HD** — checklist 5.HD. Sau đó tick G1 trong roadmap.

### Phase 5 — Bí mật (tùy chọn, GD2) — chỉ làm khi người dùng muốn

Mật khẩu SSH/sudo trong `secure_storage` (khoá `host:<alias>:ssh_password`, `…:sudo_password`), `SSH_ASKPASS`
helper `warp --warpctrl askpass`, tự điền sudo một lần theo AO8. Cần thiết kế riêng (prompt giả, lịch sử shell,
log). Không bắt đầu khi chưa có yêu cầu rõ ràng; nếu G2 bắt đầu thì làm ở đó.

---

## 5. Checklist test tay (người dùng)

Build: `./script/run --features warp_control_cli,warp_sync,agent_bridge,agent_ops_policy,warp_sync_remote_edit,agent_ops_hosts`.
Trước khi test, chạy `sha256sum ~/.ssh/config ~/.ssh/config.* > /tmp/ssh-before.sha` để so sau.

**5.HA — Kho + nhập**

1. Khởi động Warp lần đầu với flag → toast "Imported 454 SSH hosts" (parser đã chạy thử trên config thật,
   chỉ đọc: 454 alias cụ thể / 14 file, khớp kiểm chéo bằng script độc lập; không tính wildcard); `~/.warp/agent-ops/hosts.toml` tồn tại, quyền `-rw-------`.
2. `grep -c '^\[\[hosts\]\]' ~/.warp/agent-ops/hosts.toml` = 454; không có alias chứa `*`.
3. `sha256sum -c /tmp/ssh-before.sha` → mọi file ssh **không đổi**.
4. Thêm `Host test-new-1` vào một file config, chạy palette "Agent Ops: Import hosts from SSH config" → toast
   "Found 1 new SSH host".
5. Xoá host đó khỏi file, import lại → host đánh `missing = true` trong `hosts.toml`, không bị xoá.
6. Sửa tay `hosts.toml` thành TOML hỏng → khởi động lại: toast lỗi, file **không bị ghi đè**.

**5.HB — Quick connect**

1. Palette (Cmd/Ctrl-P) "Agent Ops: Connect to server…" → chip `servers` xuất hiện, danh sách hiện đủ host; gõ vài chữ alias
   → lọc mờ đúng, chữ khớp in đậm; host có tag hiện tag ở bên phải.
2. Sửa tay `hosts.toml`: thêm `tags = ["prod"]` cho vài host, mở lại picker; gõ `tag:prod` → chỉ host có tag; gõ `prod` (không
   tiền tố) → cũng ra host có tag đó, xếp sau host có alias khớp.
3. Enter trên một host → tab mới chạy `ssh <alias>`, tên tab = alias; Warpify SSH như khi gõ tay (hỏi/tự Warpify theo cài đặt).
4. Host đã `missing = true` (xoá khỏi ssh config) và alias có ký tự lạ tạo tay trong `hosts.toml` (`a;b`) → không hiện trong picker.
5. Sau khi thêm một `Host` mới vào ssh config: mở picker → host mới hiện ngay ở lần mở kế tiếp (picker tự quét lại nền), toast
   "Found 1 new SSH host".
6. Cuối cùng cho tôi biết: có muốn thử `ssh -t <alias> sudo -i` cho host `sudo_nopasswd` không (GD12).

**5.HC — Settings + ghi ngược**

1. Settings > Servers: thấy toàn bộ host, tìm được, cột `user@host` điền dần (không treo UI).
2. Đặt tag `prod` cho một host `ssh_config` → `hosts.toml` có tag; `~/.ssh/config` **không đổi**.
3. Add server (alias `warp-test`, HostName lab, User, Port, IdentityFile) → hộp thoại xin thêm `Include` →
   Đồng ý → `~/.ssh/config` chỉ đổi đúng một dòng `Include config.d/warp.conf` ở đầu (`diff` với bản backup),
   `warp.conf` có khối + `# warp:tags=…`, quyền `0600`.
4. Ngoài Warp: `ssh warp-test` và `ssh -G warp-test | head` đúng giá trị.
5. Nhập `HostName x\n ProxyCommand …` (dán nhiều dòng) → bị từ chối, không ghi gì.
6. Xoá host `warp-test` → khối biến khỏi `warp.conf`, mục biến khỏi `hosts.toml`; file ssh của người dùng
   không đổi thêm.
7. Từ chối `Include` → host vẫn lưu, cảnh báo "ssh <alias> ngoài Warp chưa chạy".

**5.HD — Nối Warp Sync + `list_hosts`**

1. Connect tới VM lab bằng quick connect → Warp Sync download một file → `hosts.toml` có `mirror_key` +
   `machine_id` của host đó.
2. Claude Code (MCP đã pair) gọi `list_hosts` → thấy alias, tag, `user@host`, session đang mở, thư mục mirror,
   path đã sync; không có trường nào trông như bí mật.
3. Đổi `/etc/machine-id` (hoặc alias trỏ máy khác) → kết nối/sync lần sau → toast cảnh báo, mirror cũ không
   bị dùng.
4. Hai alias cùng một máy (IP và jump) → cùng `mirror_key`.

---

## 6. Rủi ro và để sau

| Rủi ro | Giảm thiểu / chấp nhận |
|---|---|
| Alias độc (`-oProxyCommand=…`, `a;rm …`) đi vào `ssh <alias>` gõ trong shell | `validate_alias` chặt (GD5), kiểm lại lúc mở tab, không quote nên phải cấm hẳn |
| Ghi hỏng `~/.ssh/config` làm người dùng mất SSH | Chỉ thêm một dòng `Include` sau xác nhận + backup; khối của Warp ở file riêng; xác minh `ssh -G` sau khi ghi và khôi phục |
| `ssh -G` chạy `Match exec` trong config | Chỉ gọi cho alias trong config của chính người dùng; timeout 5 s; ghi ở GD7 |
| Config lớn/vòng Include làm treo lúc khởi động | Chạy nền, giới hạn độ sâu/số file/kích thước, chống vòng |
| `hosts.toml` bị sửa tay làm hỏng | Không ghi đè file hỏng; báo lỗi rõ; danh bạ rỗng trong RAM |
| Danh sách host (inventory) lộ cho agent qua `list_hosts` | Cùng quyền với `remote.session.list` (cần pairing khi O2 bật); không trả bí mật; tag/host là thông tin người dùng chủ động khai |
| Lệch giữa `ssh_config` và kho khi người dùng đổi tên alias | Alias cũ → `missing`, alias mới → host mới; metadata không tự chuyển (chấp nhận, ghi rõ) |

**Để sau:** file watcher `~/.ssh/config`; "Move to Warp"; nhóm/thư mục host; import từ `~/.ssh/known_hosts`;
đồng bộ danh bạ giữa máy; bí mật (phase 5/G2); G2–G5.

---

## G2. Agent tự mở session — Implementation plan (v1)

> Viết ngày 2026-09-30 (Claude Sonnet 5.5) theo `ROADMAP.md` mục G2, AO7, AO13 và rủi ro của G. Người
> dùng quyết định làm G2 **trước** khi gate O2 đủ 1 tuần (sớm nhất 2026-10-06) nên có ba chốt an toàn:
> cờ riêng `AgentOpsOpenSession` (không vào DOGFOOD/PREVIEW/RELEASE), `open_session` chỉ nhận client đã
> **pair**, và host chưa có quy tắc thì **hỏi**, không bao giờ tự mở. Quyết định ghi vào bảng `GD26+`
> (mục 7), tiến độ và nhật ký cùng mục 7. Chia hai giai đoạn: **G2a** (host dùng key/agent, không có bí mật)
> làm ngay; **G2b** (mật khẩu SSH/sudo) chỉ **thiết kế** ở đây, không code tới khi người dùng xác nhận G2a.

### G2.1 Khảo sát: các câu hỏi phải trả lời bằng code (2026-09-30)

| # | Câu hỏi | Trả lời (đã đọc code) | Bằng chứng |
|---|---|---|---|
| Q1 | Sự kiện tin cậy "session remote đã Warpify xong"? | **Có.** `Sessions` (model của mỗi terminal view) phát `SessionsEvent::SessionBootstrapped(SessionBootstrappedEvent { session_id, spawning_command, shell, subshell_info, session_type })` ngay sau khi `sessions.insert(session)` trong `initialize_bootstrapped_session`, nên `Sessions::get(session_id)` đã trả `Session`. `TerminalView::handle_session_bootstrapped` chỉ là một người nghe. GD12 ghi "không có sự kiện" là **sai**: cái thiếu là sự kiện **thất bại** (ssh sai mật khẩu, đang chờ người bấm Warpify) → phải có timeout (GD32). | `terminal/model/session.rs:143-161, 350-470`, `terminal/view.rs:10208, 14212`, `terminal/model_events.rs:314` |
| Q2 | Nối tab mới với đúng `SessionId`? | Tab do Warp tạo nên biết `PaneId` và terminal view của nó (`tabs[active].pane_group` → `focused_pane_id` → `terminal_view_from_pane_id`). `TerminalView::sessions_model()` là `pub`, nên `AgentBridgeModel` **subscribe** thẳng vào `Sessions` của đúng view đó, không sửa `terminal/view.rs`. Session đầu tiên của tab là shell **local** (`session_type = Local`) → bỏ qua; session ssh có `session_type = WarpifiedRemote` và `subshell_info.ssh_connection_info.host` = alias gõ trong `ssh <alias>` (GD10, đã đúng ở HD). Khớp = cùng view + `WarpifiedRemote` + host (bỏ `user@`) = alias. `session_id` client dùng là `PaneId.to_string()`, còn attach theo `SessionId` của shell (đổi khi `sudo -i`) → registry giữ cả hai. | `terminal/ssh/util.rs:202`, `terminal/view.rs:8255`, `handlers/metadata.rs:927`, `handlers/remote.rs:982` |
| Q3 | Chỗ đặt logic mở tab? | `Workspace::agent_ops_connect_to_server` (private) dựng `PaneTemplateType::PaneTemplate { commands: [ssh <alias>] }` rồi `add_tab_with_pane_layout` — hàm này **kích hoạt** tab mới. `local_control` gọi Workspace qua `workspace_for_window(..)` + `workspace.update` (mẫu `layout.rs::create_tab`). `Workspace` có field private ⇒ code mới phải là module con của `view`: `workspace/view/agent_session_tab.rs` (cạnh `tab_grouping.rs`). | `workspace/view.rs:19704, 13113`, `handlers/layout.rs:38`, `workspace/view/tab_grouping.rs` |
| Q4 | Nhóm "Agents" luôn hiển thị? | Nhóm tab có thật: `tab_groups: HashMap<TabGroupId, TabGroup { name, collapsed, .. }>`, gate `FeatureFlag::GroupedTabs` (cargo `grouped_tabs` nằm trong `default`). Mẫu chèn tab vào nhóm: `new_tab_in_group` (`index_after_group` + `move_tab_to_index` + `expand_tab_group`). "Luôn hiển thị" = tạo nhóm tên `Agents` nếu chưa có và **mở rộng** mỗi lần thêm tab; nhóm tắt (`GroupedTabs` off) thì chỉ đặt tiêu đề tab `Agent · <alias>`. | `workspace/view.rs:7621, 7844`, `workspace/tab_group.rs`, `app/Cargo.toml:670` |
| Q5 | Giới hạn theo host/agent, `close_session` chỉ đóng session của chính agent? | Chưa có gì: cần registry RAM mới `OpenedSessions` trong `AgentBridgeModel` (cạnh `attachments`), khoá theo `PaneId`, giữ `agent_id` (danh tính đã pair), alias, mức access, trạng thái. Đếm theo alias và theo agent, dọn mục có pane đã biến mất bằng `session_entries`. Đóng = `PaneGroup::close_pane` (như `pane.close`, không hộp thoại) sau khi kiểm chủ sở hữu. | `handlers/close.rs:110`, `pane_group/mod.rs:4802` |
| Q6 | Đi qua policy O2 và hộp thoại duyệt? | `authorize` gắn với session đã attach (nhận `SessionId`); mở session **chưa có session** nên cần đường riêng, dùng lại phần hạ tầng: `ApprovalQueue` (đã cho `session: None` từ pairing), `wait_for_decision`, hộp thoại `AgentApprovalDialog`, toast persistent (P28), audit fail-closed. Thêm `ApprovalSubject::OpenSession`, một mục `[open]` trong `policy.toml` và `Policy::evaluate_open` (thuần). | `agent_bridge/approval.rs:18-72`, `handlers/agent.rs`, `agent_bridge/policy.rs` |
| Q7 | Lên root có đáng tin cậy không? | Có cơ chế xác định: `TerminalView::set_and_execute_subshell_command(cmd, shell_type)` đặt `pending_auto_bootstrap_shell_type` rồi chạy lệnh, và `AfterBlockStarted` tự bootstrap subshell (không banner) **nếu** `WarpifySettings::is_compatible_subshell_command(cmd)` đúng. Máy người dùng có `added_subshell_commands = ["sudo -i"]` nên khớp đúng chuỗi `sudo -i`. Điều kiện kiểm được trước khi gõ; kết quả kiểm chứng được bằng sự kiện `SessionBootstrapped` thứ hai (`spawning_command == "sudo -i"`). Hàm đó private → thêm một wrapper `pub(crate)` nhỏ (patch duy nhất vào `terminal/view.rs`). | `terminal/view.rs:26647, 26873, 12611-12640`, `terminal/warpify/settings.rs:520`, `~/.config/warp-oss/settings.toml` |

**Rủi ro/điều chưa chắc (không đoán, để CHECKPOINT xác nhận):** (a) tab **nền** (không active) có bootstrap
ssh bình thường không (H2A.3) — nếu không, GD30 lùi về "kích hoạt tab mới"; (b) Warpify hỏi người dùng trước
khi cài extension (`SshExtensionInstallMode::AlwaysAsk`) hoặc ssh hỏi passphrase/host key → session không
bao giờ bootstrap tới khi người bấm → `open_session` trả `connecting` (H2A.10); (c) đóng pane cuối của tab
có hiện hộp thoại xác nhận không (H2A.7).

### G2.2 Thiết kế

```
agent ─open_session{host,access,purpose,root?}─► warpctrl mcp ─(agent_token)─► remote.session.open
   1. cờ, tham số, alias (validate_alias, GD5), host có trong danh bạ và không `missing`
   2. token → agent_id đã pair (chưa pair = từ chối, GD28)         [nền: agents.toml]
   3. giới hạn (host/agent/tổng) trên registry đã dọn               [main]
   4. policy.toml `[open]` → Allow | Ask | Deny                     [nền]   Ask ⇒ hộp thoại O2 (subject OpenSession)
   5. audit `started` (fail-closed) → Workspace mở tab `ssh <alias>` (không cướp focus) → registry `Connecting`
   6. AgentBridgeModel nghe `Sessions` của view đó: WarpifiedRemote + host==alias ⇒ `attach(session, access)`
      (root: gõ `sudo -i` bằng wrapper subshell, chờ SessionBootstrapped thứ hai ⇒ chuyển attach sang session root)
   7. trả `ready` {session_id = pane id, user@host, access, elevation} hoặc `connecting` khi hết `wait_secs`
agent ─close_session{session_id}─► chỉ khi registry ghi đúng agent_id này đã mở ─► close_pane + detach
```

**Registry (`agent_bridge/opened.rs`, thuần + test):** `OpenedSessions { by_pane: HashMap<String, Opened> }`,
`Opened { agent_id, alias, access, purpose, want_root, opened_at, state }`,
`state = Connecting | Elevating { user_session } | Ready { session } | Abandoned`. Hàm thuần:
`check_limits(agent_id, alias, limits, live_panes)`, `on_bootstrapped(pane, &Bootstrapped) -> Step
{ Ignore, Attach{session}, RunSudo{session}, AttachRoot{session, replaces}}`, `expire(now)`, `elevation_plan(root_login,
want_root, sudo_i_is_warpifiable) -> Elevation {NotRequested, AlreadyRoot, Run, Skipped(reason)}`.
`AgentBridgeModel` giữ registry và các `oneshot::Sender` chờ `Ready`; đăng ký nghe bằng
`ctx.subscribe_to_model(view.sessions_model(), ..)`.

**Policy `[open]` (`policy.rs`, thêm — không đổi `[defaults]`/`[[hosts]]`/`[deny]`):**

```toml
[open]
max_sessions_per_host  = 2      # mặc định 2, trần cứng 8
max_sessions_per_agent = 4      # mặc định 4, trần cứng 16

[[open.hosts]]
match = "lab-*"                 # glob trên ALIAS (không phân biệt hoa thường), luật đầu tiên khớp thắng
tag = "lab"                     # tuỳ chọn: host phải có tag này
mode = "allow"                  # allow | ask | deny
max_access = "full"             # read_only (mặc định) | full — mức tối đa được mở KHÔNG hỏi
allow_root = false              # true: được tự lên root (sudo -i) mà không hỏi
```

`evaluate_open`: Deny/Ask/Allow theo luật đầu tiên khớp; **không có luật ⇒ Ask** (không đọc `[defaults]`);
`allow` mà access xin > `max_access`, hoặc xin root mà `allow_root = false` ⇒ **Ask** (không tự hạ mức
lặng lẽ); `deny` ⇒ Deny. File lỗi/quyền rộng ⇒ Deny (fail-closed như P8). Sau khi attach tự động, mọi
`exec`/`write` vẫn qua `authorize` và `[[hosts]]`/`[deny]` cũ như trước: tự attach **không** cấp quyền chạy lệnh.
"Mức tối đa theo root_login/tag": tag chọn luật; `root_login` của host trong danh bạ quyết định root có khả
thi (`sudo_nopasswd` ⇒ chạy `sudo -i`; `root` ⇒ đã là root; `sudo_password`/`none` ⇒ không lên, nói rõ trong kết quả).

**Hộp thoại duyệt:** `ApprovalSubject::OpenSession { alias, access, purpose, tags, connection, root }`
(`connection` = `user@host:port` từ `ssh -G`, GD7/GD19, nền, tối đa 5 s). Title `Open a session to <alias>?`;
body: agent (paired), `Access: read-only|full`, `Purpose: …`, `Connects as: …`, tag, dòng root theo kế hoạch
lên root ("Runs sudo -i for you" / "Signs in as root" / "Stays as the login user: <lý do>"), hạn tự từ chối.
Chỉ nút Deny / Approve (không có "Allow in session"). Toast persistent, mỗi request chờ một `object_id` chung
`agent_ops_open_request` (gỡ khi không còn request mở nào chờ). Tối đa 3 request mở đang chờ mỗi agent.

**Audit:** action `remote.session.open` / `remote.session.close`; `host` = alias, `agent`, `agent_id`,
`request_id`, `policy_decision`, `policy_reason`, thêm hai trường `purpose` và `access`
(`skip_serializing_if` rỗng). Dòng `approval_requested` (nếu Ask) → dòng `started` **trước** khi mở tab
(fail-closed) → dòng kết thúc (`ok`/`error`, `session_id` = pane id). Không bao giờ ghi bí mật (G2a không
có).

**Giao thức (không đổi `PROTOCOL_VERSION`):**

- `remote.session.open` (target `Instance`): `RemoteSessionOpenParams { host, access: RemoteAccess (mặc định
  read_only), purpose (1–200 ký tự, không ký tự điều khiển), root: bool (mặc định false), wait_secs: 1–180
  (mặc định 60), agent }`, kết quả `RemoteSessionOpenResult { status: ready|connecting, session_id, host_alias,
  host?, user?, access, elevation: not_requested|already_root|elevated|pending|skipped, note? }`.
- `remote.session.close` (target `Session`, id trong selector như `remote.exec`): `RemoteSessionCloseParams { agent }`,
  kết quả `RemoteSessionCloseResult { session_id, closed }`.
- Catalog 98 → 100 (`REMOTE_ACTIONS` 7 → 9). Không có subcommand `warpctrl` (như `agent.pair`, P25 của O2): hai
  action này cần danh tính đã pair nên chỉ MCP dùng được; test ví dụ CLI loại trừ chúng.
- MCP: `open_session {host, access?, purpose, root?, wait_secs?}`, `close_session {session_id}`; thời gian chờ client =
  `wait_secs + APPROVAL_CLIENT_MARGIN + 30 s`. `INSTRUCTIONS` thêm: chỉ mở session khi thật cần, ghi `purpose` thật, mở
  đúng số session cần, `close_session` khi xong, đừng thử lách khi bị từ chối, `connecting` nghĩa là người dùng có thể
  phải trả lời một câu hỏi trong tab.

**Lên root (`root: true`, G2a phase 4):** chỉ khi `elevation_plan == Run` (host `sudo_nopasswd`, shell bash/zsh/fish, và
`is_compatible_subshell_command("sudo -i")` đúng); gõ **đúng** `sudo -i` bằng wrapper subshell (không bao giờ gõ mật
khẩu ở G2a — nếu `sudo` hỏi mật khẩu, người dùng tự gõ trong tab); thành công khi có `SessionBootstrapped` thứ hai
(`spawning_command == "sudo -i"`, cùng view, `WarpifiedRemote`), lúc đó attach chuyển sang session root và
detach session user. Điều kiện không thoả ⇒ giữ nguyên session ở mức user, `elevation: skipped` + `note` nêu lý do
(ví dụ "thêm `sudo -i` vào Settings > Warpify > Added commands").

### G2.3 Các phase

Thứ tự: 1 → 2 → 3 → 4 → 5 → ⛔ **CHECKPOINT H2A** (dừng, chờ người dùng test tay). Cuối mỗi phase: test nhỏ nhất
liên quan; **cuối phase 5**: test liên quan → clippy (`-p warp -p local_control -p warp_cli`, features
`warp/warp_control_cli,warp/warp_sync,warp/agent_bridge,warp/agent_ops_policy,warp/agent_ops_hosts,warp/agent_ops_open_session`)
→ `./script/format` một lần. Không commit, không push, không tick G2 trong `ROADMAP.md`.

| Phase | Nội dung | Kiểm |
|---|---|---|
| **1** Nền thuần | 1.1 cờ `AgentOpsOpenSession` + cargo feature `agent_ops_open_session` (không vào DOGFOOD/PREVIEW/RELEASE) + hằng số; 1.2 protocol: params/result, `ActionKind::RemoteSessionOpen/Close`, catalog, specs, resolver, loại khỏi test ví dụ CLI, đếm catalog (protocol_tests 7→9, mod_tests 98→100); 1.3 `policy.rs` `[open]` + `evaluate_open` + `open_limits`; 1.4 `agent_bridge/opened.rs` (registry, giới hạn, `on_bootstrapped`, `elevation_plan`, `expire`); 1.5 audit (`purpose`, `access`, hàm ghi open/close); 1.6 `ApprovalSubject::OpenSession` + nội dung hộp thoại + toast + giới hạn chờ | `nextest` nhỏ theo file |
| **2** Mở/đóng | 2.1 `workspace/view/agent_session_tab.rs` (tab không cướp focus, nhóm `Agents`, tiêu đề); 2.2 `AgentBridgeModel`: registry, `watch_open` (subscribe `Sessions`), attach khi `Step::Attach`, gỡ khi detach; 2.3 handler `handlers/open_session.rs` (`open`, `close`) + arm trong `bridge.rs`; 2.4 test handler với `mock_workspace` | nextest `agent_bridge::|local_control::|workspace::` |
| **3** MCP | 3.1 `open_session`/`close_session` trong `tools.rs`, `format.rs::render_open`, `INSTRUCTIONS`, `specs/agent-bridge/claude/SKILL.md`; 3.2 timeout client; 3.3 `tools_tests`/`format_tests`/`jsonrpc_tests` | nextest `warp_cli` gộp với `warp` |
| **4** Lên root | 4.1 wrapper `pub(crate)` trong `terminal/view.rs`; 4.2 luồng `Elevating` (sự kiện thứ hai, chuyển attach); 4.3 test thuần + handler | nextest nhỏ |
| **5** Review | tự review theo AGENTS.md/quy tắc security (input người dùng, thi hành lệnh, đường dẫn, log), test toàn bộ phần đụng tới, clippy, format một lần, nhật ký | như trên |

### G2.4 Checklist test tay (người dùng) — ⛔ CHECKPOINT H2A

Mỗi bước có 4 phần: **Làm** (lệnh/thao tác cụ thể), **Kỳ vọng**, **Ghi lại** (cái cần báo lại cho Claude), **Dọn** (đưa về trạng thái ban đầu
cho bước sau). Làm tuần tự; bước nào lệch kỳ vọng thì **dừng** và dán cho Claude: câu bạn gõ, kết quả Claude trả, 3 dòng audit cuối.

#### Chuẩn bị (làm một lần)

**P0 — Chọn host thử.** Dùng host homelab có key, không mật khẩu, **đừng dùng prod**. Trong `~/.ssh/config` của bạn có sẵn:

| Vai trò | Alias | Ghi chú |
|---|---|---|
| `LAB` | `home-docker-02` | `root@192.168.100.211:22`, key `~/.ssh/id_rsa`. Host chính để mở session |
| `OTHER` | `home-docker-03` | `root@192.168.100.212:22`. Host **không có luật** `[open]`, dùng cho H2A.2, H2A.4, H2A.6, H2A.10 |

Đổi sang host khác nếu muốn, nhưng phải có trong Settings > Servers (không `missing`) và đăng nhập được bằng key. Kiểm ngay ngoài Warp
(kết quả phải là hostname, **không** hỏi gì, không treo):

```bash
ssh -o BatchMode=yes home-docker-02 hostname
ssh -o BatchMode=yes home-docker-03 hostname
```

Lỗi `Permission denied` hoặc `Host key verification failed` ⇒ sửa trước (`ssh home-docker-02` một lần, gõ `yes`, nhập passphrase), nếu không H2A.3
sẽ treo mà bạn không biết vì sao. Trong Warp, xác nhận `list_hosts` (hoặc Settings > Servers) thấy cả hai alias.

**P1 — Build và chạy Warp.**

```bash
cd /projects/github/warp-agent-bridge
./script/run --features warp_control_cli,warp_sync,agent_bridge,agent_ops_policy,warp_sync_remote_edit,agent_ops_hosts,agent_ops_open_session,release_bundle
```

`release_bundle` tuỳ chọn như các lần trước. Chạy xong Warp mở lên là bản để test; **không** đóng nó trừ khi bước yêu cầu (H2A.14, H2A.15).
Cờ `agent_ops_open_session` là thứ bật `open_session`; thiếu nó thì mọi bước dưới đều trả `unsupported_action`.

**P2 — Hai hàm shell** (dán vào terminal **ngoài Warp** hoặc một tab local, dùng suốt buổi test):

```bash
# Xem N dòng audit cuối, gọn, không hiện nội dung lệnh dài
aud() { tail -n "${1:-3}" ~/.warp/agent-bridge/audit.jsonl | jq -c '{ts_unix, action, host, session_id, result, agent, agent_id, request_id, purpose, access, policy_decision}'; }

# Ghi lại policy.toml = bản gốc + phần bạn truyền qua stdin (heredoc). `setopen </dev/null` = về bản gốc.
setopen() { cp ~/.warp/agent-ops/policy.toml.bak-h2a ~/.warp/agent-ops/policy.toml && cat >> ~/.warp/agent-ops/policy.toml && chmod 600 ~/.warp/agent-ops/policy.toml; }
```

**P3 — Sao lưu policy gốc** (một lần; file hiện chưa có mục `[open]`, tức là mọi host đều **hỏi**):

```bash
cp -p ~/.warp/agent-ops/policy.toml ~/.warp/agent-ops/policy.toml.bak-h2a
chmod 600 ~/.warp/agent-ops/policy.toml.bak-h2a
grep -c '^\[open' ~/.warp/agent-ops/policy.toml     # phải in 0
```

`policy.toml` được đọc lại ở **mỗi request**, sửa xong có hiệu lực ngay, không restart. Nếu file sai cú pháp hoặc không phải `0600` thì mọi request bị từ chối
kèm lý do, đó là fail-closed chứ không phải lỗi của bước.

**P4 — Đăng ký MCP server thứ hai** (chỉ H2A.1 cần; làm sớm để khỏi quên). Đây là **cùng binary**, thêm cờ `--no-pair` nên nó không bao giờ gửi token đã pair:

```bash
claude mcp add --scope user warp-bridge-nopair -- /projects/github/warp-agent-bridge/target/debug/warp-oss --warpctrl mcp --no-pair
claude mcp list          # phải thấy warp-bridge và warp-bridge-nopair, cả hai Connected
```

Claude Code chỉ nạp MCP server mới khi **mở phiên mới** (hoặc gõ `/mcp` để kết nối lại). Tool của server này tên `mcp__warp-bridge-nopair__open_session`;
tool của server đã pair tên `mcp__warp-bridge__open_session`. Xong buổi test: `claude mcp remove warp-bridge-nopair --scope user`.

**P5 — Cách ra lệnh cho Claude Code.** Nói thẳng tên tool và tham số, kèm yêu cầu trả nguyên văn, ví dụ:

> Dùng tool `open_session` của MCP `warp-bridge` (không phải tool nào khác): host `home-docker-02`, access `read_only`, purpose `kiểm tra dung lượng đĩa`.
> Đừng tự thử lại hay tự sửa nếu bị từ chối; dán **nguyên văn** kết quả hoặc lỗi trả về.

Với server không pair: đổi thành "MCP `warp-bridge-nopair`". Các câu mẫu khác dùng cùng khuôn: đổi host, `access`, `purpose`, `wait_secs`, `root`.

**P6 — Cần nhìn ở đâu trong Warp.**

- **Tab và nhóm `Agents`:** thanh tab (hoặc panel tab dọc). Nhóm `Agents` chỉ hiện khi có tab do agent mở (`GroupedTabs` bật mặc định). Tab đặt tên `Agent · <alias>`.
- **Toast** "wants to open a session to …" và nút **Review** mở hộp thoại duyệt. Toast không tự tắt cho tới khi bạn xử lý hoặc hết giờ (5 phút thì tự Deny).
- **Palette:** `Ctrl+Shift+P`, gõ `Agent Bridge`: `Allow agents to control this session` (full), `Allow agents to read this session (read-only)`, `Revoke access to this session`, `Revoke all sessions`.
- **Session nào đang attach:** bảo Claude gọi `list_sessions` (cho biết `attached` và mức `read-only`/`full`).
- **Audit:** `aud 3` (P2).

**P7 — Mẫu báo kết quả** (dán vào chat cuối buổi hoặc theo từng bước): `H2A.n: đạt / không đạt / bỏ qua — ghi chú`. Các mục **Ghi lại** bên dưới là những thông tin
Claude cần để chốt các câu hỏi còn treo ở mục G2.1 (tab nền, Warpify có hỏi trước, đóng tab có hộp thoại, `sudo -i`).

---

#### H2A.1 — Chưa pair

**Làm**
1. Đảm bảo P4 đã xong và phiên Claude Code mới thấy `warp-bridge-nopair`.
2. Bảo Claude: "Dùng tool `open_session` của MCP `warp-bridge-nopair`: host `home-docker-02`, access `read_only`, purpose `test chưa pair`. Dán nguyên văn kết quả."
3. `aud 2`.

**Kỳ vọng**
- Claude nhận lỗi `policy_denied`, nội dung có "needs an agent that is paired with Warp … without --no-pair".
- **Không** có tab mới, **không** toast, **không** hộp thoại.
- Audit: không có dòng `started` nào cho lần gọi này (nếu có dòng từ chối thì cũng không kèm tab).

**Ghi lại:** đúng mã lỗi và câu chữ; có dòng audit mới hay không.
**Dọn:** không cần. (Server `warp-bridge-nopair` để nguyên tới hết buổi, mọi bước sau dùng `warp-bridge`.)

#### H2A.2 — Hỏi (mặc định, chưa có luật)

**Làm**
1. `setopen </dev/null` (về policy gốc, không có `[open]`).
2. Bảo Claude (MCP `warp-bridge`): host `home-docker-02`, access `read_only`, purpose `kiểm tra dung lượng đĩa`, `wait_secs` `120`.
3. Trong Warp: xem toast, bấm **Review**, đọc hộp thoại.
4. Thử phím **Enter** (không được làm gì), rồi **Esc** (đóng hộp thoại, request vẫn chờ, toast vẫn còn). Mở lại bằng Review.
5. Bấm **Deny**.
6. `aud 4`.

**Kỳ vọng**
- Toast không tự tắt: "'claude-code' wants to open a session to home-docker-02".
- Hộp thoại có: agent `claude-code (paired)`, `Access: read-only`, `Purpose: kiểm tra dung lượng đĩa`, `Connects as: root@192.168.100.211:22` (có thể chưa có nếu `ssh -G` chậm),
  tag (nếu có), dòng root nói không lên root. Chỉ có **Deny** / **Approve**, không có "Allow in session".
- Sau Deny: Claude nhận "the user denied it". **Không** có tab mới.
- Audit: `approval_requested`, rồi dòng kết thúc từ chối; `purpose` và `access` có mặt.

**Ghi lại:** hộp thoại thiếu/dư trường nào; Enter có làm gì không; toast có tắt không.
**Dọn:** không cần.

#### H2A.3 — Approve

**Làm**
1. Đứng ở **tab A** bất kỳ (ghi nhớ tab nào), gõ dở vài chữ vào prompt của tab A (để thử "không cướp focus").
2. Lặp lại yêu cầu của H2A.2, lần này **Approve**.
3. Quan sát ngay lúc tab mới hiện: tab đang xem có đổi không, chữ đang gõ có còn ở tab A không.
4. Chờ Claude nhận kết quả. Trong tab `Agent · home-docker-02` xem ssh chạy và Warpify (khối/banner xuất hiện).
5. Bảo Claude: gọi `list_sessions`; `read_file` `/etc/hostname` trên session đó; rồi `exec` `hostname` trên session đó (phải bị từ chối vì read-only).
6. Nhìn header pane của tab agent.

**Kỳ vọng**
- Tab `Agent · home-docker-02` nằm trong nhóm **Agents** (nhóm mở rộng). Tab bạn đang xem **không đổi** (nhưng chỉ số tab có thể dịch 1 vì nhóm `Agents` nằm đầu, GD38: kiểm bằng *tab nào* đang được chọn, không bằng số thứ tự).
- Claude nhận `ready` gồm `session_id`, `root@home-docker-02` (hoặc user@host), `read-only`, `elevation: not_requested`.
- `list_sessions` thấy session đó `attached`; `read_file` được; `exec` bị từ chối vì chỉ đọc.
- Header pane hiện "Agents · read-only".

**Ghi lại:** *(quan trọng)* tab **nền** có bootstrap ssh/Warpify bình thường không, hay đứng im tới khi bạn bấm vào tab (nếu đứng im thì GD30 lùi về "kích hoạt tab mới"). Chữ đang gõ có bị nhảy tab không. Thời gian từ Approve tới `ready`.
**Dọn:** để session mở cho H2A.7 (hoặc đóng bằng H2A.7 ngay).

#### H2A.4 — Luật `allow`

**Làm**
1. Đặt luật cho LAB:
```bash
setopen <<'EOF'

[open]
[[open.hosts]]
match = "home-docker-02"
mode = "allow"
max_access = "full"
EOF
```
2. Bảo Claude: `open_session` host `home-docker-02`, access `full`, purpose `thử luật allow`.
3. Bảo Claude `exec` `hostname` trên session vừa mở. Xem hộp thoại O2 (hỏi quyền chạy lệnh).
4. Bảo Claude: `open_session` host `home-docker-03` (OTHER, không luật), access `read_only`. → phải hỏi; bấm **Deny**.
5. Đổi luật, thêm deny cho OTHER:
```bash
setopen <<'EOF'

[open]
[[open.hosts]]
match = "home-docker-02"
mode = "allow"
max_access = "full"
[[open.hosts]]
match = "home-docker-03"
mode = "deny"
EOF
```
6. Lại `open_session` `home-docker-03`. `aud 5`.

**Kỳ vọng**
- Bước 2: mở tab **ngay**, không toast, không hộp thoại; `ready` với `full`.
- Bước 3: `exec` đầu tiên **vẫn hỏi** như O2 (tự attach không cấp quyền chạy lệnh); Approve thì chạy được.
- Bước 4: OTHER vẫn hỏi.
- Bước 6: từ chối **ngay** (`policy_denied`), không tab, không hộp thoại.
- Audit: `policy_decision` khác nhau giữa allow / ask / deny.

**Ghi lại:** `policy_decision` của từng dòng; có hộp thoại thừa/thiếu không.
**Dọn:** đóng tab agent đã mở (`close_session` hoặc tay). Giữ luật cho bước sau tuỳ bước.

#### H2A.5 — Vượt mức `max_access`

**Làm**
1. Đặt luật (dán vào terminal ngoài Warp):
```bash
setopen <<'EOF'

[open]
[[open.hosts]]
match = "home-docker-02"
mode = "allow"
max_access = "read_only"
EOF
```
2. `open_session` `home-docker-02`, access `full`, purpose `thử vượt mức`.
3. Đọc hộp thoại rồi **Deny**.
4. `open_session` `home-docker-02`, access `read_only` → phải mở không hỏi. Đóng nó.

**Kỳ vọng**
- Bước 2: **hỏi** (không tự hạ xuống read-only), hộp thoại ghi `Access: full`.
- Bước 4: mở ngay, không hỏi.

**Ghi lại:** hộp thoại ghi `full` hay `read-only`. **Dọn:** đóng tab agent.

#### H2A.6 — Giới hạn

Giới hạn tính **theo từng agent** (không tính session bạn mở tay).

**Làm**
1. Đặt luật (dán vào terminal ngoài Warp):
```bash
setopen <<'EOF'

[open]
max_sessions_per_host = 2
[[open.hosts]]
match = "home-docker-02"
mode = "allow"
max_access = "read_only"
EOF
```
2. `open_session` `home-docker-02` (read_only) **3 lần liên tiếp** (mỗi lần chờ `ready`, đừng đóng).
3. Đóng cả hai tab agent (`close_session` hoặc tay), rồi:
```bash
setopen <<'EOF'

[open]
max_sessions_per_agent = 1
[[open.hosts]]
match = "home-docker-02"
mode = "allow"
max_access = "read_only"
EOF
```
4. `open_session` `home-docker-02` (mở được), rồi `open_session` `home-docker-03` (host khác).

**Kỳ vọng**
- Bước 2: hai lần đầu `ready`; lần 3 lỗi `policy_denied`: "you already have 2 sessions open on home-docker-02; close one with close_session first." Không có tab thứ ba, không hộp thoại.
- Bước 4: lần 2 lỗi: "you already have 1 sessions open; close one with close_session first." và **không** hiện hộp thoại (giới hạn kiểm **trước** policy).

**Ghi lại:** câu chữ lỗi thật. **Dọn:** `setopen </dev/null`, đóng tab agent.

#### H2A.7 — `close_session`

**Làm**
1. Mở một session agent (luật allow như H2A.6 cho nhanh, hoặc Approve).
2. Bảo Claude gọi `close_session` với `session_id` đó.
3. Quan sát tab. Gọi lại `close_session` lần hai với cùng id.
4. Tự mở **tay** một tab ssh tới `home-docker-02` (`ssh home-docker-02`, Warpify), attach tay (palette `Agent Bridge: Allow agents to read this session (read-only)`), rồi bảo Claude `close_session` với `session_id` của tab tay đó (lấy từ `list_sessions`).

**Kỳ vọng**
- Bước 3: tab biến mất, attach mất; lần gọi thứ hai lỗi rõ "cannot find that session any more".
- Bước 4: từ chối "this session was not opened by you, so it is not yours to close. Sessions the user opened stay open until the user closes them." Tab tay còn nguyên.

**Ghi lại:** *(quan trọng)* khi đóng tab agent có **hộp thoại xác nhận** (kiểu "process still running") không? Nếu có, Claude bị treo hay nhận lỗi?
**Dọn:** đóng tab tay.

#### H2A.8 — Đóng tay

**Làm**
1. Với luật `max_sessions_per_host = 2` (như H2A.6 bước 1): mở 2 session agent tới `home-docker-02`. Xác nhận lần 3 bị từ chối.
2. Tự đóng **một** tab agent bằng nút × trên tab (hoặc `Ctrl+Shift+W`).
3. Bảo Claude mở lần nữa.

**Kỳ vọng:** lần mở ở bước 3 thành công (giới hạn được giải phóng, không cần Claude gọi `close_session`).
**Ghi lại:** bước 2 có hộp thoại xác nhận đóng không. **Dọn:** đóng hết tab agent.

#### H2A.9 — Giành lại quyền

**Làm**
1. Mở một session agent (read-only), xác nhận `read_file` chạy được.
2. Trong tab agent: bấm **Revoke** trên header pane (hoặc palette `Agent Bridge: Revoke access to this session`).
3. Bảo Claude `read_file` lại trên session đó, rồi `close_session`.

**Kỳ vọng:** `read_file` ở bước 3 trả `session_not_attached`; tab **vẫn còn**; `close_session` vẫn đóng được (session do agent mở nên vẫn thuộc agent).
**Ghi lại:** câu lỗi thật. **Dọn:** không cần.

#### H2A.10 — Chậm / hỏi (passphrase hoặc host key mới)

**Làm** (tạo tình huống "host key mới" bằng cách quên host key của OTHER; ssh sẽ hỏi `yes/no`):
1. `ssh-keygen -R 192.168.100.212` (ghi `known_hosts.old` làm bản sao; muốn khôi phục thì `mv ~/.ssh/known_hosts.old ~/.ssh/known_hosts` hoặc trả lời `yes` lần sau).
Nếu `HashKnownHosts` bật vẫn dùng đúng lệnh này. Kiểm: `ssh -o BatchMode=yes home-docker-03 hostname` phải **lỗi** `Host key verification failed`.
2. Cho OTHER một luật allow để không vướng hộp thoại, hoặc Approve tay:
```bash
setopen <<'EOF'

[open]
[[open.hosts]]
match = "home-docker-03"
mode = "allow"
EOF
```
3. Bảo Claude: `open_session` `home-docker-03`, read_only, `wait_secs` `15`.
4. Đợi 15 s. Chuyển sang tab `Agent · home-docker-03`: thấy dòng "Are you sure you want to continue connecting (yes/no)?". Gõ `yes`, Enter.
5. Chờ Warpify xong. Bảo Claude `list_sessions` (không mở lại).

**Kỳ vọng**
- Sau ~15 s Claude nhận `status: connecting` (**không phải lỗi**) kèm note nói người dùng có thể phải trả lời câu hỏi trong tab.
- Sau bước 4, Warpify xong, session **tự attach**: `list_sessions` thấy `attached`, mức read-only, không cần Claude mở lại.

**Ghi lại:** *(quan trọng)* Warpify có hiện hộp thoại hỏi cài extension (`SshExtensionInstallMode::AlwaysAsk`) không; nếu có và bạn không bấm, tab đứng im bao lâu.
**Dọn:** đóng tab agent; `setopen </dev/null`; đảm bảo host key OTHER đã có lại (`ssh -o BatchMode=yes home-docker-03 hostname` chạy được).

#### H2A.11 — Alias xấu, host không có

**Làm**: bảo Claude gọi lần lượt `open_session` (purpose bất kỳ) với `host`:
1. `a;b`
2. `-oProxyCommand=x`
3. `does-not-exist`
4. (tuỳ chọn) một alias từng có trong `~/.ssh/config` nhưng bạn đã xoá (host `missing` trong Settings > Servers).

Ngoài ra thử `purpose` rỗng, `purpose` chứa xuống dòng, và `wait_secs` `0` hoặc `999`.

**Kỳ vọng**
- 1 và 2: `invalid_params` ngay, **không** vào bước policy, không tab, không hộp thoại.
- 3: từ chối "there is no server "does-not-exist" in the user's server list; see list_hosts".
- 4: từ chối "is no longer in the user's SSH configuration".
- `purpose`/`wait_secs` sai: `invalid_params` nêu đúng tham số.

**Ghi lại:** câu chữ lỗi; xác nhận **không có tab nào mở** cho các trường hợp này. **Dọn:** không cần.

#### H2A.12 — Song song

**Làm**
1. Đặt luật allow cho `home-docker-02` (read_only) như H2A.6 bước 1.
2. Bảo Claude gọi `open_session` `home-docker-02` **hai lần liên tiếp** rồi `list_sessions`.
3. Bảo Claude `read_file` `/etc/hostname` trên **cả hai** session **trong cùng một lượt** (song song).
4. (Tuỳ chọn, thử `exec`) đổi luật thành `max_access = "full"`, mở hai session `access: full`, bảo Claude `exec` `hostname` trên cả hai trong một lượt và Approve từng hộp thoại.

**Kỳ vọng:** hai tab riêng, hai `session_id` khác nhau, cả hai `attached`; hai lệnh chạy độc lập không lẫn kết quả.
**Ghi lại:** hai tab có cùng tên `Agent · home-docker-02` (phân biệt bằng gì); có lần nào nhầm session không.
**Dọn:** đóng cả hai.

#### H2A.13 — Audit

**Làm:** `aud 30` sau khi đã làm các bước trên; đọc các dòng `remote.session.open` và `remote.session.close`.

**Kỳ vọng:** mỗi dòng có `agent`, `agent_id`, `request_id`, `purpose`, `access`, `policy_decision`; `session_id` = id pane; dòng `started` có **trước** dòng kết thúc của cùng `request_id`; **không** có nội dung giống bí mật. Kiểm nhanh:

```bash
grep -E 'remote\.session\.(open|close)' ~/.warp/agent-bridge/audit.jsonl | tail -20 | grep -iE 'password|passphrase|token|BEGIN .*KEY' || echo "sạch"
```

**Ghi lại:** dòng nào thiếu trường.

#### H2A.14 — Khởi động lại Warp

**Làm**
1. Mở một session agent (Approve hoặc luật allow), giữ nó mở.
2. Thoát hẳn Warp rồi chạy lại (P1, hoặc mở lại `target/debug/warp-oss`).
3. Nếu Warp khôi phục tab, xem tab `Agent · …`. Bảo Claude `list_sessions`, rồi `open_session` lại.

**Kỳ vọng:** registry và attach **mất** sau khởi động lại; tab khôi phục (nếu có) **không** ở trạng thái attach; `open_session` mới chạy bình thường, không bị giới hạn oan.
**Ghi lại:** tab có được khôi phục không, nhóm `Agents` còn không.

#### H2A.15 — Không cờ

**Làm**
1. Thoát Warp. Build lại **không** có `agent_ops_open_session`:
```bash
./script/run --features warp_control_cli,warp_sync,agent_bridge,agent_ops_policy,warp_sync_remote_edit,agent_ops_hosts,release_bundle
```
2. Bảo Claude `open_session` `home-docker-02`; rồi `list_hosts`; rồi `exec` trên một session attach tay.

**Kỳ vọng:** `open_session` lỗi `unsupported_action`; `list_hosts` và `exec` **không đổi** hành vi.
**Dọn:** build lại bản có cờ nếu còn bước cần.

#### H2A.16 — Lên root (tuỳ chọn)

Cả hai host homelab ở trên đăng nhập thẳng bằng `root` nên chỉ thử được nhánh `already_root`. Muốn thử `sudo -i` thật cần một host đăng nhập **không phải root** và
`sudo` không hỏi mật khẩu; nếu không có, bỏ qua phần b.

**Làm**
- **a) Đã là root:** trong Settings > Servers đặt `home-docker-02` là `root_login = root`. Luật `allow_root = true` cho host này. `open_session` `home-docker-02` `root: true`.
- **b) `sudo -i`:** host `sudo_nopasswd` (Settings > Servers) + Settings > Warpify > Added commands có `sudo -i`. `open_session … root: true`.
- **c) Không đủ điều kiện:** host `sudo_password` hoặc thiếu `sudo -i` trong Warpify. `open_session … root: true`.

**Kỳ vọng**
- a) hộp thoại (nếu hỏi) ghi "Signs in as root"; kết quả `elevation: already_root`.
- b) hộp thoại ghi "Runs sudo -i for you"; sau Approve tab tự gõ `sudo -i`, Warpify lần hai, Claude nhận `elevation: elevated`, user `root`.
- c) session mở ở mức user, `elevation: skipped` + lý do (ví dụ "thêm `sudo -i` vào Settings > Warpify > Added commands").

**Ghi lại:** *(quan trọng, chỉ b)* `sudo -i` do wrapper gõ có tự Warpify không, hay Warp hiện banner hỏi bạn (nếu hỏi, `elevation` sẽ là `pending` hay `skipped`).

---

#### Sau khi xong

1. Khôi phục: `cp -p ~/.warp/agent-ops/policy.toml.bak-h2a ~/.warp/agent-ops/policy.toml`; đóng mọi tab agent; `claude mcp remove warp-bridge-nopair --scope user`.
2. Báo Claude bảng `H2A.n: đạt/không đạt — ghi chú` và các mục **Ghi lại** quan trọng (H2A.3, H2A.7, H2A.10, H2A.16). Claude sẽ tick CHECKPOINT H2A ở mục 7, ghi nhật ký, và quyết định GD30/GD32 nếu cần.

### G2.5 G2b — Mật khẩu SSH/sudo (chỉ thiết kế, không code cho tới khi người dùng xác nhận G2a)

Mục tiêu (AO8): host `auth = password` và `root_login = sudo_password` dùng được, bí mật **không bao giờ** tới agent, audit, log,
lịch sử shell. Cần khảo sát thêm trước khi code (ghi ở đây để khỏi đoán):

1. **Kho:** `warpui_extras::secure_storage`, khoá `host:<alias>:ssh_password` / `:sudo_password` / `:ssh_passphrase`. Nhập ở Settings > Servers (ô che chữ, chỉ ghi,
có "Forget"); danh bạ vẫn không chứa bí mật; `list_hosts` chỉ báo `has_secret: bool`.
2. **Askpass:** ssh local đọc mật khẩu từ helper (`warp --warpctrl askpass <prompt>`) do `SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force` chỉ tới. Hai câu hỏi mở phải
trả lời bằng khảo sát: (a) đưa biến môi trường vào **tab của agent** thế nào khi lệnh phải là `ssh <alias>` (Warpify chỉ bắt lệnh mở đầu bằng `ssh`, `terminal/ssh/util.rs:202`,
nên `VAR=x ssh …` mất Warpify) — hướng ưu tiên: đặt env cho shell của tab lúc spawn (xem `PaneTemplateType`/terminal manager) thay vì gõ `export`; (b) prompt
`yes/no` của host key: helper **không bao giờ trả lời**, ssh sẽ thất bại (không quay về tty khi `force`) ⇒ host phải có sẵn trong `known_hosts` (báo lỗi rõ: "kết nối tay một lần").
3. **Broker:** vé một lần (32 byte ngẫu nhiên, RAM, sống 60 s, gắn alias + pane) truyền qua env; helper gửi `{ticket, prompt}` tới bridge bằng cơ chế credential/UID có sẵn
(`local_control::auth`); Warp **chỉ trả lời** prompt khớp regex neo đúng cho alias đó (`^<user>@<host>'s password: $`, `^Enter passphrase for key '<path của key>': $`), mỗi vé một lần,
sai/lạ thì trả lỗi (không im lặng). Không ghi prompt đầy đủ vào log.
4. **sudo:** chỉ khi chính Warp vừa gõ `sudo -i` (phase 4) và block output kết thúc bằng `^\[sudo\] password for <user>: $` trong 15 s: ghi mật khẩu một lần thẳng vào PTY (không qua ô
input/lịch sử/AI context), rồi xoá cờ; prompt giả khác (chương trình khác in dòng giống hệt) không có cờ nên không được điền.
5. **Kiểm thử:** hàm thuần khớp prompt (bảng dương/âm gồm prompt giả), vé hết hạn/dùng lại, test log không chứa mật khẩu (grep), audit không chứa mật khẩu.
Tách thành phase B1 (kho + Settings), B2 (askpass + vé), B3 (sudo) khi được duyệt.

### G2.6 Rủi ro và để sau

| Rủi ro | Giảm thiểu / chấp nhận |
|---|---|
| Agent tự mở session root ở máy chưa qua gate O2 | Cờ riêng (không vào DOGFOOD), phải pair, mặc định hỏi, root chỉ theo `sudo_nopasswd` + `allow_root`; mọi lệnh ghi sau đó vẫn qua `authorize` |
| Nhiều hộp thoại mở session làm người dùng bấm theo phản xạ | Tối đa 3 request mở chờ mỗi agent; hộp thoại chỉ mở khi bấm Review; không Enter |
| Tab nền không bootstrap | H2A.3 kiểm; lùi về kích hoạt tab (GD30) |
| Session không bao giờ Warpify (hỏi passphrase, host key, extension, mạng) | `connecting` + `PENDING_TTL` 10 phút; vẫn tính vào giới hạn tới khi pane đóng; `close_session` đóng được |
| `ssh -G` chạy `Match exec` khi dựng hộp thoại | Chỉ cho alias trong danh bạ đã `validate_alias`, timeout 5 s (GD7/GD19) |
| Ai đó (cùng UID) gọi thẳng broker với token của agent khác | Giới hạn đã biết của mô hình O2 (danh tính, không ngăn được cùng UID) — chấp nhận |

**Để sau:** tái dùng session cùng host (thay vì mở mới); G2b; kênh exec trực tiếp (G3); tự đóng session idle; danh sách "session do agent mở" trong Settings > Servers.

---

## 7. Tiến độ, quyết định, nhật ký

### Tiến độ

- [x] Plan v1 (2026-09-29)
- [x] 1.1 flag · [x] 1.2 model · [x] 1.3 store · [x] 1.4 ssh_config · [x] 1.5 directory/palette · [x] 1.6 review + clippy + format
- [x] ⛔ CHECKPOINT HA (người dùng test đạt, 2026-09-29)
- [x] 2.1 khuôn picker · [x] 2.2 palette · [x] 2.3 mở tab (Reconnect hoãn, GD13) · [x] 2.4 lên root (không làm, GD12) · [x] 2.5 review + clippy + format
- [x] ⛔ CHECKPOINT HB (người dùng test đạt, 2026-09-29)
- [x] 3.1 trang Settings · [x] 3.2 `warp_conf` · [x] 3.3 ghi + xác minh + khôi phục · [x] 3.4 `Include` · [x] 3.5 form + palette · [x] 3.6 xoá/quên · [x] 3.7 review + clippy + format
- [x] ⛔ CHECKPOINT HC (người dùng test đạt, 2026-09-30)
- [x] 4.1 hook mirror · [x] 4.2 `remote.host.list` · [x] 4.3 MCP `list_hosts` + CLI `remote hosts` · [x] 4.4 nút Open mirror · [x] 4.5 review + clippy + format
- [x] ⛔ CHECKPOINT HD (người dùng test đạt, 2026-09-30) · [x] tick G1 trong roadmap
- [ ] 5 (tùy chọn; gộp vào G2b)

**G2 — Agent tự mở session** (plan: mục G2 phía trên; người dùng làm trước gate O2, GD26)

- [x] Plan G2 v1 (2026-09-30)
- [x] G2a phase 1: 1.1 cờ · 1.2 protocol/catalog · 1.3 policy `[open]` · 1.4 registry `opened.rs` · 1.5 audit · 1.6 approval subject
- [x] G2a phase 2: 2.1 tab agent · 2.2 model + nghe `Sessions` · 2.3 handler open/close · 2.4 test handler (kể cả e2e với `mock_workspace`)
- [x] G2a phase 3: 3.1 tool MCP · 3.2 timeout client · 3.3 test
- [x] G2a phase 4: 4.1 wrapper subshell · 4.2 luồng root · 4.3 test
- [x] G2a phase 5: review + test + clippy + format
- [x] ⛔ CHECKPOINT H2A (người dùng test đạt, 2026-09-30)
- [ ] G2b (chỉ thiết kế xong; code sau khi người dùng xác nhận G2a)

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| GD1 | 2026-09-29 | Module mới `app/src/host_directory/`, flag `AgentOpsHosts` + cargo feature `agent_ops_hosts` | Dùng cho cả người và agent, không thuộc riêng Bridge; cùng mẫu flag `WarpSyncRemoteEdit` |
| GD2 | 2026-09-29 | G1 **không lưu bí mật**; mật khẩu SSH/sudo + askpass + tự điền tách ra phase 5 / G2. `auth` trong kho chỉ là ghi chú | Roadmap G1a nói lưu mật khẩu, nhưng chưa có consumer nào cho tới G2; lưu mà không dùng là rủi ro thuần (AO8 khó nhất) |
| GD3 | 2026-09-29 | `hosts.toml` hỏng thì **không ghi đè**, danh bạ rỗng trong RAM + toast | Khác `agents.toml` (chỉ nhãn): đây là dữ liệu người dùng nhập tay |
| GD4 | 2026-09-29 | Một dòng `Include config.d/warp.conf` đặt ở **đầu** `~/.ssh/config`, hỏi trước, backup, idempotent | `Include` sau một khối `Host` sẽ bị coi là thuộc khối đó; đầu file là chỗ duy nhất đúng |
| GD5 | 2026-09-29 | Alias theo `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`, gõ không quote; alias khác bị bỏ khi nhập và bị từ chối khi tạo | Alias đi vào shell của người dùng; cấm hẳn thay vì quote để không có option/command injection |
| GD6 | 2026-09-29 | Không dùng file watcher cho ssh config ở G1: làm mới lúc khởi động và khi mở picker/Settings (so mtime) | Đủ dùng, rẻ, ít bề mặt lỗi; watcher để sau |
| GD7 | 2026-09-29 | Giá trị kết nối chỉ lấy từ `ssh -G -- <alias>` chạy nền có timeout 5 s, cache trong RAM; chỉ cho alias hợp lệ | AO10; `ssh -G` chạy `Match exec` của chính người dùng — chấp nhận, ghi rõ |
| GD8 | 2026-09-29 | "Move to Warp" (chuyển khối sang `warp.conf`) để sau, không nằm trong 4 phase đầu | Đòi sửa khối do người dùng viết; rủi ro cao, giá trị thấp so với tag/tạo host |
| GD9 | 2026-09-29 | Host tạo trong Warp chỉ cho `HostName/User/Port/IdentityFile/ProxyJump`; cấm `ProxyCommand`/`LocalCommand`/`Match` | Ba cái đó chạy lệnh; form của Warp không được thành đường thực thi lệnh tuỳ ý qua ssh config |
| GD11 | 2026-09-29 | Không viết trước code chưa có nơi dùng (`validate_tag`, `is_connectable`, accessor của model): thêm ở phase cần | Clippy `-D warnings` chặn dead code; YAGNI. `RootLogin::RootLogin` đổi thành `Root` (clippy `enum_variant_names`), TOML `root_login = "root"` |
| GD12 | 2026-09-29 | G1 **không tự lên root** sau khi kết nối (bỏ task 2.4). Quick connect chỉ chạy `ssh <alias>`; người dùng tự `sudo -i` như luồng O1 | Không có cách kiểm chứng "session đã bootstrap" mà không chạy GUI; gõ thêm `sudo -i` vào shell trong lúc Warpify còn đang chạy dễ đua với nó; người dùng đang ngồi trước máy. Có thể thử `ssh -t <alias> sudo -i` (một lệnh) nếu người dùng muốn — chưa rõ Warpify SSH có bắt được dạng có lệnh từ xa không |
| GD13 | 2026-09-29 | Bỏ "Reconnect" khỏi G1 | Khi ssh rớt, shell local quay lại và Up+Enter chạy lại đúng lệnh; nhớ alias theo tab cần thêm trạng thái tab, giá trị thấp |
| GD14 | 2026-09-29 | Picker = filter `Servers` của palette Warp (theo khuôn Launch Configurations): `QueryFilter::Servers` (`warp_search_core`), `PaletteMode::Servers`, `CommandPaletteItemAction::ConnectToServer`, nguồn dữ liệu `search::command_palette::servers`; hàm tìm thuần `host_directory::search` (từ khoá mờ theo alias, `tag:x` lọc theo tag, từ khoá cũng khớp tag nhưng xếp sau alias) | Tìm mờ trên 454 host, bàn phím, không phải viết modal mới; mỗi lần mở picker quét lại ssh config nền (GD6) |
| GD15 | 2026-09-30 | Trang Settings > Servers là **danh sách + chi tiết của host đang chọn + form thêm**, không phải bảng sửa tại chỗ; sửa tag/`root_login`/`transport` ghi ngay (tag: Enter/rời ô), xoá/quên hai bước bấm (không dùng `confirm_dialog`) | Một bộ editor dùng chung cho host đang chọn thay vì một editor mỗi hàng của 454 host; hai bước bấm inline không cần modal |
| GD16 | 2026-09-30 | `Include` **không hỏi trước khi tạo host** mà hiện banner "Let ssh read Warp's servers" ngay khi có host `warp` mà `~/.ssh/config` chưa đọc `warp.conf`; chỉ bấm nút mới sửa file (có backup `config.warp-backup-<giây>`) | Tạo host chỉ ghi `warp.conf` (file của Warp) nên an toàn; việc duy nhất đụng file người dùng là dòng `Include`, và nó chỉ xảy ra khi bấm nút. Bỏ qua banner = từ chối, cảnh báo vẫn hiện |
| GD17 | 2026-09-30 | Chỉnh sửa (tạo/sửa/xoá) chạy nền, một việc một lúc (`busy`); quét ssh config và sửa tay có thể đua nhau ghi `hosts.toml` | Chấp nhận: cửa sổ đua rất nhỏ (thao tác đơn lẻ của người dùng); làm khoá nếu thực tế gặp |
| GD18 | 2026-09-30 | `remote.host.list` có params tùy chọn `query` (cú pháp picker: từ khoá mờ, `tag:x`) và `limit` (1–100, mặc định 50; MCP/CLI mặc định 20); kết quả có `total`. Plan 3.7 ghi "params None" | Danh bạ thật có 454 host: trả hết tốn context của agent và buộc chạy `ssh -G` hàng loạt (GD7). Không đổi `PROTOCOL_VERSION` (thêm action là additive; D12 chỉ nói về trường `agent` của params `remote.*`, `remote.session.list` cũng không có) |
| GD19 | 2026-09-30 | `ssh -G` của `remote.host.list` chạy nền, tuần tự, chung ngân sách 5 s; host chưa kịp giải hoặc `missing` thì `connection` vắng. Không có cache: mỗi lần gọi giải lại cho các host trả về | `ssh -G` chạy `Match exec` của người dùng: agent không được kích hoạt nó vô hạn. Cache thuộc `ServersSettingsPageView` (RAM của trang), không dùng chung được; tăng khi thực tế cần |
| GD20 | 2026-09-30 | Hook mirror nằm ở `WarpSyncModel` (nhánh thành công của `spawn_download`/`resume_upload`, mọi requester kể cả External/RemoteEdit): `RemoteShell::ssh_host()` (mặc định `None`) → `HostDirectoryModel::alias_for_ssh_host` (bỏ `user@`) → đọc manifest nền → `observe_mirror` đọc-sửa-ghi `hosts.toml`. `mirror_key` = tên thư mục mirror đã giải, `machine_id` = của manifest | Không thêm variant vào `WarpSyncEvent` (nhiều nơi match) và không đổi `DownloadOutcome`/`UploadOutcome`; chỉ thêm. `ssh_connection_info.host` cũng có ở `gcloud/eb/doctl … ssh`: chỉ khớp khi trùng alias trong danh bạ, chấp nhận |
| GD21 | 2026-09-30 | `mirror_link_update`: `machine_id` khác bản đã lưu → `Warn` **và ghi liên kết mới** (toast một lần, lần sau không lặp); host không báo `machine_id` không xoá id đã biết; cùng máy đổi tên mirror → `Set` lặng lẽ | Nếu không ghi thì `list_hosts` chỉ mãi mirror cũ của máy khác. Warp Sync vốn đã tách thư mục theo `machine_id`, G1 chỉ báo |
| GD22 | 2026-09-30 | `mirror_key` trong `hosts.toml` (sửa tay được) phải qua `warp_sync::is_mirror_key` (`[A-Za-z0-9._-]`, không bắt đầu `.`, ≤ 255) trước khi ghép đường dẫn (`host_directory::mirror_dir`), ở cả `list_hosts` lẫn nút Open mirror | Chặn `../…` thoát khỏi mirror root |
| GD23 | 2026-09-30 | "Path đã sync" = khoá `last_sync` của manifest (gốc của mỗi lần download/upload, đã sắp xếp, ≤ 200, có cờ cắt), không phải từng file | Từng file có thể hàng chục nghìn; gốc đồng bộ mới là thứ agent cần |
| GD24 | 2026-09-30 | Thêm CLI `warpctrl remote hosts [--query] [--limit]` | Test bảng catalog đòi mọi `ActionKind` (trừ `agent.pair`) có ví dụ CLI; chi phí nhỏ, dùng chung `render_hosts` với MCP. Catalog 98 action |
| GD25 | 2026-09-30 | Open mirror (4.4) chỉ là nút ở chi tiết host trong Settings, gọi thẳng `ViewContext::open_file_path_in_explorer` (cùng cách `WarpSyncOpenMirror`), **không** thêm `WorkspaceAction`/palette mới | `WarpSyncOpenMirror` lấy host từ session đang active nên không dùng được cho host chưa mở; palette cần chọn host, giá trị thấp |
| GD10 | 2026-09-29 | Nối session ↔ alias bằng `ssh_connection_info.host` (alias người dùng gõ) | Warp đã có sẵn; đúng cho cả quick connect lẫn `ssh` gõ tay; không cần đoán từ hostname |
| GD26 | 2026-09-30 | G2 làm **trước** gate O2 theo quyết định của người dùng, nên: `FeatureFlag::AgentOpsOpenSession` + cargo feature `agent_ops_open_session`, **không** vào `DOGFOOD_FLAGS`/`PREVIEW_FLAGS`/RELEASE; handler đòi đủ bốn cờ (`AgentBridge`, `AgentOpsPolicy`, `AgentOpsHosts`, `AgentOpsOpenSession`) | Roadmap ghi G2 chỉ sau gate O2 (AO7); người dùng chấp nhận rủi ro đổi lấy cờ riêng, pairing bắt buộc và mặc định hỏi |
| GD27 | 2026-09-30 | Hai action mới `remote.session.open` (target `Instance`) và `remote.session.close` (target `Session`), MCP `open_session`/`close_session`, **không** có subcommand `warpctrl` (loại khỏi test ví dụ CLI như `agent.pair`, P25 của O2); catalog 98 → 100; không đổi `PROTOCOL_VERSION` | Cần danh tính đã pair nên chỉ client MCP dùng được; thêm action là additive (cùng lý do GD18) |
| GD28 | 2026-09-30 | `open_session` **luôn** đòi `agent_token` đã xác minh (không phụ thuộc `require_pairing`); chưa pair → `policy_denied`. Chủ sở hữu session = `agent_id` đã pair; `close_session` cũng đòi pair | Không có danh tính đã xác minh thì "chỉ đóng session do chính agent mở" vô nghĩa (tên tự khai giả được) |
| GD29 | 2026-09-30 | Sự kiện "Warpify xong" = `SessionsEvent::SessionBootstrapped` của `Sessions` thuộc terminal view của tab vừa mở, do `AgentBridgeModel` subscribe qua `TerminalView::sessions_model()` (không sửa `terminal/view.rs`); khớp = cùng view + `WarpifiedRemote` + `ssh_connection_info.host` (bỏ `user@`) = alias; session Local đầu tiên bị bỏ qua. **Đính chính GD12**: sự kiện này có, cái thiếu là sự kiện thất bại | Đọc code (G2.1 Q1/Q2); attach ngay trong callback của sự kiện nên không có khoảng hở giữa "xong" và "attach" |
| GD30 | 2026-09-30 | Tab của agent **không cướp focus** (tạo rồi kích hoạt lại tab trước đó), tiêu đề `Agent · <alias>`, và khi `GroupedTabs` bật thì nằm trong nhóm `Agents` (tạo nếu chưa có, luôn mở rộng). Lệnh gõ vào tab giống hệt quick connect: `ssh <alias>` sau `validate_alias` (GD5). Code ở `workspace/view/agent_session_tab.rs` | Đang gõ dở mà tab đổi thì phím vào nhầm terminal (rủi ro thật khi tab đang hỏi passphrase); người dùng vẫn thấy tab và giành lại được. **Chưa kiểm chứng** tab nền bootstrap được (H2A.3): nếu không thì kích hoạt tab mới |
| GD31 | 2026-09-30 | Registry `OpenedSessions` trong `AgentBridgeModel` (RAM, khoá `PaneId`, giữ `agent_id`, alias, access, purpose, trạng thái). Giới hạn: mặc định 2 session/host, 4/agent, trần cứng 8/host, 16/agent, 16 tổng; đếm cả mục `Connecting`; mục có pane đã biến mất bị dọn khi mở/đóng. Agent thấy `session_id` = pane id (như `list_sessions`) | Khớp cách client đang địa chỉ hoá session; RAM là đủ vì attach cũng chỉ ở RAM (D4) |
| GD32 | 2026-09-30 | `wait_secs` mặc định 60 (1–180). Hết giờ chưa Warpify → **không** lỗi mà trả `status: connecting` kèm `session_id`; mục giữ trạng thái chờ tới `PENDING_TTL` 10 phút và vẫn tự attach nếu Warpify xong muộn (người dùng gõ passphrase), sau đó `Abandoned` (không attach nữa, vẫn tính vào giới hạn tới khi pane đóng, `close_session` đóng được). Kiểm trước khi mở tab: `enable_ssh_warpification` bật, alias không nằm trong `is_ssh_host_denylisted` (nếu không, lỗi rõ thay vì chờ vô ích) | Không có sự kiện thất bại (GD29) nên thời gian là tín hiệu duy nhất; tab thấy được nên người dùng xử lý được |
| GD33 | 2026-09-30 | Policy `[open]`: `max_sessions_per_host`, `max_sessions_per_agent`, `[[open.hosts]] { match (glob alias), tag?, mode = allow|ask|deny, max_access = read_only|full, allow_root }`; luật đầu tiên khớp thắng; **không luật ⇒ Ask**, không đọc `[defaults]`; `allow` mà access xin > `max_access` hoặc xin root khi `allow_root = false` ⇒ Ask (không hạ mức lặng lẽ); file lỗi ⇒ Deny. `[defaults]`/`[[hosts]]`/`[deny]` không đổi và vẫn quyết định từng lệnh sau khi attach | Người dùng yêu cầu "host chưa có quy tắc là HỎI"; tự attach chỉ mở cửa, không thay quyết định ghi |
| GD34 | 2026-09-30 | Duyệt mở session dùng lại `ApprovalQueue`/dialog/toast persistent (P28) với `ApprovalSubject::OpenSession` (`session: None`); một `object_id` toast chung `agent_ops_open_request`; tối đa 3 request mở chờ mỗi agent; dialog chỉ Approve/Deny; `connection` (`user@host:port`) lấy bằng `ssh -G` nền ≤ 5 s | Tái dùng hạ tầng đã test (P4/P28); người duyệt cần thấy host thật trước khi đồng ý |
| GD35 | 2026-09-30 | Audit `remote.session.open`/`close` thêm `purpose` và `access` (bỏ khi rỗng); `started` ghi **trước** khi mở tab (fail-closed như P13); `host` = alias, `session_id` = pane id khi đã có | Truy vết được "agent nào mở gì, vì sao" |
| GD36 | 2026-09-30 | Lên root (`root: true`) chỉ khi `root_login = sudo_nopasswd`, shell bash/zsh/fish và `is_compatible_subshell_command("sudo -i")`; gõ đúng `sudo -i` bằng wrapper `pub(crate)` quanh `set_and_execute_subshell_command`; thành công khi có `SessionBootstrapped` thứ hai (`spawning_command == "sudo -i"`), khi đó attach chuyển sang session root. Không đủ điều kiện ⇒ giữ session user + `elevation: skipped` + lý do. Không gõ mật khẩu ở G2a | Đọc code (G2.1 Q7): có cơ chế xác định thay vì gõ mù vào shell đang Warpify; bỏ GD12 "không tự lên root" chỉ ở phạm vi `open_session` |
| GD37 | 2026-09-30 | Bước nền (`stage`) **không** đọc policy và **không** chạy `ssh -G` cho agent chưa pair; ở luồng chính `Deny` của policy được báo trước giới hạn số session | Không để client chưa pair kích hoạt `Match exec` của ssh config; lý do từ chối cụ thể hơn |
| GD38 | 2026-09-30 | Nhóm `Agents` mới nằm **đầu** danh sách tab chưa ghim (như `create_new_tab_group`), nên chỉ số tab đang active của người dùng dịch đi 1 dù vẫn đúng tab đó; test so bằng `EntityId` của pane group, không so chỉ số | Phát hiện lúc viết test e2e: khẳng định "không cướp focus" phải theo danh tính tab, không theo chỉ số (ghi để H2A.3 không hiểu nhầm) |
| GD39 | 2026-09-30 | `close_session` bắt buộc selector session dạng `Id` (không nhận `Active`), kiểm chủ sở hữu bằng `agent_id`, đóng bằng `PaneGroup::close_pane` (cách `pane.close`), rồi gỡ mục registry và attach; code ở `handlers/close_session.rs` (tách khỏi `open_session.rs` cho file < 800 dòng) | Không thể đóng nhầm tab đang active; khớp mô hình xác thực GD28 |
| GD40 | 2026-09-30 | Test e2e chạy handler thật (`open`/`close`) trên `mock_workspace`, `$HOME` tạm (`#[serial]`) chứa `policy.toml`, `agents.toml`, `~/.ssh/config`, `hosts.toml`; sự kiện Warpify được **giả lập** bằng `Sessions::initialize_bootstrapped_session` trên `Sessions` của tab thật | Kiểm chứng toàn bộ dây nối (policy → duyệt → tab → subscribe → attach → trả lời → đóng) mà không cần ssh thật; phần ssh/Warpify thật để H2A |

### Nhật ký

- 2026-09-29 — Plan v1 (Claude Sonnet 5.5) sau khảo sát code (mục 2). Chưa code.
- 2026-09-29 — Phase 1 (Claude Sonnet 5.5). File mới `app/src/host_directory/`: `model.rs` (`Host`, `HostSource`,
  `AuthMethod`, `RootLogin`, `Transport`, `validate_alias`, `merge_discovered`, `MergeReport`), `store.rs`
  (`hosts.toml` `parse`/`serialize`/`load`/`save`, atomic + `0600`, file hỏng không bị ghi đè),
  `ssh_config.rs` (`discover_aliases`: tokenizer kiểu OpenSSH, `Host`, `Include` tương đối/`~`/glob, chống vòng,
  giới hạn độ sâu/file/kích thước), `directory.rs` (`HostDirectoryModel` singleton, `refresh_blocking`,
  `refresh_message`) + test `*_tests.rs`. Đổi: flag `AgentOpsHosts` (`warp_features`, DOGFOOD) + cargo feature
  `agent_ops_hosts` (`app/Cargo.toml`, `features.rs`), `lib.rs` (mod + singleton), `WorkspaceAction::AgentOpsImportSshHosts`
  + palette "Agent Ops: Import hosts from SSH config", `Workspace` subscribe model + toast ở cửa sổ đang active.
  Chạy thử parser **chỉ đọc** trên `~/.ssh/config` thật: 454 alias / 14 file / 0 cảnh báo, khớp script Python độc lập
  (test tạm đã xoá). Lệnh: `cargo nextest run -p warp --features warp_sync,agent_bridge,agent_ops_hosts -E
  'test(/host_directory::|agent_bridge::|warp_sync::/)'` → 729/729 pass (68 test host_directory); `cargo clippy -p warp
  --features warp_sync,agent_bridge,agent_ops_hosts --all-targets --tests -- -D warnings` sạch; `cargo check -p warp`
  (không feature) sạch; `./script/format`. Dừng ở ⛔ CHECKPOINT HA.
- 2026-09-29 — CHECKPOINT HA: người dùng test đạt.
- 2026-09-29 — Phase 2 (Claude Sonnet 5.5). 2.1: chọn filter của palette làm picker (GD14; `FilterableDropdown` cần neo
  trong modal, không hợp với lệnh palette). File mới: `host_directory/search.rs` (+ test), `search/command_palette/servers/`
  (`DataSource`, `SearchItem`). Đổi: `QueryFilter::Servers` (`warp_search_core`), `PaletteMode::Servers`,
  `CommandPaletteItemAction::ConnectToServer` + `ItemSummary::Server`, `data_sources.rs` (thêm nguồn khi `AgentOpsHosts`
  bật), `zero_state.rs`, `filter_chip_renderer.rs`, `view.rs` của palette (`is_mode_enabled`, mở filter từ binding, nhận
  `ConnectToServer`), `HostDirectoryModel::hosts()`, `WorkspaceAction::AgentOpsConnectToServer { alias }` + binding "Agent
  Ops: Connect to server…" (mở palette ở filter Servers), `Workspace::open_servers_palette` (quét lại nền khi mở) và
  `agent_ops_connect_to_server` (kiểm lại `validate_alias`, mở tab qua `PaneTemplateType::PaneTemplate` chạy `ssh <alias>`).
  2.4/Reconnect: xem GD12, GD13. Lệnh: `cargo nextest run -p warp --features warp_sync,agent_bridge,agent_ops_hosts -E
  'test(/host_directory::|command_palette/)'` → 102/102 pass (gồm 12 test `search`); `cargo clippy -p warp -p warp_search_core
  --features … --all-targets --tests -- -D warnings` sạch; `cargo check -p warp` sạch; `./script/format`. Chưa chạy GUI: dừng
  ở ⛔ CHECKPOINT HB.
- 2026-09-29 — CHECKPOINT HB: người dùng test đạt.
- 2026-09-30 — Phase 3 (Claude Sonnet 5.5). File mới `host_directory/`: `warp_conf.rs` (hàm thuần: `NewHost`, `validate`
  — chỉ HostName/User/Port/IdentityFile/ProxyJump, không cho ProxyCommand/LocalCommand/Match, chặn xuống dòng, `-` đầu, ký tự lạ —,
  `render_block`, `parse_blocks`, `append_block`, `remove_block`, `set_tags`, `has_warp_include`, `with_warp_include`),
  `ssh_resolve.rs` (`parse_ssh_g`, trait `SshResolver`, `SystemSsh` chạy `ssh -G -- alias` timeout 5 s), `provision.rs`
  (`create_host`: từ chối alias đã có ở ssh config/danh bạ/warp.conf, ghi atomic `0600` + `warp.conf.bak`, xác minh bằng `ssh -G -F warp.conf`
  rồi khôi phục nếu lệch; `update_host`, `remove_host`, `install_include` giữ nguyên mọi byte khác + quyền file + đi qua symlink, backup
  `config.warp-backup-<giây>`), `search::search_all` (host `missing` vẫn hiện trong Settings). UI: `settings_view/servers_page.rs`
  + `servers_page_widgets.rs` (`SettingsSection::Servers`, banner Include, danh sách tìm được, chi tiết host, form thêm), palette
  "Agent Ops: Add or manage servers…" mở trang. Sửa lỗi lúc làm: `validate` cũ tách tag theo khoảng trắng nên `"bad tag"` lọt qua
  (nay kiểm từng tag). Lệnh: `cargo nextest run -p warp --features warp_sync,agent_bridge,agent_ops_hosts -E 'test(/host_directory::|settings_view::|command_palette/)'`
  → 437/437 pass (riêng `host_directory::` 134 test, gồm provision/warp_conf/ssh_resolve/servers_page/search_all); `cargo clippy -p warp --features … --all-targets --tests
  -- -D warnings` sạch; `cargo check -p warp` (không feature) sạch; `./script/format`. Chưa chạy GUI: dừng ở ⛔ CHECKPOINT HC.
- 2026-09-30 — CHECKPOINT HC: người dùng test đạt.
- 2026-09-30 — Phase 4 (Claude Sonnet 5.5). **4.1** file mới `host_directory/mirror.rs` (`MirrorLink`, `MirrorUpdate {Set, Warn, Keep}`, hàm thuần `mirror_link_update`, `apply_observation` đọc-sửa-ghi `hosts.toml`, `alias_of_ssh_host`, `mirror_dir`); `HostDirectoryModel::{alias_for_ssh_host, observe_mirror}` (toast qua `Notice`); `RemoteShell::ssh_host()` (mặc định `None`, `SessionShell` đọc `subshell_info().ssh_connection_info.host`); hook `link_host_mirror` + `read_mirror_link` trong `warp_sync/model.rs` (GD20–21). Không đổi hành vi Warp Sync. **4.2** `ActionKind::RemoteHostList` (`remote.host.list`, Instance, params `RemoteHostListParams`, kết quả `RemoteHostListResult`/`RemoteHostSummary`…) ở `local_control` (catalog, protocol, `ActionParameterSpec::RemoteHostList`), `app/src/local_control/handlers/hosts.rs` (cùng cổng `ensure_enabled` = Agent Bridge như `remote.session.list`, thêm cờ `AgentOpsHosts`), `remote::listed_sessions` (tách từ `session_list`, hành vi cũ giữ nguyên), `warp_sync::{synced_paths, is_mirror_key}` (GD18, 19, 22, 23). **4.3** MCP `list_hosts` (`tools.rs`, `format.rs::render_hosts`, dòng trong `INSTRUCTIONS`) + CLI `warpctrl remote hosts` (GD24); kết quả không có trường nào chứa bí mật (test khoá bộ khoá JSON). **4.4** nút "Open mirror of this server" trong chi tiết host (`servers_page.rs`, `servers_page_widgets.rs`; GD25). Test mới: `mirror_tests` (12+2), `hosts_tests` (19), 5 test e2e `host_list_*` qua `handle_control_request` trong `remote_tests.rs`, 2 `read_mirror_link` (`warp_sync/model_tests.rs`), `is_mirror_key`/`synced_paths`, `tools_tests`/`format_tests` cho `list_hosts`, cập nhật đếm catalog (98) và bảng ví dụ CLI. Lệnh: `cargo nextest run -p warp_cli -p local_control -p warp --lib --features warp/warp_sync,warp/warp_control_cli,warp/agent_bridge,warp/agent_ops_policy,warp/agent_ops_hosts -E 'package(warp_cli) | package(local_control) | test(/warp_sync::|local_control::|host_directory::/)'` → 1081/1081 pass (build `-p warp_cli` đơn lẻ hỏng vì `fontconfig.pc`, như Bridge 7.4); `cargo nextest run -p warp --lib --features warp_control_cli,warp_sync,agent_bridge,agent_ops_policy,agent_ops_hosts -E 'test(/warp_sync::|local_control::|host_directory::|settings_view::/)'` → 935/935 pass; `cargo clippy -p warp -p local_control -p warp_cli --features warp/warp_control_cli,warp/warp_sync,warp/agent_bridge,warp/agent_ops_policy,warp/agent_ops_hosts --all-targets --tests -- -D warnings` sạch; `cargo check -p warp` (không feature) sạch; `./script/format` một lần ở cuối. Chưa chạy GUI/MCP thật, chưa tick G1 trong ROADMAP: dừng ở ⛔ CHECKPOINT HD.
- 2026-09-30 — CHECKPOINT HD: người dùng test đạt. G1 hoàn tất (Phase 1–4); đã tick G1 trong `ROADMAP.md`. Phase 5 (bí mật) vẫn tùy chọn, gộp vào G2.
- 2026-09-30 — Plan G2 v1 (Claude Sonnet 5.5) sau khảo sát code (bảng G2.1, Q1–Q7): đọc `ROADMAP.md`, plan G1/O2/Bridge, `bridge.rs`, `handlers/{remote,agent,hosts,close,layout}.rs`,
  `agent_bridge/{model,attachments,approval,approval_dialog,policy,audit,ops}.rs`, `terminal/model/session.rs`, `terminal/view.rs` (Warpify, subshell), `terminal/ssh/util.rs`,
  `workspace/view.rs` + `view/tab_grouping.rs`, `pane_group/mod.rs`, `mcp/tools.rs`, catalog/protocol và test đếm; đọc thêm (chỉ đọc) `~/.config/warp-oss/settings.toml`, `~/.warp/agent-ops/`.
  Kết quả chính: có sự kiện `SessionBootstrapped` đáng tin (đính chính GD12), nối tab ↔ session bằng subscribe `Sessions` của đúng view, cơ chế lên root xác định (`is_compatible_subshell_command` +
  sự kiện thứ hai). Quyết định GD26–GD36. Chưa code.
- 2026-09-30 — G2a phase 1–5 (Claude Sonnet 5.5). **1.1** `FeatureFlag::AgentOpsOpenSession` (`warp_features`) + cargo feature `agent_ops_open_session`
  (`app/Cargo.toml`, `features.rs`), không vào DOGFOOD/PREVIEW/RELEASE; hằng số `OPEN_*` trong `agent_bridge/mod.rs`. **1.2** `local_control`: `RemoteSessionOpenParams/Result`,
  `RemoteSessionCloseParams/Result`, `RemoteOpenStatus`, `RemoteOpenElevation`, `RemoteAccess: Default`, `ActionKind::RemoteSessionOpen/Close` + specs + `resolver.rs`; catalog 100 (`protocol_tests`
  `REMOTE_ACTIONS` 9, `mod_tests` 100); test ví dụ CLI loại trừ hai action như `agent.pair` (GD27). **1.3** `policy.rs`: `[open]` (`max_sessions_per_*`, `[[open.hosts]]`),
  `Policy::evaluate_open`/`open_limits`, `invalid_policy_reason` (dùng chung với `remote.rs`); 15 test mới. **1.4** `agent_bridge/opened.rs` (registry thuần: giới hạn, `on_bootstrapped`, `expire`,
  `elevation_plan`, `give_up_elevation`) + 25 test. **1.5** `audit.rs` thêm `purpose`/`access`; `open_audit.rs` (`SessionRequestAudit`, fail-closed) + 6 test. **1.6** `ApprovalSubject::OpenSession`,
  `persistent_toast_id` (thay `toast_is_persistent`), `OPEN_TOAST_ID`, giới hạn 3 request chờ mỗi agent, `has_pending_open`, nội dung hộp thoại + test. **2.1** `workspace/view/agent_session_tab.rs`
  (`Workspace::open_agent_session_tab`: tab `Agent · <alias>`, nhóm `Agents`, không cướp focus) + 4 test. **2.2** `AgentBridgeModel`: `opened`, `open_waiters`, `track_open` (subscribe `Sessions`),
  `follow_open`, `forget_opened`, `check_open_limits`, `opened_by`, `OpenReady` + 7 test. **2.3** `handlers/open_session.rs` (luồng: kiểm tham số → host trong danh bạ → Warpify bật → `stage` nền → policy/giới hạn →
  duyệt → audit `started` → mở tab → chờ Ready) và `handlers/close_session.rs`; `bridge.rs` thêm hai arm; `hosts.rs`/`remote.rs` chỉ mở visibility (`resolve_connection`, `resolve_agent_id`). **2.4** 37 test
  (`open_session_tests.rs`: 25 test thuần + 12 test e2e qua `mock_workspace`, GD40). **3.1–3.3** `mcp/tools.rs` (`open_session`, `close_session`, `INSTRUCTIONS`, `OPEN_CLIENT_MARGIN`), `format.rs::render_open/render_close`,
  `specs/agent-bridge/claude/SKILL.md`, test `tools_tests`/`format_tests`. **4.1–4.3** `TerminalView::execute_subshell_command_and_warpify` (patch duy nhất vào `terminal/view.rs`), luồng `Elevating` trong
  `follow_open`, test thuần + 2 test e2e. Đã tìm và sửa lúc test: focus (GD38), `HostDirectoryModel` bị làm mới từ `$HOME` tạm ghi đè danh bạ giả (test dựng `~/.ssh/config`), `retain_live`
  bỏ luôn `open_waiters`, `follow_open` gọi `expire` trước khi khớp TTL. Lệnh: `cargo nextest run --no-fail-fast -p warp --lib --features warp/agent_ops_policy,warp/agent_bridge,warp/agent_ops_hosts,warp/agent_ops_open_session
  -E 'test(/open_session|close_session|agent_bridge::/)'` → 356/356; `cargo nextest run --no-fail-fast -p warp_cli -p local_control -p warp --lib --features warp/warp_sync,warp/warp_control_cli,warp/agent_bridge,
  warp/agent_ops_policy,warp/agent_ops_hosts,warp/agent_ops_open_session -E 'package(warp_cli) | package(local_control) | test(/open_session|close_session/)'` → 489/489 (lần đầu 1766/1767, sửa
  `every_tool_has_a_schema_and_the_list_matches_the_dispatch`); cùng lệnh rộng hơn (`warp_sync::|local_control::|host_directory::|agent_bridge::|agent_session_tab|settings_view::|command_palette`) 1767 test trước sửa
  đó; `cargo clippy -p warp -p local_control -p warp_cli --features warp/warp_control_cli,warp/warp_sync,warp/agent_bridge,warp/agent_ops_policy,warp/agent_ops_hosts,warp/agent_ops_open_session --all-targets --tests
  -- -D warnings` sạch; `cargo check -p warp` (không feature) sạch; `./script/format` một lần ở cuối. Chưa chạy GUI/ssh thật, không commit, không tick G2 trong `ROADMAP.md`.
  Điều chưa kiểm chứng (dành cho H2A): tab nền có bootstrap ssh không (H2A.3), Warpify có hỏi trước khi cài extension không (H2A.10), đóng pane cuối của tab có hỏi xác nhận không (H2A.7), `sudo -i` gõ bằng
  wrapper có tự Warpify không (H2A.16). Dừng ở ⛔ CHECKPOINT H2A.
- 2026-09-30 — Viết lại checklist G2.4 (H2A) thành hướng dẫn từng bước: mục chuẩn bị P0–P7 (chọn host `home-docker-02`/`home-docker-03`, hàm `aud`/`setopen`, sao lưu policy, đăng ký MCP `--no-pair`, cách ra lệnh cho Claude, nơi quan sát) và H2A.1–H2A.16 theo khuôn Làm / Kỳ vọng / Ghi lại / Dọn, câu lỗi lấy từ code. Chỉ sửa tài liệu, không đổi code; vẫn dừng ở ⛔ CHECKPOINT H2A.
- 2026-09-30 — H2A.3 lần đầu: tab `Agent · home-pi-01` Warpify xong nhưng không tự attach. Log chẩn đoán (`Agent session tab …`) cho thấy `remote: true, ssh host: None`: session đăng nhập qua **SSH wrapper** (ControlMaster) không có `subshell_info`, nên `ssh_connection_info.host` rỗng và `on_bootstrapped` bỏ qua (GD10 chỉ đúng cho đường subshell; Q2 trong G2.1 đã nhầm). Sửa: `opened::typed_ssh_host` lấy host từ subshell nếu có, không thì phân tích `spawning_command` (chính dòng `ssh <alias>` đã gõ) bằng `parse_interactive_ssh_command`; 3 test mới; giữ `log::info!` trong `follow_open`. Còn nghi vấn cùng gốc: `RemoteShell::ssh_host` của Warp Sync (`warp_sync/remote_shell.rs`) cũng đọc `subshell_info` nên trả `None` với session wrapper. Chờ người dùng chạy lại H2A.3. Cũng nên đặt `warpify.ssh.ssh_extension_install_mode = "never_install"` trước khi test (hộp thoại extension chặn bootstrap).
- 2026-09-30 — H2A.3 lần hai: vẫn `ssh host: None`. Session wrapper không mang host ở đâu cả: `init_shell` gọi `reinit_shell` nên `spawning_command` của nó không phải dòng `ssh <alias>`, và socket ControlMaster chỉ đặt theo `WARP_SESSION_ID`. Đổi `on_bootstrapped`: session remote **không có** host ⇒ coi là kết quả của `ssh <alias>` Warp gõ (session remote đầu tiên của tab khi còn `Connecting`); có host thì vẫn phải khớp alias. Rủi ro còn lại (chấp nhận): ssh của Warp thất bại rồi người dùng tự `ssh` host khác trong chính tab agent trước khi hết `OPEN_PENDING_TTL` ⇒ session đó được attach với mức agent xin; mọi lệnh ghi vẫn qua `authorize`. Test `an_ssh_session_to_another_host_is_ignored` bỏ nhánh không-host, thêm `a_session_through_the_ssh_wrapper_is_attached_without_a_host`. `agent_bridge::|open_session` 360/360, clippy sạch, format.
- 2026-09-30 — CHECKPOINT H2A: người dùng test đạt (sau hai lần sửa khớp session SSH wrapper ở trên). Việc ngoài G2 cùng lúc: `autoupdate::start_polling` không chạy vòng poll trên kênh `Local`/`Integration`/`Oss` (`channel_ships_updates`), vì `release_bundle` bật `Autoupdate` trong bản build local và mỗi lần poll ghi lỗi "don't support autoupdate". Hộp thoại GNOME "Remote Desktop / Allow Remote Interaction" chỉ do `computer_use` (AI agent của Warp dùng máy tính trên Wayland) mở trong Warp; chưa xác định tiến trình nào gọi, chờ người dùng bắt bằng `dbus-monitor`. Chưa tick G2 trong `ROADMAP.md` (G2b còn chờ).
