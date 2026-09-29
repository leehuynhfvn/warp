# G1 — Danh bạ server — Implementation plan (v1)

> Viết ngày 2026-09-29 (Claude Sonnet 5.5), theo `ROADMAP.md` mục G và quyết định AO9–AO13. Plan này
> chỉ chi tiết **G1**; G2–G5 giữ ở mức phác trong roadmap, sẽ viết thêm vào file này khi tới lượt
> (G2+ chỉ sau gate O2, AO7). Người dùng giao Claude quyết định thiết kế (AO13); người dùng chỉ làm
> các ⛔ CHECKPOINT test tay.

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
- [ ] 5 (tùy chọn)

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
