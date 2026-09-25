# Agent Bridge — Claude Code điều khiển session SSH đã Warpify — Implementation Plan (v1)

> Người thực thi: một coding agent (Claude Sonnet hoặc Gemini 3.1 Pro qua Antigravity CLI).
> Làm **tuần tự từng Phase**, dừng ở mọi **CHECKPOINT** để người dùng xác nhận. Không tự ý mở
> rộng phạm vi. Plan viết ngày 2026-09-24 bởi Claude Opus sau khi khảo sát code thật (mục 1.2).

---

## 0. Quy tắc bắt buộc cho agent thực thi

1. Đọc `AGENTS.md` trước. Skill liên quan trong `.agents/skills/`: `add-feature-flag`,
   `gui-ui-guidelines`, `rust-unit-tests`, `logging-and-error-reporting`. Đọc `SKILL.md` tương ứng
   **trước** khi làm task dùng tới nó.
2. Làm việc trong **worktree riêng** (Phase 0), không đụng checkout `/projects/github/warp` — ở đó
   một agent khác đang làm Warp Sync với thay đổi chưa commit.
3. Mọi lệnh cargo chạy với `export CARGO_TARGET_DIR=/projects/github/warp/target` (dùng lại cache
   build). Nếu cargo báo "Blocking waiting for file lock" thì chờ, **không** xoá lock.
4. **Sau MỖI task** chạy `cargo check` cho các package đã sửa (`-p warp`, `-p local_control`,
   `-p warp_cli`) và sửa hết lỗi trước khi sang task sau. Không commit code không compile.
5. Không `unwrap()`/`expect()` trên dữ liệu từ remote, filesystem, client local-control hoặc input
   của user. Không `let _ =` để nuốt lỗi IO. Không trả thành công khi thao tác thất bại.
6. Match exhaustive, không dùng `_` nếu tránh được. `ctx` là tham số cuối. Không prefix `_` cho
   tham số thừa — xoá hẳn. Comment chỉ giải thích "why". Format args inline (`{e}`).
7. Unit test đặt trong `<name>_tests.rs`, include cuối module bằng
   `#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;`.
8. Lệnh kiểm tra (`nextest` chưa cài trên máy này):
   - `cargo test -p local_control`, `cargo test -p warp_cli`,
     `cargo test -p warp --lib agent_bridge`, `cargo test -p warp --lib local_control`.
   - Cuối mỗi Phase: `cargo clippy -p warp -p local_control -p warp_cli --all-targets --tests -- -D warnings`.
   - `./script/format` chỉ chạy **một lần** ở Task 4.3. **Không** chạy `./script/presubmit`.
9. Commit theo conventional commits (`feat(agent-bridge): ...`), mỗi task một commit nhỏ.
10. Gặp API không tồn tại / chữ ký khác plan → tìm pattern tương tự trong code; vẫn mơ hồ thì
    **dừng và hỏi**, không bịa API. Số dòng trong plan là gần đúng — tìm theo tên hàm.
11. File này là **nguồn sự thật duy nhất** của feature. Bắt đầu phiên: đọc mục 8. Sau mỗi task đã
    commit: tick checkbox + thêm 1 dòng "Nhật ký" ở mục 8 (commit cùng task). Lệch plan → ghi vào
    "Quyết định" kèm lý do. Không tạo thêm file memory/context khác (không dùng `.context/`).

---

## 1. Bài toán và kết quả khảo sát

### 1.1 Mục tiêu

Người dùng quản trị server bằng Warp: `ssh user@host` → `sudo -i` (nhập mật khẩu một lần) →
Warpify subshell → gõ lệnh bằng input hiện đại của Warp. Họ muốn **Claude Code** (đang chạy ở máy
local, với skills/hooks/memory/MCP của họ) làm việc **trực tiếp trên server đó, trong chính session
đó, với đúng quyền của shell đó** (root sau `sudo -i`): chạy lệnh chẩn đoán, đọc/sửa file cấu hình,
kiểm tra config, reload service — mỗi thao tác thay đổi đều được người dùng duyệt.

Không cài gì lên server. Không đặt credential Anthropic lên server. Không cần `NOPASSWD` sudo.

### 1.2 Hiện trạng code (đã kiểm chứng)

| Thành phần có sẵn | Ý nghĩa với feature | Bằng chứng |
|---|---|---|
| **Local control** (`warpctrl`): discovery record owner-only → Unix socket broker cấp credential ngắn hạn theo **đúng 1 action** (kiểm tra peer UID) → HTTP loopback `127.0.0.1` có bearer → bridge chạy trên main thread. Bật mặc định ở channel Local/Dev (`WarpControlCli` nằm trong `DOGFOOD_FLAGS`; `bin/local.rs` bật `DOGFOOD_FLAGS`). | Kênh điều khiển an toàn từ process bên ngoài vào Warp — **dùng lại, không dựng server mới**. | `app/src/local_control/mod.rs` (doc đầu file), `crates/local_control/src/{catalog,auth,discovery,client}.rs`, `app/src/settings/local_control.rs` |
| Catalog action định nghĩa bằng macro `define_action_catalog!`; bridge `match` exhaustive trên `ActionKind`. | Thêm action `remote.*` theo cùng khuôn. | `crates/local_control/src/catalog.rs`, `app/src/local_control/bridge.rs` |
| `input.insert` **cố ý từ chối** newline/control char. | Local control hiện **không** cho chạy lệnh — feature này mở rộng quyền đáng kể ⇒ cần cổng đồng ý riêng (mục 2.3). | `app/src/local_control/handlers/app_state.rs::validate_staged_input_text` |
| `LocalControlBridge::handle_request` **đồng bộ**; HTTP handler `await` `bridge_spawner.spawn(...)`. | Cần thêm nhánh bất đồng bộ cho lệnh chạy lâu (mục 3.5). | `app/src/local_control/mod.rs::handle_control_request` |
| Client dùng `reqwest::blocking::Client::new()` — **timeout mặc định 30 s**. | Cần biến thể có timeout dài cho `remote.exec`. | `crates/local_control/src/client.rs::send_request` |
| `Session::execute_command` chạy lệnh qua executor của session: sau `sudo -i` + Warpify là `InBandCommandExecutor` — lệnh chạy **trong shell root**, trong subshell nền (`eval ... &`), output hex-encode qua OSC. | Kênh chạy với quyền root — giống quyết định D1 của Warp Sync. Lệnh không làm đổi `cd`/`export` của shell người dùng. | `app/src/terminal/model/session.rs::execute_command`, `app/assets/bundled/bootstrap/bash_body.sh::_warp_execute_command` |
| PTY controller **huỷ ngay** lệnh in-band nếu người dùng đang chạy lệnh foreground. | Trả lỗi `SessionBusy` rõ ràng, không treo. | `app/src/terminal/writeable_pty/pty_controller.rs::queue_in_band_command` |
| Warp Sync đã có `posix_quote`, `wrap_for_any_shell`, `upload_begin_command`, `validate_tmp_dir`, `upload_chunk_commands`, `cleanup_command`, `normalize_remote_path` (đã test, gồm test chạy `sh` thật). | Dùng lại để build script remote và upload nội dung file. | `app/src/warp_sync/{remote_script,paths}.rs` (nhánh `feature/warp-sync`) |
| Agent Mode của Warp đọc file remote qua **daemon remote-server** (chạy dưới user SSH, không phải root). | Feature này làm tốt hơn: đọc/ghi file qua shell root. | `app/src/ai/blocklist/action_model/execute/read_files.rs` |
| `secret_redaction::redact_secrets(&mut String)`; regex **rỗng cho tới khi được set** (`set_user_and_enterprise_secret_regexes`), mặc định ở `regexes::DEFAULT_REGEXES_WITH_NAMES`. | Che secret trước khi gửi cho model; process `warpctrl` phải tự nạp regex mặc định. | `crates/secret_redaction/src/lib.rs` |
| `rmcp` 1.6 có feature `server`/`transport-io`, nhưng repo chỉ dùng phía client. | v1 tự viết MCP stdio server nhỏ, đồng bộ (quyết định D7). | `Cargo.toml` (`rmcp = { version = "1.6" }`), `crates/mcp/Cargo.toml` |
| Binary local: `target/debug/warp` (bin `warp` = channel Local). `warp --warpctrl <args>` vào chế độ `warpctrl`, không mở GUI. | Claude Code gọi MCP server bằng `…/warp --warpctrl mcp`. | `app/src/lib.rs::run` (đoạn `from_control_mode_env`), `app/Cargo.toml [[bin]]` |

### 1.3 Các phương án đã cân nhắc

| Phương án | Ưu | Nhược | Kết luận |
|---|---|---|---|
| **A. Claude Code local + Agent Bridge qua Warp** (plan này) | Không cài gì lên server; không lộ credential; tự động có quyền root của `sudo -i`; dùng nguyên skills/memory/hooks của Claude Code; Warp kiểm soát đồng ý + audit; dùng được cho mọi MCP client (Gemini CLI, Codex). | Kênh in-band chậm hơn SSH thẳng; lệnh không tương tác; cần code. | **Chọn** |
| B. Cài `claude` lên server, chạy trong `sudo -i` | Chạy được ngay hôm nay (Warp đã có CLI-agent toolbar/rich input cho Claude Code). | Credential + binary trên từng server prod; agent root không qua lớp đồng ý nào của Warp; mỗi server cấu hình riêng. | Chỉ hợp cho 1 máy dev |
| C. Agent Mode của Warp chọn model Claude | Có sẵn. | Không phải Claude Code (không có skills/hooks/MCP/memory của user); đọc file remote dưới user SSH. | Không đáp ứng |
| D. Claude Code tự `ssh host 'sudo …'` bằng Bash tool | Không cần code. | Cần `NOPASSWD` sudo hoặc lộ mật khẩu; không dùng lại session Warp. | Không đáp ứng |

---

## 2. Kiến trúc

### 2.1 Sơ đồ

```
 Máy local                                                          Server
┌──────────────────────────────────────────────────────────────┐
│ Claude Code (VSCode/terminal)                                │
│   └─ MCP stdio ─► `warp --warpctrl mcp`   (warp_cli, Phase 3)│
│                      │ 1. đọc discovery record (owner-only)  │
│                      │ 2. Unix socket broker → credential    │
│                      │    ngắn hạn cho đúng 1 action         │
│                      │ 3. POST 127.0.0.1/v1/control          │
│                      ▼                                       │
│ Warp app ─ LocalControlBridge (main thread)                  │
│   ├─ kiểm tra: flag, Scripting, credential, target           │
│   ├─ AgentBridgeModel: session có được ATTACH không? quyền?  │
│   └─ ctx.spawn(ops::exec/read/write) ─► Session::execute_command
│                                           │                  │
│        pane: ssh user@host → sudo -i (Warpified subshell)    │
│                                           └── InBand ──PTY──►│ root shell: sh script (nền)
└──────────────────────────────────────────────────────────────┘
```

### 2.2 Luồng một lệnh `remote.exec`

1. MCP tool `exec {session_id, command}` → adapter gửi `RequestEnvelope{action: remote.exec,
   target.session = Id(pane_id)}` bằng client có timeout = `timeout_secs + 30 s`.
2. Bridge (main thread): các kiểm tra sẵn có → resolve pane → `TerminalView` → `ActiveSession` →
   `Arc<Session>` → `AgentBridgeModel::check(session.id(), Access::Full, now)`.
3. `ctx.spawn(ops::exec(session, params).with_timeout(..), callback)`; trả `BridgeResult::Pending(rx)`.
4. HTTP handler `await rx` trên runtime tokio của control server (main thread không bị chặn).
5. `ops::exec` build script (mục 3.6), gọi `session.execute_command`, parse output có khung nonce,
   ghi audit log, trả JSON (exit code, stdout/stderr đã cắt head+tail).
6. Callback trên main thread: `record_use` (cập nhật TTL, đếm lệnh), gửi kết quả qua oneshot.

### 2.3 Mô hình an toàn (cốt lõi của feature)

| Lớp | Cơ chế |
|---|---|
| Có sẵn | Feature flag `WarpControlCli` + Settings > Scripting; broker kiểm UID; credential theo đúng action, TTL 5 phút; HTTP chỉ loopback, chặn `Origin` trình duyệt. |
| Flag mới | `AgentBridge` (Phase 2). Tắt flag → mọi `remote.*` trả `UnsupportedAction`. |
| **Đồng ý theo session** | Chỉ session được người dùng **chủ động Attach** qua Command Palette mới nhận `remote.*`. Attach gắn với **`SessionId`** của session active lúc attach: `exit` khỏi `sudo -i` → session active đổi → tự mất quyền; vào lại `sudo -i` phải attach lại. Chỉ lưu trong RAM (khởi động lại Warp = mất hết). |
| Mức quyền | `Full` (exec + read + write) hoặc `ReadOnly` (chỉ read + list). |
| Hết hạn | Không dùng quá `ATTACH_IDLE_TTL` (30 phút) → tự huỷ. Có "Revoke all" làm kill switch. |
| Chỉ session remote | v1 chỉ nhận `SessionType::WarpifiedRemote { .. }` (không yêu cầu `host_id`). Session local → lỗi (Claude Code đã có Bash local). |
| Target tường minh | `remote.exec/read/write` **bắt buộc** `target.session = Id(..)`; từ chối `Active`/thiếu — tránh chạy nhầm vào pane đang focus. |
| Không ghi mù | `remote.file.write` bắt buộc `MustNotExist` hoặc `MustMatch{sha256}` (optimistic concurrency). Luôn backup trước khi ghi đè. |
| Duyệt từng thao tác | Do **Claude Code** đảm nhiệm (permission prompt cho MCP tool). Hướng dẫn user chỉ allowlist tool chỉ-đọc (mục 3.11). |
| Riêng tư | MCP adapter che secret (regex mặc định) trong mọi text gửi cho model; `edit_file` tính toán ở adapter nên nội dung thô không tới model. |
| Truy vết | Audit log JSONL local, quyền `0600`: mọi request `remote.*` (lệnh đầy đủ, path, exit code, thời gian, user@host). Không ghi output. |

### 2.4 Ngoài phạm vi v1

Chạy lệnh dạng block **hiển thị** trên terminal (như Agent Mode), lệnh tương tác (vim/top/sudo hỏi
mật khẩu), PowerShell, Windows (broker dùng Unix socket), front-end TUI, pairing token, duyệt từng
lệnh ở phía Warp, nhiều file/thư mục trong một lệnh write, stream output theo thời gian thực.

---

## 3. Thiết kế chi tiết

### 3.1 Action mới trong catalog (`crates/local_control/src/catalog.rs`, group mới `remote`)

| `ActionKind` | Tên | Target | Params (`ActionParameterSpec`) | Result (`ActionResultSpec`) | Quyền attach |
|---|---|---|---|---|---|
| `RemoteSessionList` | `remote.session.list` | `Instance` | `None` | `RemoteSessionList` | không cần |
| `RemoteExec` | `remote.exec` | `Session` | `RemoteExec` | `RemoteExecResult` | `Full` |
| `RemoteFileRead` | `remote.file.read` | `Session` | `RemoteFileRead` | `RemoteFileContent` | `ReadOnly` trở lên |
| `RemoteFileWrite` | `remote.file.write` | `Session` | `RemoteFileWrite` | `RemoteFileWriteResult` | `Full` |
| `RemoteOutputRecent` (Phase 4) | `remote.output.recent` | `Session` | `RemoteOutputRecent` | `RemoteOutputRecent` | `ReadOnly` trở lên |

Phase 1 khai báo với `status: Stub` (bridge trả `UnsupportedAction`); Task 2.5 đổi sang
`Implemented`. Thêm vào `resolver.rs::validate_action_params` / `validate_action_target` theo
compiler. `remote.session.list` không có selector (liệt kê mọi window/tab/pane).

### 3.2 Params / Response (`crates/local_control/src/protocol.rs`, `#[serde(deny_unknown_fields)]`)

```rust
pub struct RemoteExecParams {
    pub command: String,                 // 1..=MAX_COMMAND_BYTES, không chứa '\0'
    #[serde(default)] pub cwd: Option<String>,        // None = cwd hiện tại của session
    #[serde(default)] pub timeout_secs: Option<u32>,  // None = 120; tối đa 600
}
pub struct RemoteFileReadParams { pub path: String }
pub struct RemoteFileWriteParams {
    pub path: String,
    pub content_base64: String,
    pub expectation: WriteExpectation,
}
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WriteExpectation { MustNotExist, MustMatch { sha256: String } } // sha256: 64 hex thường
pub struct RemoteOutputRecentParams { #[serde(default)] pub count: Option<u32> } // Phase 4
```

Response (`data` của `ResponseEnvelope::ok`), mọi response có `host`, `user`, `session_id`:

- `remote.session.list` → `{"sessions": [{session_id (= pane_id), window_index, tab_index, pane_index,
  is_active, session_type: "remote"|"local", host, user, shell, cwd,
  attached: null | {access: "full"|"read_only", idle_secs, expires_in_secs, exec_count}}]}`.
- `remote.exec` → `{cwd, exit_code: i32|null, timed_out, duration_ms,
  stdout: {text, total_bytes, truncated}, stderr: {text, total_bytes, truncated}}`.
- `remote.file.read` → `{"status": "ok", path, size, sha256, content_base64}` hoặc
  `{"status": "not_found", path}`. (`not_found` không phải lỗi — adapter cần nó để tạo file mới.)
- `remote.file.write` → `{path, bytes, sha256, backup_path: string|null, created: bool}`.

### 3.3 `ErrorCode` mới (`protocol.rs`)

Thêm: `SessionNotAttached`, `SessionBusy`, `Timeout`, `RemoteOperationFailed`. Dùng lại:
`InsufficientPermissions` (attach `ReadOnly` mà gọi exec/write), `TargetStateConflict` (sha không
khớp, file đã tồn tại), `InvalidParams`, `MissingTarget`, `InvalidSelector`, `UnsupportedAction`.
Message phải hướng dẫn được cho model, ví dụ `SessionNotAttached`: "Session 12 (root@prod-1) is not
attached. Ask the user to run 'Agent Bridge: Allow Claude Code to control this session' from the
Warp command palette in that pane."

### 3.4 Attach registry

`app/src/agent_bridge/attachments.rs` (thuần, test được, thời gian truyền vào):

```rust
pub enum Access { ReadOnly, Full }
pub struct Attachment { access: Access, user: String, host: String,
                        attached_at: Instant, last_used: Instant, exec_count: u32 }
pub struct Attachments { by_session: HashMap<SessionId, Attachment> }
impl Attachments {
    pub fn attach(&mut self, id: SessionId, access: Access, user: String, host: String, now: Instant);
    pub fn detach(&mut self, id: SessionId) -> bool;
    pub fn detach_all(&mut self) -> usize;
    /// Xoá entry hết hạn (lazy) rồi kiểm tra mức quyền.
    pub fn check(&mut self, id: SessionId, needed: Access, now: Instant) -> Result<&Attachment, AgentBridgeError>;
    pub fn record_use(&mut self, id: SessionId, is_exec: bool, now: Instant);
    pub fn get(&self, id: SessionId, now: Instant) -> Option<&Attachment>; // không trả entry hết hạn
}
```

`app/src/agent_bridge/model.rs`: `AgentBridgeModel` (SingletonEntity, `type Event = ()`) bọc
`Attachments`; đăng ký trong `app/src/lib.rs` cạnh
`ctx.add_singleton_model(remote_server::manager::RemoteServerManager::new)`.

### 3.5 Nhánh bất đồng bộ trong bridge

`app/src/local_control/bridge.rs`:

```rust
pub(super) enum BridgeResult {
    Ready(ResponseEnvelope),
    Pending { request_id: Uuid,
              receiver: futures::channel::oneshot::Receiver<Result<serde_json::Value, ControlError>> },
}
```

- `handle_request` trả `BridgeResult`; mọi arm hiện có bọc `Ready(..)` (giữ nguyên logic kiểm tra ở
  đầu hàm). Arm `RemoteExec | RemoteFileRead | RemoteFileWrite` gọi `handlers::remote::start(..)`
  trả `Result<Receiver, ControlError>`.
- `mod.rs::handle_control_request`: `Ready(r)` → như cũ; `Pending` → `receiver.await`:
  `Ok(Ok(data))` → ok, `Ok(Err(e))` → error, `Err(Canceled)` → `BridgeUnavailable`.
- Test hiện có gọi `handle_request` trực tiếp → thêm helper `#[cfg(test)] fn expect_ready(self)`.
- Trong handler: `ctx.spawn(future, move |_, result, ctx| { record_use...; send })`. Khi gửi qua
  oneshot thất bại (client đã ngắt) → `log::debug!`, không panic.
- Timeout Rust = `timeout_secs + EXEC_TIMEOUT_GRACE` bằng `warpui_core::r#async::FutureExt::with_timeout`
  (xem cách dùng trong `crates/remote_server/src/ssh.rs`).

### 3.6 Script remote (`app/src/agent_bridge/script.rs`)

Nguyên tắc chung (giống Warp Sync mục 3.4): script **POSIX sh**, bọc bằng
`warp_sync::remote_script::wrap_for_any_shell`; mọi giá trị động qua `posix_quote`; script luôn
`exit 0` và báo kết quả qua **dòng khung** `M='@@WARP-AGENT-<nonce>@@'` (nonce = 16 hex ngẫu nhiên mỗi
request, dùng `uuid`/`rand` sẵn có). Vì `sh` đọc script từ stdin, lệnh người dùng **luôn** chạy với
`</dev/null`. Parser đọc `CommandOutput.stdout`; nếu không thấy khung thì thử `stderr` (executor khác
nhau đặt output khác nhau); không thấy khung → `RemoteOperationFailed("unexpected output: <≤200 byte cuối>")`.

**Exec** — `exec_script(nonce, command, cwd: Option<&str>, timeout_secs) -> String`:

```sh
M='@@WARP-AGENT-<nonce>@@'
T=$(mktemp -d "${TMPDIR:-/tmp}/warp-agent.XXXXXX") || { echo "$M fatal mktemp"; exit 0; }
cd <q cwd> 2>/dev/null || { rm -rf "$T"; echo "$M fatal cwd"; exit 0; }   # chỉ khi có cwd
export PAGER=cat GIT_PAGER=cat SYSTEMD_PAGER=cat NO_COLOR=1 DEBIAN_FRONTEND=noninteractive
if command -v bash >/dev/null 2>&1; then S=bash; else S=sh; fi
if command -v timeout >/dev/null 2>&1; then
  timeout <secs> "$S" -c <q command> </dev/null >"$T/o" 2>"$T/e"; rc=$?; TO=1
else
  "$S" -c <q command> </dev/null >"$T/o" 2>"$T/e"; rc=$?; TO=0
fi
emit() {
  n=$(wc -c < "$2" | tr -d ' ')
  echo "$M $1 $n"
  if [ "$n" -le $3 ]; then cat "$2"; else head -c $4 "$2"; printf '\n%s cut\n' "$M"; tail -c $4 "$2"; fi
  printf '\n%s end\n' "$M"
}
emit stdout "$T/o" <STDOUT_MAX> <STDOUT_MAX/2>
emit stderr "$T/e" <STDERR_MAX> <STDERR_MAX/2>
echo "$M rc $rc $TO"
rm -rf "$T"
exit 0
```

Parse → `ExecOutput { stdout: Stream, stderr: Stream, exit_code: i32, timed_out: bool }` với
`Stream { text: String /*from_utf8_lossy*/, total_bytes: u64, truncated: bool }`. Quy tắc khung:
nội dung section = byte sau dòng header tới trước `"\n$M cut\n"` / `"\n$M end\n"` (bỏ đúng 1 `\n`
do script thêm). Có `cut` → `text = head + "\n… [N bytes omitted] …\n" + tail`, `truncated = true`.
`timed_out = TO == 1 && rc == 124`. `fatal cwd` → `InvalidParams("cwd does not exist")`.

**Read** — `read_script(nonce, path)`: in `$M user $(id -un)`; kiểm tra lần lượt `-e` → `status
not_found`, `-f` → `status not_regular`, `-r` → `status permission_denied`, có `base64` không →
`status missing_base64`; `n=$(wc -c < "$P")` → `$M size $n`; `n > READ_MAX_FILE_BYTES` → `status
too_large`; sau đó `echo "$M status ok"`, `base64 < "$P"`, `printf '\n%s end\n' "$M"`.
Rust: decode base64 (bỏ whitespace), kiểm tra `len ≤ READ_MAX_FILE_BYTES`, **tự tính sha256** từ byte
đã decode (không phụ thuộc `sha256sum` khi đọc).

**Write** — `ops::write_file` dùng lại upload của Warp Sync:
`upload_begin_command()` → `validate_tmp_dir()` → `upload_chunk_commands(tmp, bytes)` (tuần tự) →
`write_commit_script(..)` → parse; lỗi sau `begin` → chạy `cleanup_command(tmp)` best-effort rồi trả
lỗi gốc. `write_commit_script(nonce, tmp, path, expectation, expected_len, backup_name)`:

```sh
M=…; T=<q tmp>; P=<q path>; F="$T/<PAYLOAD_FILE_NAME>"
h() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1;
      elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1; fi; }
fail() { rm -rf "$T"; echo "$M error $1"; exit 0; }
[ "$(wc -c < "$F" | tr -d ' ')" = "<len>" ] || fail size_mismatch
# --- MustNotExist ---
if [ -e "$P" ]; then fail already_exists; fi
[ -d "$(dirname "$P")" ] || fail parent_missing
cp "$F" "$P" || fail write_failed
echo "$M created 1"
# --- MustMatch{sha} ---
[ -e "$P" ] || fail not_found
[ -f "$P" ] || fail not_regular
cur=$(h "$P"); [ -n "$cur" ] || fail missing_sha256
[ "$cur" = "<sha>" ] || fail changed_on_server
B="$HOME/.warp-agent/backups"; mkdir -p "$B" && chmod 700 "$B" || fail backup_failed
cp -p "$P" "$B/<backup_name>" || fail backup_failed
echo "$M backup $B/<backup_name>"
cat "$F" > "$P" || fail write_failed
# --- chung ---
rm -rf "$T"
echo "$M sha256 $(h "$P")"
echo "$M status ok"
```

- Ghi đè **tại chỗ** (`cat > "$P"`), không `mv` file tạm: giữ inode, owner, mode, ACL, nhãn SELinux,
  hardlink; ghi qua symlink vào file đích (đúng kỳ vọng với `sites-enabled/*`). Payload đã nằm đủ
  trên server nên cửa sổ ghi dở rất nhỏ, và đã có backup (dòng `backup` được in **trước** khi ghi để
  thông báo lỗi `write_failed` kèm được đường dẫn backup). Xem D6.
- `backup_name` = `<basename đã sanitize [A-Za-z0-9._-], khác → _>.<YYYYmmddTHHMMSS>.<4 hex>`.
- Map lỗi: `already_exists`/`changed_on_server` → `TargetStateConflict`; `not_found`,
  `not_regular`, `parent_missing`, `missing_sha256`, `backup_failed`, `write_failed`,
  `size_mismatch` → `RemoteOperationFailed` với message tiếng Anh dễ hiểu.

**Path**: mọi path đi qua `warp_sync::paths::normalize_remote_path(input, Some(cwd_của_session))`
(từ chối rỗng, `\0`, `\n`, `~`, `..`, `/`, `/proc|/sys|/dev|/run`). `cwd` của `remote.exec` cũng
phải tuyệt đối, không `\0`/`\n`.

### 3.7 Hằng số (`app/src/agent_bridge/mod.rs`)

| Constant | Giá trị | Lý do |
|---|---|---|
| `MAX_COMMAND_BYTES` | `8 * 1024` | Script bọc base64 được gõ qua line editor của shell |
| `EXEC_DEFAULT_TIMEOUT_SECS` / `EXEC_MAX_TIMEOUT_SECS` | `120` / `600` | In-band chỉ chạy khi shell rảnh |
| `EXEC_TIMEOUT_GRACE` | `15 s` | Rust chờ lâu hơn `timeout` phía remote |
| `EXEC_STDOUT_MAX_BYTES` / `EXEC_STDERR_MAX_BYTES` | `24 KiB` / `8 KiB` | Giữ tool result dưới giới hạn token MCP của Claude Code |
| `READ_MAX_FILE_BYTES` | `512 KiB` | Base64 + hex qua PTY; file lớn hơn → dùng `exec` với `tail`/`grep` |
| `WRITE_MAX_BYTES` | `512 KiB` | ~45 chunk 16 KiB |
| `ATTACH_IDLE_TTL` | `30 min` | Giảm rủi ro quên revoke |
| `AUDIT_LOG_MAX_BYTES` | `10 MiB` | Xoay vòng sang `.1` |

### 3.8 Audit log (`app/src/agent_bridge/audit.rs`)

- File `dirs::home_dir()/.warp/agent-bridge/audit.jsonl`; thư mục `0700`, file `0600` (`#[cfg(unix)]`).
- Mỗi request `remote.exec/read/write` một dòng JSON:
  `{ts_unix, action, session_id, host, user, cwd?, command?, path?, exit_code?, result: "ok"|"error", error_code?, duration_ms, bytes?}`.
- Ghi trong future nền sau khi xong (không ghi trên main thread). File > `AUDIT_LOG_MAX_BYTES` →
  `rename` thành `audit.jsonl.1` (ghi đè bản cũ) rồi tạo mới. Lỗi ghi audit → `log::warn!`, không
  làm hỏng request.
- Log thường (`log`/`safe_*`) theo skill `logging-and-error-reporting`: phần `safe` **không** chứa
  lệnh/path/host.

### 3.9 CLI `warpctrl remote` (`crates/warp_cli/src/local_control/`)

```
warpctrl remote sessions
warpctrl remote exec  --session <ID> [--cwd DIR] [--timeout SECS] -- <COMMAND...>
warpctrl remote read  --session <ID> <PATH>
warpctrl remote write --session <ID> <PATH> --from <LOCAL_FILE> (--expected-sha256 <SHA> | --create)
```

- Dùng lại `TargetArgs`/`instance_selector`/`target_selector` sẵn có (kiểm tra flag `--session`
  đã tồn tại chưa; tránh trùng tên).
- `exec`: nối `COMMAND...` bằng dấu cách; in `stdout`→stdout, `stderr`→stderr; **exit code của CLI
  = exit code lệnh remote** (timeout → 124). `read`: in nội dung đã decode ra stdout (byte thô).
  `--output-format json` → in nguyên `data`.
- `local_control::client`: thêm `send_request_with_timeout(instance, request, timeout: Duration)`;
  `send_request` gọi lại nó với hành vi cũ (không đổi timeout hiện tại).

### 3.10 MCP server `warpctrl mcp` (Phase 3, `crates/warp_cli/src/local_control/mcp/`)

**Giao thức** (tự viết, đồng bộ, không tokio): stdio, mỗi message JSON-RPC 2.0 trên **một dòng**.
- `initialize` → `{protocolVersion, capabilities: {tools: {listChanged: false}}, serverInfo: {name:
  "warp-agent-bridge", version}, instructions}`. `protocolVersion`: trả lại bản client gửi nếu thuộc
  `["2025-06-18", "2025-03-26", "2024-11-05"]`, ngược lại `"2025-06-18"`.
- `notifications/*` → bỏ qua (không trả lời). `ping` → `{}`. `tools/list`. `tools/call`.
- Method lạ → error `-32601`; JSON hỏng → `-32700` (id null); params sai → `-32602`.
- Lỗi của tool → **result** `{content: [{type: "text", text}], isError: true}` (không phải JSON-RPC error).
- **stdout chỉ chứa message**, flush sau mỗi dòng; log → stderr. Xử lý tuần tự (in-band cũng tuần tự).
- Tham số dòng lệnh: `--instance <ID>` (mặc định: đúng 1 instance, như `select_instance`),
  `--no-redact`.

**`instructions`** (tiếng Anh, ngắn): exec/read/write chạy trên **server remote** trong session Warp
của user (thường là root) — khác Bash tool chạy ở máy local; luôn `list_sessions` trước và nói rõ
`user@host` trước khi thay đổi gì; ưu tiên `read_file` + `edit_file` cho file cấu hình; kiểm tra cú
pháp trước khi reload (`nginx -t`, `sshd -t`, `visudo -c`, `apachectl configtest`,
`systemd-analyze verify`); không restart sshd/network/firewall khi chưa được user xác nhận rõ; lệnh
không tương tác (stdin = /dev/null) nên dùng `-y`/`--no-pager`; timeout mặc định 120 s, tối đa 600 s;
output dài bị cắt head/tail — dùng `grep`/`tail`/`head`.

**Tools** (`session_id` luôn tuỳ chọn: bỏ trống → dùng session **duy nhất** đang attach; 0 hoặc
≥ 2 → lỗi kèm danh sách):

| Tool | Input | Hành vi | `annotations` |
|---|---|---|---|
| `list_sessions` | — | `remote.session.list`, in bảng gọn: id, user@host, cwd, attached/access | `readOnlyHint: true` |
| `exec` | `command`, `cwd?`, `timeout_secs?`, `session_id?` | `remote.exec`; text: `exit_code: N (1.2s) root@host:/cwd` + `--- stdout ---` + `--- stderr ---` (+ ghi chú cắt/timeout) | `destructiveHint: true` |
| `read_file` | `path`, `offset?` (dòng, từ 1), `limit?` (mặc định 2000), `session_id?` | `remote.file.read`; UTF-8 bắt buộc (không → lỗi "binary file"); định dạng `cat -n` như Read tool của Claude Code, mỗi dòng cắt 2000 ký tự, text trả về ≤ 80 KiB; ghi nhớ `(session, path) → sha256` | `readOnlyHint: true` |
| `write_file` | `path`, `content`, `session_id?` | Đọc thô trước: `not_found` → `MustNotExist`; tồn tại → bắt buộc đã `read_file` và sha khớp bản đã nhớ (không khớp → "Read the file first / it changed"), **và** nội dung thô không chứa secret (nếu redaction làm thay đổi nội dung thô → từ chối, bảo dùng `edit_file`) → `MustMatch` | `destructiveHint: true` |
| `edit_file` | `path`, `old_string`, `new_string`, `replace_all?`, `session_id?` | Đọc thô → `apply_edit` (0 match → lỗi, thêm gợi ý nếu `old_string` chứa `********`; > 1 match và `!replace_all` → lỗi nêu số match; `old == new` → lỗi) → `MustMatch{sha của bản đọc}`; trả số lần thay, backup path, đoạn ±3 dòng quanh thay đổi đầu tiên (đã redact) | `destructiveHint: true` |
| `recent_output` (Phase 4) | `count?` (mặc định 3, tối đa 10), `session_id?` | `remote.output.recent`: lệnh + exit code + output (cắt 16 KiB/block) các block gần nhất — để Claude "nhìn thấy" lỗi user vừa gặp | `readOnlyHint: true` |

**Redaction**: lúc khởi động (trừ `--no-redact`) compile `secret_redaction::regexes::DEFAULT_REGEXES_WITH_NAMES`
thành `regex::Regex` rồi `set_user_and_enterprise_secret_regexes(&compiled, [])` (xem
`app/src/ai/agent_sdk/driver/terminal_tests.rs` ~dòng 74). Mọi text gửi model đi qua **một** hàm
`to_model_text()`. `edit_file`/`write_file` so khớp trên nội dung **thô** — nội dung thô không bao giờ
nằm trong tool result.

**Testability**: trait `ControlTransport { fn call(&self, action, params, target, timeout) -> Result<Value, ControlError>; }`;
bản thật dùng discovery + `send_request_with_timeout`; test dùng fake.

### 3.11 Thiết lập Claude Code (đưa vào toast "Copy setup command" + mục 7)

```bash
claude mcp add --scope user warp-bridge -- /projects/github/warp/target/debug/warp --warpctrl mcp
```

`~/.claude/settings.json` — chỉ tự động cho phép tool chỉ-đọc; `exec`/`write_file`/`edit_file`
luôn hỏi:

```json
{ "permissions": { "allow": [
  "mcp__warp-bridge__list_sessions", "mcp__warp-bridge__read_file", "mcp__warp-bridge__recent_output"
] } }
```

---

## 4. Các Phase

### Phase 0 — Chuẩn bị worktree

1. Từ `/projects/github/warp`:
   `git worktree add ../warp-agent-bridge -b feature/agent-bridge feature/warp-sync`
   (tách từ **commit** mới nhất của `feature/warp-sync` để dùng lại `warp_sync::{remote_script,paths}`;
   thay đổi chưa commit của Warp Sync không đi theo — đúng ý).
2. Chuyển plan: `mkdir -p ../warp-agent-bridge/specs/agent-bridge && mv specs/agent-bridge/IMPLEMENTATION_PLAN.md ../warp-agent-bridge/specs/agent-bridge/ && rmdir specs/agent-bridge`.
3. Mọi bước sau làm trong `/projects/github/warp-agent-bridge` với `CARGO_TARGET_DIR` như mục 0.3.
4. Commit plan: `docs(agent-bridge): add implementation plan`.
5. Verify: `git status` sạch; `cargo check -p warp` pass.

### Phase 1 — Protocol + logic thuần (không UI, không async) + unit test

**Task 1.1 — Protocol.** Trong `crates/local_control/src/`: action mục 3.1 (status `Stub`), params +
`WriteExpectation` + `ActionParameterSpec`/`ActionResultSpec` mới (mục 3.2), `ErrorCode` mới (3.3),
`send_request_with_timeout` (3.9). Cập nhật `app/src/local_control/resolver.rs` (params/target) và
arm tạm trong `bridge.rs` trả `UnsupportedAction` cho action `Stub`; sửa các match exhaustive khác
theo compiler (`warp_cli` `output.rs`/`commands.rs`, …).
Test (`protocol_tests.rs`/`catalog` test sẵn có): serde round-trip từng params; `deny_unknown_fields`;
`WriteExpectation` JSON đúng `{"type":"must_match","sha256":…}`; tên action duy nhất.
Verify: `cargo test -p local_control`, `cargo check -p warp -p warp_cli`.

**Task 1.2 — Mở visibility Warp Sync (chỉ đổi visibility).** `app/src/warp_sync/mod.rs`:
`pub(crate) mod paths; pub(crate) mod remote_script;`; `remote_script.rs`:
`pub(crate) const PAYLOAD_FILE_NAME`. Không sửa logic. Verify: `cargo check -p warp`.

**Task 1.3 — Module.** `app/src/agent_bridge/mod.rs` (constants 3.7, `pub(crate) mod` con) +
`error.rs`: `AgentBridgeError` (`thiserror`, Display là message cho model/user):
`NotRemoteSession`, `UnsupportedShell`, `NotAttached { user, host }`, `AttachmentExpired`,
`ReadOnlyAttachment`, `SessionBusy`, `Timeout { secs }`, `InvalidParams(String)`,
`Conflict(String)`, `RemoteFailed(String)`, `Executor(String)`, `UnexpectedOutput(String)`,
`Io(String)` + `impl From<AgentBridgeError> for ControlError` (map theo 3.3) + `From<WarpSyncError>`.
Khai báo `mod agent_bridge;` trong `app/src/lib.rs` cạnh `mod warp_sync;` (tạm
`#[allow(dead_code)]`, gỡ ở Task 2.5).

**Task 1.4 — `script.rs` + `script_tests.rs`.** API: `new_nonce()`, `exec_script`, `parse_exec_output`,
`read_script`, `parse_read_output -> ReadOutcome { NotFound, Ok { bytes, sha256, user } }` (các
status lỗi → `AgentBridgeError`), `write_commit_script`, `parse_write_output`, `backup_name(path, now)`.
Test tối thiểu:
- Script chỉ chứa lệnh/path ở dạng đã quote: lệnh độc `'; rm -rf / #`, `$(id)`, `` `id` ``, `'`.
- `wrap_for_any_shell(exec_script(..))` khớp regex `^printf %s [A-Za-z0-9+/=]+ \| base64 -d \| sh$`.
- Parser: đủ section; section rỗng; nội dung không có `\n` cuối; có `cut`; thiếu `rc` → lỗi;
  rác trước khung bị bỏ qua; khung nằm ở `stderr` thay vì `stdout`.
- `#[cfg(unix)]` chạy thật `sh -c <exec_script>`: `echo out; echo err >&2; exit 3` → rc 3 và 2
  stream đúng; `seq 1 200000` → `truncated`, `total_bytes` đúng; `read x; echo got=$x` trả ngay
  (stdin /dev/null); nếu có `timeout`: `sleep 5` với timeout 1 → `timed_out`; `cd /tmp` trong lệnh
  không ảnh hưởng process test.
- `#[cfg(unix)]` read: file thường / không tồn tại / thư mục / `chmod 000` (bỏ qua assert nếu chạy
  bằng root) / vượt `READ_MAX_FILE_BYTES` (tham số hoá giới hạn trong test).
- `#[cfg(unix)]` write commit với `HOME` = thư mục tạm (set env cho process con): `MustNotExist` tạo
  file; file đã có → `already_exists`; `MustMatch` sai sha → file **không đổi**, không backup;
  đúng sha → nội dung mới, backup có nội dung cũ, **mode file giữ nguyên** (`chmod 640` trước).

**Task 1.5 — `attachments.rs` + tests.** Mục 3.4. Test: attach/check Full vs ReadOnly; hết TTL →
`AttachmentExpired` và entry bị xoá; `record_use` gia hạn; `detach_all` trả số lượng; session khác
→ `NotAttached`.

**Task 1.6 — `audit.rs` + tests.** Mục 3.8, hàm `append(dir: &Path, record: &AuditRecord)` (dir
truyền vào để test bằng `tempfile`). Test: tạo thư mục/file đúng quyền (unix); mỗi record một dòng
JSON hợp lệ; xoay vòng khi vượt giới hạn (tham số hoá).

**Kết thúc Phase 1:** test + clippy pass. Commit.

### Phase 2 — Nối vào app + CLI (dùng được qua `warpctrl`)

**Task 2.1 — Feature flag `AgentBridge`.** Theo skill `add-feature-flag`: variant trong
`crates/warp_features/src/lib.rs`, thêm vào `DOGFOOD_FLAGS` (build `./script/run` dùng bin `warp` =
Local, đã bật `DOGFOOD_FLAGS`). Verify: `cargo test -p warp_features`.

**Task 2.2 — `ops.rs` (async, không UI).**
`pub async fn exec(session: Arc<Session>, params: RemoteExecParams, audit_dir: Option<PathBuf>) -> Result<Value, AgentBridgeError>`,
tương tự `read_file`, `write_file`.
- Kiểm tra: `session.session_type()` là `WarpifiedRemote { .. }` (không đòi `host_id`), shell
  PowerShell → `UnsupportedShell`; validate params (3.2, 3.6) **trước** khi chạy gì. Tham khảo
  `warp_sync::remote_shell::SessionShell` (kiểm tra session/shell + `with_timeout`) — nhưng không
  dùng thẳng `RemoteShell::run` vì nó coi exit ≠ 0 là lỗi, còn script của bridge luôn `exit 0`.
- `run(session, cmd)`: `session.execute_command(&cmd, None, None, ExecuteCommandOptions::default())`;
  lỗi executor do bị huỷ (user đang chạy lệnh foreground) → `SessionBusy`; lỗi khác → `Executor`.
  Tìm cách phân biệt "cancelled" trong `in_band_command_executor.rs`; nếu không phân biệt được thì
  map mọi lỗi executor sang `SessionBusy` với message nêu cả 2 khả năng — ghi vào "Quyết định".
- `cwd` mặc định = cwd hiện tại của session (đọc trên main thread ở handler, truyền vào params).
- Audit: ghi sau khi xong (thành công hay lỗi).

**Task 2.3 — `model.rs`: `AgentBridgeModel`** (mục 3.4) + đăng ký singleton trong `lib.rs`.

> **Cập nhật 2026-09-25:** Task 2.4 và hàm `send_request_with_timeout` (mục 3.9) được làm ở **Warp Sync
> Phase 7.1** (`feature/warp-sync`, xem `specs/warp-sync/IMPLEMENTATION_PLAN.md`, D15). Trước khi bắt đầu
> Agent Bridge: rebase `feature/agent-bridge` lên `feature/warp-sync`, kiểm lại Task 1.2 (module
> `warp_sync` đã đổi nhiều ở Phase 4–6: `paths`, `transfer`, `.git`/baseline) và **bỏ qua Task 2.4** nếu
> `BridgeResult::Pending` đã có.

**Task 2.4 — Nhánh async của bridge** (mục 3.5) + sửa test `app/src/local_control/mod_tests.rs`.
Verify: `cargo test -p warp --lib local_control`.

**Task 2.5 — Handler `app/src/local_control/handlers/remote.rs`.**
- Helper trong `handlers/metadata.rs`: `pub(crate) fn resolve_terminal_view_for_session(target, action, ctx) -> Result<ViewHandle<TerminalView>, ControlError>`
  dựa trên `select_pane_entries` (target chỉ có `session: Id` → duyệt mọi window/tab) + lọc
  `pane_id.to_string() == id` + `pane_group.terminal_view_from_pane_id(..)` (xem `input_text` ở
  `handlers/app_state.rs`). `target.session` khác `Id` → `InvalidSelector`.
- `session_list(ctx)` (đồng bộ): duyệt như `metadata::session_list` với target mặc định, bổ sung
  thông tin từ `terminal_view.active_session().as_ref(ctx).session(ctx)` (`user()`, `hostname()`,
  `session_type()`, `shell()`), `current_working_directory()` và `AgentBridgeModel::get`.
- `start(kind, request, ctx) -> Result<Receiver, ControlError>`: flag tắt → `UnsupportedAction`;
  resolve session; `check(access)`; `ctx.spawn(ops::…)`.
- Đổi status 4 action sang `Implemented`; gỡ `#[allow(dead_code)]` của `agent_bridge`.

**Task 2.6 — Command Palette + toast.** Thêm `WorkspaceAction::{AgentBridgeAttach { read_only: bool },
AgentBridgeRevoke, AgentBridgeRevokeAll, AgentBridgeCopySetupCommand}`; binding trong
`app/src/workspace/mod.rs` bọc `if FeatureFlag::AgentBridge.is_enabled()` (theo pattern
`register_editable_bindings` sẵn có), tên `workspace:agent_bridge_attach`, `…_attach_read_only`,
`…_revoke`, `…_revoke_all`, `…_copy_setup_command`; mô tả:
"Agent Bridge: Allow Claude Code to control this session", "… to read this session (read-only)",
"Agent Bridge: Revoke access to this session", "Agent Bridge: Revoke all sessions",
"Agent Bridge: Copy Claude Code setup command". Context `id!("Workspace")`. **Không** đưa vào group
`Settings`, **không** thêm vào `should_save_app_state_on_action`.
Handler (`app/src/workspace/view.rs`): `self.active_session_view(ctx)` → active session; không phải
remote → toast lỗi. Attach → toast: "Claude Code can now run commands as root@prod-1 in this session
(expires after 30 min idle). Revoke: 'Agent Bridge: Revoke…'". Copy setup → ghi
`claude mcp add --scope user warp-bridge -- '<std::env::current_exe()>' --warpctrl mcp` vào clipboard
(`ClipboardContent::plain_text`) + toast. Toast dùng `self.toast_stack.update(..add_ephemeral_toast..)`
(tìm theo tên hàm).

**Task 2.7 — `warpctrl remote`** (mục 3.9) + test parse CLI trong `crates/warp_cli/src/local_control_tests.rs`.

**Task 2.8 — Tự review** (security: mọi giá trị động đã quote? mọi lỗi được map? không log lệnh ở
phần safe?) + test + clippy. Commit.

**⛔ CHECKPOINT A** — người dùng chạy checklist 5.A. Mục tiêu: xác nhận exec/read/write với quyền
root qua `sudo -i`, đo độ trễ mỗi lệnh (`duration_ms`), và thử cho Claude Code dùng CLI qua Bash
tool. Độ trễ > ~2 s/lệnh hoặc terminal bị ảnh hưởng → dừng, bàn lại kiến trúc trước Phase 3.

### Phase 3 — MCP server cho Claude Code

**Task 3.1 — Dependencies** `crates/warp_cli/Cargo.toml`: `base64`, `sha2`, `regex`,
`secret_redaction` (đều `.workspace = true`). Verify `cargo check -p warp_cli`.

**Task 3.2 — `mcp/jsonrpc.rs` + tests**: đọc/ghi dòng, `initialize` (thương lượng version), `ping`,
`tools/list`, method lạ, JSON hỏng, notification không có response. Test bằng `Cursor` in/out.

**Task 3.3 — Hàm thuần + tests**: `mcp/edit.rs::apply_edit(content, old, new, replace_all) -> Result<(String, usize), EditError>`;
`mcp/format.rs` (`cat -n`, offset/limit, cắt dòng/tổng, snippet ±3 dòng, render exec result);
`mcp/redact.rs` (`init_default_redaction()`, `to_model_text()`; test: chuỗi dạng AWS key
`AKIA…` bị che; `--no-redact` giữ nguyên).

**Task 3.4 — `mcp/tools.rs` + tests** với fake `ControlTransport`: chọn session mặc định (0/1/2
attach); exec render; read → nhớ sha; write không đọc trước → lỗi; write khi sha đổi → lỗi; write
file chứa secret → từ chối; write file mới → `MustNotExist`; edit thành công gửi đúng `MustMatch`;
edit 0/2 match; lỗi `SessionNotAttached` trả `isError: true` với message gốc.

**Task 3.5 — Lệnh `warpctrl mcp`** (`--instance`, `--no-redact`), vòng lặp stdio; stdout sạch.

**Task 3.6 — Tài liệu**: chốt mục 3.11 + mục 7; viết `specs/agent-bridge/claude/SKILL.md` (skill tuỳ
chọn cho `~/.claude/skills/warp-remote-ops/`: quy trình chẩn đoán → đề xuất → xác nhận → sửa →
kiểm tra cú pháp → reload → xác minh; danh sách lệnh nguy hiểm cần hỏi lại).

**Task 3.7 — Test + clippy. Commit.**

**⛔ CHECKPOINT B** — người dùng chạy checklist 5.B với Claude Code thật.

### Phase 4 — Hoàn thiện

**Task 4.1 — `remote.output.recent` + tool `recent_output`.** Lấy N block gần nhất của pane (lệnh,
exit code, pwd, output cắt 16 KiB). Tham khảo `app/src/ai/block_context.rs` (`BlockContext`) và
cách Agent Mode gắn block làm context. **Cẩn thận lock `TerminalModel`** (AGENTS.md): lock một lần,
copy dữ liệu ra, nhả lock trước khi làm gì khác. Không rõ cách lấy text block → dừng và hỏi.

**Task 4.2 — Chỉ báo thường trực** cho session đang attach (ví dụ badge trên header pane "Claude Code ·
root" có nút Revoke). Đọc skill `gui-ui-guidelines`; tìm indicator có sẵn tương tự (shared session,
SSH/remote indicator) để làm theo. Không tìm được pattern rõ ràng → dừng và hỏi user.

**Task 4.3 — `./script/format`** (một lần, cuối cùng). Commit.

**⛔ CHECKPOINT C** — người dùng test `recent_output` + indicator.

### Phase 5 — v2 (chỉ làm khi user yêu cầu)

Chế độ "visible": chạy lệnh thành block thật trên terminal (`Input::try_execute_command` /
`TerminalView::execute_command_or_set_pending`) rồi đọc output block; duyệt từng lệnh phía Warp
(dialog); pairing token; stream output; PowerShell; Windows; đóng gói `warpctrl` cho bản release;
tích hợp với Warp Sync (mở file mirror trong editor rồi upload).

---

## 5. Checklist test tay (cho người dùng)

**Môi trường:** VM/container có sshd, **hostname khác máy local**, user thường có sudo **cần mật
khẩu**, có nginx (hoặc service bất kỳ có config). Không dùng `ssh localhost` (subshell cùng hostname
bị coi là local). Chạy Warp build từ worktree: `cd ../warp-agent-bridge && ./script/run`.
`W=/projects/github/warp/target/debug/warp` (đúng `CARGO_TARGET_DIR`).

**A. CLI (Checkpoint A)**
1. `ssh user@vm` (Warpify). `$W --warpctrl remote sessions` → thấy session remote, `attached: null`.
   `$W --warpctrl remote exec --session <ID> -- id` → lỗi `SessionNotAttached` có hướng dẫn.
2. Palette "Agent Bridge: Allow Claude Code to control this session" → toast. `exec -- id -un` → user thường.
3. `sudo -i` → Warpify → `exec` cùng ID → `SessionNotAttached` (session active đã đổi). Attach lại →
   `exec -- 'id -un; hostname'` → **root** ✅.
4. `exit` khỏi root → `exec` → `SessionNotAttached`.
5. Đang chạy `sleep 30` trong pane → `exec` trả `SessionBusy` ngay, không treo. Đang `exec -- sleep 20`
   thì gõ lệnh khác + Enter → exec báo lỗi rõ ràng; terminal vẫn bình thường.
6. `exec --timeout 5 -- sleep 999` → ~5 s, `timed_out: true`, exit 124.
7. `exec -- 'read x; echo got=$x'` trả ngay; `exec -- apt-get install htop` (không `-y`) tự abort, không treo.
8. `exec -- 'seq 1 1000000'` → có head/tail + `total_bytes`; terminal vẫn mượt.
9. Quote: `exec -- 'echo "a'"'"'b" $HOME $(id -u)'`; path có dấu cách.
10. `exec --cwd /etc -- pwd` → `/etc`; `exec -- 'cd /tmp; export X=1'` rồi xem prompt: cwd shell user không đổi.
11. `read /etc/shadow` khi root → OK; khi chưa sudo → `permission_denied`.
12. `write --create /root/agent-test.txt --from ./a.txt` → tạo. `write --expected-sha256 <sai>` → lỗi
    conflict, file không đổi. Sha đúng → nội dung mới, có `/root/.warp-agent/backups/agent-test.txt.*`.
    Với file `chmod 640; chown root:adm` → sau khi ghi `stat -c '%U:%G %a'` không đổi; máy có SELinux
    → `ls -Z` không đổi.
13. Attach read-only → `exec`/`write` bị từ chối `InsufficientPermissions`; `read` OK.
14. "Revoke all" → mọi lệnh sau đó bị từ chối.
15. `~/.warp/agent-bridge/audit.jsonl` có đủ dòng, quyền `600`.
16. Tắt Settings > Scripting → mọi `remote.*` bị từ chối.
17. Thử để Claude Code dùng CLI: nhờ nó "dùng `$W --warpctrl remote exec --session <ID> -- …` để xem
    `systemctl status nginx` trên server".

**B. Claude Code qua MCP (Checkpoint B)**
1. `claude mcp add …` (mục 3.11) → `claude mcp list` báo `warp-bridge` connected.
2. Hỏi "liệt kê các session Warp" → thấy `root@vm` attached.
3. "Kiểm tra nginx, đổi `worker_connections` thành 2048, test config rồi reload" → Claude dùng
   `read_file` → `edit_file` (hỏi duyệt) → `exec nginx -t` → `exec systemctl reload nginx` (hỏi duyệt).
   Kiểm tra trên server + backup.
4. File chứa `AWS_SECRET_ACCESS_KEY=…`: `read_file` hiện bị che; `write_file` bị từ chối; `edit_file`
   một dòng khác OK và secret trên server còn nguyên.
5. Sửa tay file trên server giữa lúc Claude đã đọc và định sửa → `edit_file` báo file đã thay đổi.
6. Nhờ Claude đọc 3 file cùng lúc (tool call song song) → đều thành công.
7. Revoke trong Warp → tool call kế tiếp báo lỗi rõ ràng, Claude dừng lại hỏi.

---

## 6. Rủi ro đã biết

| Rủi ro | Giảm thiểu |
|---|---|
| Process khác cùng user local gọi `remote.exec` vào session đã attach | Attach chủ động, gắn `SessionId`, TTL 30 phút, read-only mode, Revoke all, audit log; pairing token để v2 |
| Model chạy lệnh phá huỷ với quyền root | Permission prompt của Claude Code (không allowlist exec/write/edit), `instructions` + skill, backup trước khi ghi |
| Lộ secret của server cho model | Redaction mặc định ở adapter; `edit_file` không đưa nội dung thô cho model; ghi chú: output `exec` vẫn có thể chứa secret không khớp regex |
| In-band chậm / output lớn làm chậm terminal | Cắt output phía remote trước khi truyền; giới hạn read/write; đo ở Checkpoint A |
| User chạy lệnh khi agent đang exec → generator bị kill | Lỗi `SessionBusy`/executor rõ ràng; lệnh idempotent khi thử lại |
| Subshell `sudo -i` chưa Warpify → không có session riêng | Attach hiện `user@host` thật trong toast; `list_sessions` luôn trả `user` |
| Ghi tại chỗ bị gián đoạn giữa chừng | Payload đã ở trên server + backup có đường dẫn trong thông báo lỗi |
| Server thiếu `timeout`/`sha256sum`/`base64` (BusyBox tối giản) | Chạy không `timeout` (chỉ còn timeout phía Rust); ghi bằng `MustMatch` fail-closed; read báo `missing_base64` |
| Thay đổi Warp Sync làm vỡ API dùng chung | Chỉ dùng hàm đã có test; rebase khi Warp Sync đổi `remote_script` |

---

## 7. Hướng dẫn dùng hằng ngày (sau Phase 3)

1. Một lần: palette "Agent Bridge: Copy Claude Code setup command" → dán vào terminal → thêm
   allowlist chỉ-đọc (mục 3.11).
2. Warp: `ssh user@host` → `sudo -i` → Warpify → palette "Agent Bridge: Allow Claude Code to control
   this session" (hoặc bản read-only).
3. Mở Claude Code (VSCode hoặc một pane Warp local bên cạnh): "trên server prod-1, …".
4. Xong việc: palette "Agent Bridge: Revoke …" (hoặc `exit` khỏi `sudo -i` / để hết hạn 30 phút).

---

## 8. Tiến độ, quyết định, nhật ký

### Tiến độ

- [ ] Phase 0 — Worktree + commit plan
- [ ] 1.1 Protocol · [ ] 1.2 Visibility Warp Sync · [ ] 1.3 mod/error · [ ] 1.4 script · [ ] 1.5 attachments · [ ] 1.6 audit
- [ ] 2.1 Flag · [ ] 2.2 ops · [ ] 2.3 model · [ ] 2.4 bridge async · [ ] 2.5 handlers · [ ] 2.6 palette · [ ] 2.7 CLI · [ ] 2.8 review
- [ ] ⛔ CHECKPOINT A (user) — độ trễ đo được: _chưa có_
- [ ] 3.1 deps · [ ] 3.2 jsonrpc · [ ] 3.3 edit/format/redact · [ ] 3.4 tools · [ ] 3.5 `warpctrl mcp` · [ ] 3.6 docs · [ ] 3.7 review
- [ ] ⛔ CHECKPOINT B (user)
- [ ] 4.1 recent_output · [ ] 4.2 indicator · [ ] 4.3 format
- [ ] ⛔ CHECKPOINT C (user)

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| D1 | 2026-09-24 | Claude Code chạy ở máy local, tới server qua session Warp (phương án A) | Không cài/không để credential trên server; tự có quyền root của `sudo -i`; giữ nguyên hệ sinh thái Claude Code của user |
| D2 | 2026-09-24 | Transport = `Session::execute_command` của session active | Giống Warp Sync D1: subshell `sudo -i` dùng in-band executor chạy trong shell root |
| D3 | 2026-09-24 | Kênh điều khiển = local control sẵn có + action `remote.*`, không dựng server mới | Đã có broker UID, credential theo action, loopback-only, gate Scripting |
| D4 | 2026-09-24 | Đồng ý = Attach thủ công theo `SessionId`, chỉ trong RAM, TTL 30 phút, có read-only | Rời `sudo -i` là tự mất quyền root; không có trạng thái "quên" tồn tại qua restart |
| D5 | 2026-09-24 | Không ghi mù: `MustNotExist` / `MustMatch{sha256}` + backup | Tránh ghi đè thay đổi đồng thời của người khác |
| D6 | 2026-09-24 | Ghi đè tại chỗ (`cat > file`) thay vì `mv` file tạm | Giữ owner/mode/ACL/SELinux/hardlink mà không cần cờ GNU-only; payload đã staging đủ trên server |
| D7 | 2026-09-24 | MCP server tự viết, đồng bộ, trong `warp_cli` (không dùng `rmcp` server/tokio) | Ít phụ thuộc, dễ test, tránh API macro của `rmcp` dễ bị đoán sai |
| D8 | 2026-09-24 | Redaction + ghép `edit_file` ở adapter | Nội dung thô không tới model; không cần param `raw` phía app |
| D9 | 2026-09-24 | Worktree `feature/agent-bridge` tách từ `feature/warp-sync` | Dùng lại `remote_script`/`paths` đã test; không đụng thay đổi chưa commit của Warp Sync |
| D10 | 2026-09-24 | v1: máy local unix, server Linux, shell POSIX (bash/zsh/fish qua wrapper) | Broker dùng Unix socket; PowerShell/Windows để v2 |

### Nhật ký

- 2026-09-24 — Plan v1 được viết sau khi khảo sát `local_control`, `warpctrl`, in-band executor,
  Claude Code harness và secret redaction (Claude Opus). Chưa bắt đầu Phase 0.
