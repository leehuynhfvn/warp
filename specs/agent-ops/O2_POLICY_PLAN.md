# Policy + duyệt phía Warp — O2 — Implementation Plan (v2)

> Người thực thi: một coding agent (Claude Sonnet 5). Làm **tuần tự từng Phase**, dừng ở mọi
> **CHECKPOINT** để người dùng test tay. Không tự ý mở rộng phạm vi.
>
> v1 viết 2026-09-27 (Claude Sonnet 5) sau khảo sát code. **v2** (cùng ngày, Claude Opus 5.5): người
> dùng giao quyền chốt mọi mục "⚠ Cần duyệt" của v1; đã kiểm lại khảo sát, sửa chỗ sai (đường dẫn
> `mcp/jsonrpc.rs`, số dòng), và chốt thiết kế — xem bảng Quyết định P1–P20 ở mục 8. **Không còn mục
> nào chờ duyệt**; gặp chỗ plan sai với code thật thì theo quy tắc 0.10.
>
> Plan là phase **O2** của `specs/agent-ops/ROADMAP.md`, gồm cả **pairing token** (chuyển từ Phase 5
> của plan Bridge, D27). Code nền: O1 xong 2026-09-27 — mục 8 của
> `specs/agent-bridge/IMPLEMENTATION_PLAN.md` (D1–D32). Không gồm nhánh G (AO7: G2 chỉ sau O2).

---

## 0. Quy tắc bắt buộc cho agent thực thi

1. Đọc `AGENTS.md` trước. Skill trong `.agents/skills/`: `add-feature-flag` (Task 0.1),
   `rust-unit-tests`, `logging-and-error-reporting`, `gui-ui-guidelines` (**đọc trước Phase 3**).
   Đọc `SKILL.md` tương ứng trước task dùng tới nó.
2. Làm trong worktree `/projects/github/warp-agent-bridge`, nhánh `feature/agent-bridge`. Không đụng
   checkout `/projects/github/warp`.
3. Lệnh cargo: `export CARGO_TARGET_DIR=/projects/github/warp/target`, **luôn kèm `-p warp`** (D14 của
   Bridge). Ví dụ: `cargo test -p warp -p warp_cli --lib -- agent_bridge local_control`,
   `cargo test -p warp -p local_control --lib`. Cargo báo "Blocking waiting for file lock" thì chờ.
4. Sau MỖI task: `cargo check -p warp -p local_control -p warp_cli`, sửa hết lỗi. Không commit code
   không compile.
5. Test `agent_bridge` (chạy `sh` thật) và `terminal::view` hay fail khi chạy song song: chạy lại riêng
   với `-- --test-threads=1` trước khi kết luận là lỗi.
6. Không `unwrap()`/`expect()` trên dữ liệu từ file (policy, danh sách agent), request local-control,
   input UI. Không `let _ =` nuốt lỗi IO. Match exhaustive, không `_` nếu tránh được. `ctx` là tham số
   cuối (trừ khi có closure). Không prefix `_` — xoá hẳn tham số thừa. Comment chỉ nói "why". Format
   args inline. Không truyền `Itertools::format` vào macro log.
7. Unit test trong `<name>_tests.rs`, include cuối module:
   `#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;`.
8. **Cuối mỗi Phase:** `cargo clippy -p warp -p local_control -p warp_cli --all-targets --tests -- -D warnings`,
   sửa hết, rồi `./script/format` **đúng một lần**, commit riêng `chore(agent-ops): run the formatter`.
   Không chạy lại test/lint sau format. Không chạy `./script/presubmit`.
9. Commit conventional (`feat(agent-ops): …`, `fix(agent-ops): …`), mỗi task một commit, message kết
   thúc bằng dòng `Co-Authored-By` theo hướng dẫn attribution của phiên.
10. API không tồn tại / chữ ký khác plan → tìm pattern tương tự trong code; vẫn mơ hồ → **dừng và hỏi**,
    không bịa. Số dòng trong plan là gần đúng — tìm theo tên hàm. Khác plan → ghi dòng mới vào bảng
    "Quyết định" (mục 8, tiếp số P21…) kèm lý do.
11. File này là **nguồn sự thật duy nhất** của O2. Đầu phiên: đọc mục 8. Sau mỗi task: tick checkbox +
    1 dòng "Nhật ký" (commit cùng task). Không tạo `.context/` hay file memory khác.
12. Build để test tay: `./script/run --features warp_control_cli,warp_sync,agent_bridge,agent_ops_policy`
    (thêm ở Task 0.1). Binary MCP: `/projects/github/warp-agent-bridge/target/debug/warp-oss`
    (`./script/run` build vào `target/` của worktree, không theo `CARGO_TARGET_DIR`).

---

## 1. Bài toán và khảo sát

### 1.1 Mục tiêu

O1 để việc duyệt từng thao tác cho **permission prompt của agent** (mục 2.3 plan Bridge). Ba lỗ hổng
roadmap muốn đóng (P4, AO3):

1. Agent chạy với cờ bỏ qua permission (hoặc agent khác, script bất kỳ gọi `warpctrl`) chạy lệnh ghi
   dưới root mà Warp không chặn gì.
2. Người dùng không thấy **đúng lệnh sẽ chạy trên server** trước khi nó chạy.
3. Không phân biệt "lệnh đã tin" (chạy thẳng) với "lệnh lạ" (phải hỏi) và "lệnh cấm" (luôn chặn).

O2 thêm lớp **chính sách + hộp thoại duyệt phía Warp**, độc lập với agent, cho mọi hành động **ghi**:

| Mức | Hành động | O2 làm gì |
|---|---|---|
| L0 | `remote.session.list`, `remote.file.read`, `remote.output.recent` | Không đổi — chỉ cần attach như O1 |
| L1 | `remote.exec`/`remote.exec.visible` khớp **nguyên văn** allowlist của host | Tự chạy, ghi audit |
| L2 | Mọi lệnh ghi khác (gồm mọi `remote.file.write`) | Hộp thoại duyệt trong Warp; 5 phút không trả lời → Deny |
| L3 | Khớp `[deny]` | Luôn Deny, kể cả khi khớp allowlist hoặc đã "cho phép trong session" |

Cộng **pairing token**: MCP client ghép cặp với Warp một lần; hộp thoại + audit hiện danh tính đã xác
minh thay cho tên tự khai (D12 của Bridge); `require_pairing` trong policy chặn ghi từ client chưa
ghép cặp. Không chặn được process cùng UID (đọc được token) — giá trị là **danh tính**, là điều kiện
của G2 (AO7).

**Gate → O3/O4** (roadmap): dùng hằng ngày ≥ 1 tuần trên host lab không sự cố; mọi lệnh ghi đều qua
hộp thoại hoặc allowlist; audit đủ.

### 1.2 Hiện trạng code (đã kiểm chứng 2026-09-27)

| Thành phần | Ý nghĩa với O2 | Bằng chứng |
|---|---|---|
| `handlers/remote.rs`: `start` (Exec/Read/Write, một `ctx.spawn`), `output_recent`, `exec_visible` (chuỗi `ctx.spawn` nhiều tầng); cả ba gọi `check_access` | Policy chỉ gắn vào **2 đường ghi**: `start` khi `Operation::Exec`/`Write` và `exec_visible`. `start` phải tách thành nhiều tầng theo khuôn `exec_visible`. | `app/src/local_control/handlers/remote.rs` — `needed_access` ~71, `Operation::run` ~82, `start` ~115 (`begin_operation(Hidden)` ~129), `output_recent` ~153, `exec_visible` ~188 (`begin_operation(Visible)` ~203), `send_visible_command` ~262, `check_access` ~316 |
| `BridgeResult::Pending { request_id, receiver }`; HTTP handler `await receiver` trên runtime tokio | Chờ duyệt nằm **trong** future đã spawn; **không** đổi `BridgeResult` hay HTTP. | `app/src/local_control/bridge.rs:24-33`, `app/src/local_control/mod.rs:600-612` |
| `agent_bridge/operations.rs::Operations` (Hidden đếm, Visible độc quyền) | `begin_operation` phải gọi **sau** Allow, nếu không một request chờ người (≤ 5 phút) chiếm slot và khoá session. | `app/src/agent_bridge/operations.rs` |
| `AgentBridgeModel { attachments, operations }`, `type Event = ()`, `ctx.notify()` khi attach/detach; `TerminalView` observe model khi flag bật (D26) | Hàng đợi duyệt đặt vào đây; đổi `Event` thành enum để Workspace nghe "có request mới" và hiện toast. Header của pane tự vẽ lại qua observe sẵn có. | `app/src/agent_bridge/model.rs:1-60` |
| `Attachment { access, last_used, exec_count }` (field private), hết hạn/detach xoá entry | Tập "lệnh đã cho phép trong session" đặt **trong** `Attachment` → tự mất khi detach/hết hạn/rời `sudo -i`. | `app/src/agent_bridge/attachments.rs:33-37` |
| `AgentBridgeError` + một `From<AgentBridgeError> for ControlError` duy nhất; `ErrorCode` là enum dùng chung CLI/MCP | Thêm `AgentBridgeError::PolicyDenied` + `ErrorCode::PolicyDenied`, sửa match theo compiler. Không cần mã "đang chờ duyệt": request chờ chỉ là HTTP chưa trả lời. | `app/src/agent_bridge/error.rs`, `crates/local_control/src/protocol.rs:867-896` |
| `warp_sync/confirm_dialog.rs::WarpSyncConfirmDialog`: view dùng `ui_components::dialog::Dialog` + `ActionButton` (`NakedTheme`/`PrimaryTheme`/`DangerPrimaryTheme`), Esc = Cancel, **không** bind Enter; Workspace sở hữu một handle + cờ `is_…_open`, vẽ overlay | Khuôn trực tiếp cho **hộp thoại duyệt** (P4): một dialog/Workspace nhưng là **cửa sổ nhìn vào hàng đợi** (request nằm trong model, không nằm trong dialog) → không mất request khi mở request khác. | `app/src/warp_sync/confirm_dialog.rs`; `app/src/workspace/view.rs` field ~1136, build ~2030, show ~19188, event ~19207, render ~28261 |
| `DismissibleToast` có `with_link(ToastLink::with_onclick_action(WorkspaceAction))`, `with_action_button`; Workspace có `add_agent_bridge_toast`, `activate_tab_by_pane_group_id`, `focus_pane(PaneViewLocator)` | Toast "Agent request waiting — Review" mở hộp thoại đúng request, chuyển tới đúng tab/pane. | `app/src/view_components/dismissible_toast.rs:319-470`, `app/src/workspace/view.rs` ~18952, ~5402, ~6063 |
| Indicator D26 trên pane header (`agent_bridge_access`, `render_agent_bridge_indicator`, nút Revoke, `should_render_header`) | Thêm trạng thái "N waiting" + nút **Review**. Header hẹp → **không** đặt nội dung lệnh ở đây (P4). | `app/src/terminal/view/pane_impl.rs:752-764, 1006-1090` |
| `RequestEnvelope { protocol_version, request_id, target, action }` — **không** `deny_unknown_fields`, không bị log | Chỗ gọn nhất để client gửi token pairing cho **mọi** action (P15). | `crates/local_control/src/protocol.rs:721-737` |
| `CredentialRequest`/`CredentialGrant` là quyền **theo 1 action, TTL 5 phút**, broker kiểm UID | Pairing là tầng danh tính riêng, **không** sửa broker. | `crates/local_control/src/auth.rs`, `app/src/local_control/mod.rs:454-505` |
| MCP: `McpHandler::set_client_name` gọi trong `initialize`; transport gửi mọi action qua `commands.rs::send_action(args, action, params, timeout)` → `client::send_request_with_timeout` | Tên client biết sau `initialize`; token gắn vào `RequestEnvelope` trong transport MCP. | `crates/warp_cli/src/local_control/mcp/jsonrpc.rs:43-55,127-140`, `crates/warp_cli/src/local_control/commands.rs:791-808`, `crates/warp_cli/src/local_control/mcp/mod.rs:~73` |
| Timeout client: `wait = timeout_secs + EXEC_CLIENT_MARGIN (30 s)` ở `warpctrl remote` và `mcp/tools.rs` | Phải cộng thêm thời gian chờ duyệt, nếu không HTTP client bỏ cuộc trước khi người bấm. | `crates/warp_cli/src/local_control/remote.rs:28,206,222`, `crates/warp_cli/src/local_control/mcp/tools.rs:170-192` |
| `app/Cargo.toml` có sẵn `toml = "0.8.13"`, `regex`, `sha2`? (kiểm ở Task 1.1; `warp_cli` đã có `sha2` từ D23) | Không thêm dependency cho parse/regex. Glob tự viết (P7). | `app/Cargo.toml:172-173,211` |
| Audit `~/.warp/agent-bridge/audit.jsonl` (`AUDIT_DIR = ".warp/agent-bridge"`), `create_private_dir_all` | Policy và danh sách agent đặt ở `~/.warp/agent-ops/` (namespace của roadmap, dùng chung với G1 `hosts.toml`) — khác thư mục audit, cố ý. | `app/src/agent_bridge/audit.rs:15,62`, `app/src/warp_sync/paths.rs:302` |
| `MAX_COMMAND_BYTES = 8 KiB` | Lệnh dài vậy không ai đọc hết để duyệt → giới hạn riêng cho lệnh cần duyệt (P11). | `app/src/agent_bridge/mod.rs:24` |

---

## 2. Kiến trúc

### 2.1 Luồng

```
 agent ─MCP─► warpctrl mcp ─(RequestEnvelope + agent_token?)─► Warp: LocalControlBridge
                                                                   │
                  handlers/remote.rs::start (Exec/Write) | exec_visible
                    1. parse/validate, resolve, ensure_supported, check_access   (như O1, main thread)
                    2. flag AgentOpsPolicy tắt → chạy như O1
                    3. ctx.spawn [nền]: nạp policy.toml + agents.toml, policy::evaluate  ──► Decision
                    4. callback [main]:
                         Deny ─────────────────────────────► audit + lỗi PolicyDenied
                         Allow ────────────────────────────► begin_operation → chạy như O1
                         Ask:
                           lệnh nằm trong "cho phép trong session" → như Allow
                           ngược lại → ApprovalQueue.push, emit ApprovalRequested
                                 (header "1 waiting · Review", toast "Review")
                           ctx.spawn [nền]: audit approval_requested (fail-closed),
                                            chờ select(quyết định, Timer 5 phút)
                           callback [main]: Approve → check_access lại → begin_operation → chạy
                                            Deny / hết giờ / Revoke → audit + lỗi PolicyDenied
```

### 2.2 Luật chính sách (`policy::evaluate`, thứ tự cố định)

Input: `hostname` (tên server tự báo, `session.hostname()`), `request` (`Exec{command}` |
`ExecVisible{command}` | `Write{path}`), `paired: bool`.

1. `require_pairing = true` (mặc định `false`) và `!paired` → **Deny** ("agent is not paired").
2. Chọn luật host: host **đầu tiên** trong file có `match` khớp `hostname` (glob `*`/`?`, không phân
   biệt hoa thường); không host nào khớp → `[defaults]`.
3. Deny (L3), luôn thắng: lệnh (`Exec`/`ExecVisible`) khớp bất kỳ regex trong `[deny] patterns`; hoặc
   path (`Write`) khớp bất kỳ glob trong `[deny] paths` → **Deny**.
4. Theo `mode` của luật đã chọn:
   - `read_only` → **Deny**.
   - `approve` → **Ask**.
   - `allowlist` → `Exec`/`ExecVisible` có `command.trim()` bằng đúng một entry của `allow` → **Allow**;
     còn lại (kể cả mọi `Write`) → **Ask**.
5. Ask mà lệnh dài hơn `APPROVAL_MAX_COMMAND_BYTES` (2 KiB) hoặc hơn `APPROVAL_MAX_COMMAND_LINES` (20
   dòng) → **Deny** ("too long for a person to review; write a script with write_file, then run
   it"). (Lệnh visible vốn đã bị cấm xuống dòng — D Task 5.9 của Bridge.)

Chọn host **trước** deny để `[deny]` là chung cho mọi host (roadmap chỉ có một `[deny]` toàn cục) —
thứ tự 2/3 không đổi kết quả, chỉ để code rõ.

### 2.3 Định dạng `~/.warp/agent-ops/policy.toml`

```toml
[defaults]
mode = "approve"                 # bắt buộc: "read_only" | "approve" | "allowlist"
require_pairing = false          # tuỳ chọn

[[hosts]]
match = "lab-*"                  # glob trên hostname server tự báo (`hostname`), * và ?
mode = "allowlist"
allow = ["systemctl restart php-fpm", "systemctl reload nginx"]   # khớp nguyên văn sau trim

[deny]
patterns = ['\brm\s+-rf\s+/', '\bmkfs', '\bdd\s+if=', '\b(shutdown|reboot|halt|poweroff)\b',
            'iptables\s+-F', 'nft\s+flush', '(?i)drop\s+(database|table)']
paths = ["/etc/ssh/sshd_config", "/etc/ssh/sshd_config.d/*", "/etc/sudoers", "/etc/sudoers.d/*"]
```

Kiểm khi nạp (lỗi → `PolicyError` nêu rõ khoá/entry): `mode` hợp lệ; `match` không rỗng; regex biên
dịch được; **entry `allow` không chứa** `; | & $ \` < > ( ) \n \r` hoặc backtick (roadmap mục 3: L1 chỉ
cho lệnh đơn, không metachar); khoá lạ → lỗi (`#[serde(deny_unknown_fields)]`, bắt lỗi gõ nhầm).

| Tình trạng file | Hành vi | Lý do |
|---|---|---|
| Không tồn tại | Như `[defaults] mode = "approve"`, không deny, không host | An toàn mặc định, vẫn dùng được ngay |
| Lỗi đọc/parse/validate | **Mọi hành động ghi → Deny** với message nêu file + lỗi ("Ask the user to fix ~/.warp/agent-ops/policy.toml: …") | Fail-closed (P8) |
| Quyền rộng hơn `0600` (group/other ghi được) | Deny như lỗi parse | Người khác trên máy sửa được policy là mất tác dụng |

Nạp lại **mỗi request ghi**, ở tầng `ctx.spawn` nền (P6) — file nhỏ, không cần cache, sửa file là có
hiệu lực ngay.

### 2.4 Mô hình an toàn (bổ sung bảng 2.3 của plan Bridge)

| Lớp | Cơ chế |
|---|---|
| Có sẵn (O1) | Attach theo `SessionId`, TTL 30 phút, Full/ReadOnly, Revoke/Revoke all, audit fail-closed, redaction phía MCP |
| Flag | `AgentOpsPolicy`. Tắt → y hệt O1. Bật → mọi ghi qua `policy::evaluate` |
| Một đường vào | `authorize` là hàm duy nhất gọi `policy::evaluate`; chỉ `start` (Exec/Write) và `exec_visible` gọi nó |
| Duyệt | Hàng đợi trong RAM, 5 phút → Deny; hộp thoại chỉ mở khi người **bấm** Review (không tự bật, không cướp phím đang gõ); không bind Enter |
| Hiện nguyên văn | Hộp thoại hiện lệnh **không che secret** — người duyệt phải thấy đúng thứ sẽ chạy (P10) |
| Sau khi duyệt | Kiểm attach lại; visible còn kiểm session active + shell rảnh như O1 |
| Kill switch | Revoke / Revoke all / rời `sudo -i` → mọi request đang chờ của session đó bị Deny ngay |
| Danh tính | Pairing token → `agent_id` đã xác minh trong hộp thoại + audit; `require_pairing` |
| Truy vết | Audit thêm `approval_requested`, `policy_decision`, `policy_reason`, `agent_id` |

### 2.5 Ngoài phạm vi O2

G2–G5; sửa policy qua Settings UI (chỉ sửa file); policy theo agent (ngoài `require_pairing`); diff
đầy đủ cho `write_file` (G4 — O2 chỉ hiện 40 dòng đầu); danh sách "mọi request đang chờ" dạng panel
(O5b); L1 cho prod (chỉ là cách người dùng đặt `match`).

---

## 3. Thiết kế chi tiết

### 3.1 Flag + hằng số dùng chung

- `FeatureFlag::AgentOpsPolicy` (`crates/warp_features/src/lib.rs` cạnh `AgentBridge`), cargo feature
  `agent_ops_policy` trong `app/Cargo.toml`, ánh xạ `#[cfg(feature = "agent_ops_policy")]` trong
  `app/src/features.rs` (cạnh `agent_bridge`, dòng ~110). **Không** thêm vào `DOGFOOD_FLAGS` (giống
  `AgentBridge` trong O1). Không có setting/toggle mới → không cần entry palette bật/tắt.
- `crates/local_control/src/protocol.rs`: `pub const APPROVAL_TIMEOUT_SECS: u64 = 300;` — app dùng để
  chờ, client dùng để nới timeout (P12). Một con số, hai phía.
- `app/src/agent_bridge/mod.rs`: `APPROVAL_MAX_COMMAND_BYTES = 2048`, `APPROVAL_MAX_COMMAND_LINES = 20`,
  `APPROVAL_PREVIEW_LINES = 40` (preview nội dung file), `POLICY_FILE = ".warp/agent-ops/policy.toml"`,
  `AGENTS_FILE = ".warp/agent-ops/agents.toml"`, `MAX_PENDING_APPROVALS_PER_SESSION = 8`.

### 3.2 `app/src/agent_bridge/policy.rs` (thuần) + `policy_tests.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode { ReadOnly, Approve, Allowlist }

pub(crate) struct Policy { defaults: Rule, require_pairing: bool, hosts: Vec<HostRule>,
                           deny_patterns: Vec<Regex>, deny_paths: Vec<String> }

pub(crate) enum PolicyRequest<'a> { Exec(&'a str), ExecVisible(&'a str), Write { path: &'a str } }

pub(crate) enum Decision { Allow, Ask, Deny(String) }   // String = lý do cho agent + audit

impl Policy {
    /// Parse + validate (mục 2.3). Không đọc file — nhận `&str` để test.
    pub(crate) fn parse(text: &str) -> Result<Self, PolicyError>;
    pub(crate) fn evaluate(&self, hostname: &str, request: PolicyRequest<'_>, paired: bool) -> Decision;
}

/// Nạp từ đĩa: thiếu file → Policy mặc định; lỗi đọc/parse/quyền → Err (người gọi Deny).
pub(crate) fn load(home: &Path) -> Result<Policy, PolicyError>;

/// Glob tối giản: `*` (chuỗi bất kỳ, gồm `/`), `?` (một ký tự); `case_insensitive` cho hostname.
fn glob_matches(pattern: &str, text: &str, case_insensitive: bool) -> bool;
```

`PolicyError` (thiserror): `Read(String)`, `Parse(String)` (message của `toml` có dòng/cột),
`Invalid(String)`, `Permissions { mode: u32 }`. Không có `unwrap`.

Test (≥ 30): mỗi mode × mỗi loại request; host đầu tiên khớp thắng; glob `lab-*`/`db-?`/không phân
biệt hoa thường/không khớp; deny pattern thắng allowlist; deny path thắng; `allow` có metachar → lỗi;
regex hỏng → lỗi nêu pattern; khoá lạ → lỗi; thiếu `[defaults]`/`mode` → lỗi; `require_pairing` +
`paired=false` → Deny, reads không liên quan (evaluate không nhận read); lệnh > 2 KiB / > 20 dòng khi
Ask → Deny, nhưng khi Allow (khớp allowlist) thì vẫn Allow; `command.trim()`; `load` với file thiếu /
quyền 0644 / 0600 (dùng `tempfile` như `audit_tests.rs`).

### 3.3 Lỗi

- `AgentBridgeError::PolicyDenied(String)` → `ErrorCode::PolicyDenied` (mới, cạnh `SessionNotAttached`
  trong `protocol.rs`). Display: `"Denied by Warp's agent policy: {reason}"`. Các `reason`:
  - `"it matches the deny rule '{pattern}'. This command is never allowed."`
  - `"{host} is read-only for agents."`
  - `"the user denied it."` / `"no one approved it within 5 minutes."` /
    `"the user revoked agent access while it was waiting."`
  - `"it is too long for a person to review; write a script with write_file, then run it."`
  - `"this agent is not paired with Warp. …"` (Phase 5)
  - `"the policy file ~/.warp/agent-ops/policy.toml is invalid ({error}). Ask the user to fix it."`
  - `"too many requests are waiting for approval in this session."`
- Sửa mọi match exhaustive trên `ErrorCode` theo compiler (`warp_cli` output/exit code, MCP
  `format.rs` nếu có). CLI: exit code cho `policy_denied` giống các lỗi khác (không đặt mã riêng).
- `ErrorCode::PolicyDenied` **khác** `InsufficientPermissions` (thiếu quyền attach): agent cần biết
  hỏi user attach lại hay không.

### 3.4 Hàng đợi duyệt — `app/src/agent_bridge/approval.rs` + tests

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalSubject {
    Command { command: String, cwd: Option<String>, visible: bool },
    Write { path: String, bytes: u64, creates: bool, preview: String, preview_truncated_lines: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct ApprovalRequest {
    pub request_id: Uuid,
    pub session: SessionId,
    pub session_label: String,        // "root@draff3"
    pub agent: AgentLabel,            // tên tự khai + agent_id nếu đã pairing (Phase 5; trước đó chỉ tên)
    pub subject: ApprovalSubject,
    pub deadline: SystemTime,         // chỉ để hiện "tự từ chối lúc HH:MM"
    pub window_id: WindowId,          // cửa sổ chứa pane — Workspace nào hiện toast
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalDecision { Approve, AllowInSession, Deny, TimedOut, Revoked }

#[derive(Default)]
pub(crate) struct ApprovalQueue { pending: Vec<(ApprovalRequest, oneshot::Sender<ApprovalDecision>)> }

impl ApprovalQueue {
    /// FIFO. Lỗi khi session đã có MAX_PENDING_APPROVALS_PER_SESSION request chờ.
    pub(crate) fn push(&mut self, request: ApprovalRequest)
        -> Result<oneshot::Receiver<ApprovalDecision>, AgentBridgeError>;
    /// Gửi quyết định; false nếu request không còn (đã hết giờ / đã quyết).
    pub(crate) fn decide(&mut self, request_id: Uuid, decision: ApprovalDecision) -> bool;
    /// Bỏ khỏi hàng mà không gửi gì (dùng khi timer thắng).
    pub(crate) fn remove(&mut self, request_id: Uuid) -> bool;
    pub(crate) fn revoke_session(&mut self, id: SessionId) -> usize;   // gửi Revoked
    pub(crate) fn revoke_all(&mut self) -> usize;
    pub(crate) fn get(&self, request_id: Uuid) -> Option<&ApprovalRequest>;
    pub(crate) fn count_for_session(&self, id: SessionId) -> usize;
    pub(crate) fn oldest_for_session(&self, id: SessionId) -> Option<&ApprovalRequest>;
    pub(crate) fn oldest_in_window(&self, window_id: WindowId) -> Option<&ApprovalRequest>;
}
```

Chờ (tầng nền), theo mẫu `Timer` của `visible::wait`:

```rust
async fn wait_for_decision(receiver: oneshot::Receiver<ApprovalDecision>, timeout: Duration)
    -> ApprovalDecision
{
    // futures::future::select(receiver, Timer::after(timeout)); receiver Canceled → Revoked.
}
```

Timer thắng → callback gọi `queue.remove(id)` (nếu `false`: người vừa bấm đúng lúc — quyết định đã
gửi qua kênh nhưng future đã kết thúc bằng timeout; vẫn coi là `TimedOut`, request không chạy — ghi
test cho race này). Mọi thay đổi hàng đợi → `ctx.notify()`.

Kiểm tra `futures::future::select` + `warpui::r#async::Timer` có dùng được trong `ctx.spawn` (xem
`visible.rs:12,172` cho `Timer`); không chắc thì dừng hỏi.

### 3.5 `AgentBridgeModel` (`model.rs`)

- Thêm field `approvals: ApprovalQueue`.
- `type Event = AgentBridgeEvent;` với `pub enum AgentBridgeEvent { ApprovalRequested { request_id:
  Uuid, window_id: WindowId }, ApprovalsChanged }`. Mọi chỗ `notify()` hiện có giữ nguyên; thêm
  `emit` khi push (và `ApprovalsChanged` khi decide/remove/revoke) để Workspace đóng dialog khi request
  biến mất. Kiểm mọi `subscribe_to_model`/`observe` hiện có với `AgentBridgeModel` vẫn compile (đổi
  `Event` từ `()`).
- `detach`/`detach_all` gọi thêm `approvals.revoke_session`/`revoke_all`.
- **Rời `sudo -i` / session đổi:** attachment không tự biết (O1 kiểm lười ở `check`). Request đang chờ
  của session cũ vẫn nằm đó tới khi hết giờ; Approve → `check_access` lại (mục 3.6) trả
  `NotAttached`/`StaleTarget` → không chạy. Chấp nhận (không thêm hook vào vòng đời session).
- Allow-in-session: `Attachment` thêm `allowed_commands: HashSet<String>` (lệnh đã `trim`);
  `Attachments::allow_command(id, command)`, `Attachments::is_command_allowed(id, command, now) -> bool`
  (không trả true cho entry hết hạn). Wrapper tương ứng trong model.

### 3.6 `authorize` và wiring (`handlers/remote.rs`)

Giữ nguyên thứ tự kiểm tra đầu hàm của O1. Thêm:

```rust
/// Decides whether an agent may perform `request` in `session`, asking the user when the policy
/// says so. `proceed` runs on the main thread once the request is allowed.
fn authorize(
    ctx_info: AuthorizeInput,              // session id, hostname, user, session_label, window_id,
                                           // request_id, agent label, paired, subject, audit target
    proceed: impl FnOnce(&mut ModelContext<LocalControlBridge>) + 'static,
    fail: impl FnOnce(AgentBridgeError, &mut ModelContext<LocalControlBridge>) + 'static,
    ctx: &mut ModelContext<LocalControlBridge>,
)
```

(Hình dạng closure là gợi ý; nếu borrow/`'static` khiến khó viết, tách thành enum trạng thái +
các hàm `continue_*` — ghi Quyết định. Không được để logic Allow bị copy ở 2 chỗ.)

1. Flag tắt → `proceed(ctx)` ngay.
2. `ctx.spawn` nền: `policy::load(home)` → `evaluate`. Lỗi load → `Deny(invalid policy …)`.
3. Callback main:
   - `Deny(reason)` → audit (nền, best-effort) `result: error, error_code: policy_denied,
     policy_decision: deny` → `fail(PolicyDenied(reason))`.
   - `Allow` → `proceed(ctx)` (audit `policy_decision: allow` ghi trong bản ghi `started` của O1 — thêm
     field vào `Target`/`Audit` để `ops::*` ghi nó).
   - `Ask` + `is_command_allowed` → `proceed` với `policy_decision: session_rule`.
   - `Ask` → `push` (lỗi → `fail`), emit `ApprovalRequested` → `ctx.spawn` nền: audit
     `approval_requested` (**fail-closed**: ghi không được → `remove` + `fail(Io)`), rồi
     `wait_for_decision(rx, APPROVAL_TIMEOUT)` → callback main:
     - `Approve`/`AllowInSession` → `check_access` lại (lỗi → `fail`); `AllowInSession` +
       `Command` → `allow_command`; `proceed` với `policy_decision: approved` / `approved_in_session`.
     - `Deny`/`TimedOut`/`Revoked` → audit `ask_denied`/`ask_timeout`/`revoked` → `fail(PolicyDenied)`.

Nối vào:

- **`start`** (Exec/Write): Read → như O1 (không authorize). Exec/Write → sau `check_access`, tạo
  `oneshot` trả `receiver` ngay (như O1), rồi `authorize(proceed = begin_operation(Hidden) +
  ctx.spawn(operation.run) như code O1 hiện tại, chuyển thành hàm `run_hidden_operation`, fail =
  gửi lỗi qua `sender`)`. `Write`: `subject = Write { path, bytes, creates = expectation ==
  MustNotExist, preview }` với preview = decode base64 → nếu UTF-8 thì 40 dòng đầu, không thì
  `"(binary, N bytes)"`.
- **`exec_visible`**: sau `check_access`, thay `begin_operation(Visible)` + phần sau bằng
  `authorize(proceed = begin_operation(Visible) + toàn bộ chuỗi O1 hiện có (audit started → gõ lệnh →
  chờ block), fail = gửi lỗi)`. Lệnh chỉ được **gõ vào shell sau khi Allow**.
- Audit `started` của O1 (fail-closed) vẫn ở trong `proceed` — tức **sau** duyệt (P13).

### 3.7 Hộp thoại duyệt — `app/src/agent_bridge/approval_dialog.rs` + tests

Theo khuôn `WarpSyncConfirmDialog` (đọc `gui-ui-guidelines` trước):

- `AgentApprovalDialog { request_id: Option<Uuid>, deny_button, allow_in_session_button,
  approve_button }` — `ActionButton`: Deny = `NakedTheme`, "Allow this command in this session" =
  `SecondaryTheme` (ẩn khi subject là `Write`), Approve = `DangerPrimaryTheme` (chạy root trên server,
  giống nút Upload của Warp Sync). Kiểm tên theme thật trong `view_components/action_button.rs`;
  **không** tạo theme mới.
- Keymap: `escape` → `Close` (đóng dialog, request **vẫn chờ**). Không bind Enter.
- `set_request(request_id)`; `render` đọc `AgentBridgeModel::as_ref(app).approvals.get(id)` — không
  còn → render rỗng và Workspace đóng dialog (nghe `ApprovalsChanged`).
- Nội dung (chữ trung lập với agent, D11; tiếng Anh như phần còn lại của UI), qua hàm thuần
  `approval_dialog::content(&ApprovalRequest, now) -> (title, body)` để test:
  - Title: `Run on root@draff3?` / `Write a file on root@draff3?`
  - Body: `Agent: claude-code (paired)` hoặc `Agent: claude-code (unverified name)`; `Runs visibly
    in the terminal` / `Runs in the background`; `Directory: /etc/nginx` (nếu có);
    lệnh nguyên văn (không che, P10); với Write: `Path`, `Size`, `Creates a new file` /
    `Replaces the existing file (a backup is kept on the server)`, preview + `… N more lines`;
    dòng cuối `Denied automatically at 14:32 if no one answers.` Dùng `printable()` cho chữ lấy
    từ server/agent (bỏ ký tự điều khiển), **không** redaction.
- Events: `Decided { request_id, decision }`, `Closed`.
- Workspace (`workspace/view.rs`, theo đúng các chỗ gắn `warp_sync_confirm_dialog`): field
  `agent_approval_dialog`, cờ `is_agent_approval_dialog_open` (cùng struct chứa cờ warp sync), build,
  render overlay, `show_agent_approval_dialog(request_id)`, xử lý event → `AgentBridgeModel::decide`.
  Nếu dialog Warp Sync đang mở thì dialog duyệt đợi (toast vẫn hiện) — không chồng hai overlay.

### 3.8 Header, toast, palette

- `pane_impl.rs`: `agent_bridge_access` giữ nguyên; thêm `pending_agent_requests(app) -> usize` cho
  session active. `> 0` → nhãn indicator thành `Agents · root · 1 waiting` (màu vàng như Full) và
  thêm nút `Review` (text button nhỏ hoặc icon button + tooltip "Review the waiting agent request";
  `MouseStateHandle` tạo **một lần** trên view như nút Revoke) dispatch
  `TerminalAction::ReviewAgentRequest` → emit event lên Workspace mở dialog cho
  `oldest_for_session`. Tìm đường event TerminalView → Workspace có sẵn (vd cách các
  `TerminalAction` khác mở modal cấp Workspace); không có đường gọn thì dùng
  `ctx.dispatch_typed_action(WorkspaceAction::AgentOpsReviewRequest { request_id })`.
- Toast: Workspace `subscribe_to_model(AgentBridgeModel)`; `ApprovalRequested { window_id }` khớp
  `ctx.window_id()` → `add_agent_bridge_toast(DismissibleToast::default("An agent is waiting for
  approval on root@host").with_link(ToastLink::new("Review").with_onclick_action(
  WorkspaceAction::AgentOpsReviewRequest { request_id })))`. Action này: nếu request còn → chuyển tới
  tab/pane chứa session (dùng `metadata::session_entries`-kiểu tra pane, `activate_tab_by_pane_group_id`
  + `focus_pane`; nếu khó, chỉ mở dialog — ghi Quyết định) rồi mở dialog.
- `window_id` cho `ApprovalRequest`: lấy từ view handle của pane lúc resolve (`SessionSnapshot` đã giữ
  `terminal_view`; kiểm `ViewHandle::window_id(ctx)` hoặc tương đương). Không tìm được → hỏi.
- Palette (`WorkspaceAction` + binding trong `if FeatureFlag::AgentOpsPolicy`, như 5 binding
  `workspace:agent_bridge_*` của O1): `Agent Ops: Review waiting agent requests` (mở dialog cho
  `oldest_in_window`, không có → toast "No agent request is waiting"), `Agent Ops: Deny all waiting
  agent requests`. Revoke all hiện có đã deny luôn (mục 3.5).

### 3.9 Client: timeout + hướng dẫn

- `warpctrl remote exec` / `exec --visible` / `write` và MCP `exec`, `exec_visible`, `write_file`,
  `edit_file`: `wait = timeout_secs + EXEC_CLIENT_MARGIN + APPROVAL_TIMEOUT_SECS` (vô điều kiện, P12).
  Write hiện có timeout riêng — cộng tương tự. Test: giá trị `wait` tính đúng.
- MCP `format.rs`: lỗi `policy_denied` là `isError` với message gốc (đã có cơ chế cho mọi lỗi — chỉ
  kiểm).
- `INSTRUCTIONS` (mcp/tools.rs) + skill `specs/agent-bridge/claude/SKILL.md`: thêm đoạn: "Warp may ask
  the user to approve a write; the call then waits up to 5 minutes. If the result says 'Denied by
  Warp's agent policy', do not retry the same command or rephrase it to get around the rule; tell the
  user why it was denied." Thêm: "Keep commands short and single-purpose so the user can review them."
- Timeout phía MCP client: Claude Code và Codex có giới hạn thời gian cho một tool call. Mục 7 hướng
  dẫn đặt giới hạn ≥ 900 s. **Tra tài liệu chính thức** tên cấu hình hiện tại (Claude Code: biến
  `MCP_TOOL_TIMEOUT`?; Codex: `tool_timeout_sec` trong `[mcp_servers.<name>]`?) — không chắc thì ghi
  "kiểm tài liệu" thay vì bịa; checklist P2.9 đo thật.

### 3.10 Audit (`audit.rs`)

`AuditOutcome` thêm `ApprovalRequested`. `AuditRecord` thêm (đều `skip_serializing_if` rỗng):
`policy_decision: Option<&'static str>` (`allow` | `session_rule` | `approved` | `approved_in_session` |
`deny` | `ask_denied` | `ask_timeout` | `revoked`), `policy_reason: Option<String>`,
`agent_id: Option<String>` (Phase 5). Không ghi token, không ghi nội dung file. Test: mỗi nhánh một
dòng đúng; bản ghi Deny có `command`/`path` như O1.

### 3.11 Pairing token (Phase 5)

**Mô hình:** client giữ một token bí mật; Warp giữ `sha256(token)` + nhãn. Token đi trong
`RequestEnvelope` của **mọi** action; bridge băm và tra để biết `agent_id`.

- **Protocol** (`crates/local_control/src/protocol.rs`):
  - `pub struct AgentToken(String)` — `#[serde(transparent)]`, `Debug` in `AgentToken(****)`, không
    `Display`. Validate: 43 ký tự base64url (32 byte, như `AuthToken::generate`).
  - `RequestEnvelope` thêm `#[serde(default, skip_serializing_if = "Option::is_none")] pub agent_token:
    Option<AgentToken>`.
  - Action mới `agent.pair` (group mới `agent`, target `Instance`, `Implemented`):
    `AgentPairParams { name: String }` (`deny_unknown_fields`, cùng quy tắc tên `agent` của D12:
    `[A-Za-z0-9._-]{1,64}`); kết quả `AgentPairResult { agent_id: String, status: "paired" |
    "already_paired" }`. Không có `agent_token` trong envelope → `InvalidParams`.
- **Kho phía Warp** — `app/src/agent_bridge/pairing.rs` + tests:
  `~/.warp/agent-ops/agents.toml` (`0600`, ghi atomic: file tạm cùng thư mục + `rename`):
  ```toml
  [[agents]]
  id = "claude-code"            # Warp đặt: tên tự khai, trùng thì "-2", "-3"
  token_sha256 = "…64 hex…"
  paired_at = "2026-09-28T09:00:00Z"
  ```
  Hàm thuần: `parse`, `serialize`, `find(&[PairedAgent], sha) -> Option<&PairedAgent>`,
  `next_id(existing, name)`; I/O: `load(home)`, `add(home, name, sha)`, `forget_all(home)`. File lỗi
  → coi như **không agent nào được ghép** (+ `log::warn!`, không log nội dung). Bền qua khởi động lại
  (P16); quên từng agent = xoá khối trong file (đọc lại mỗi request).
- **Tra danh tính** (bridge): `handle_request` băm `request.agent_token` (nếu có) một lần → truyền
  `token_sha256: Option<String>` xuống `handlers::remote::{start, exec_visible}`; `authorize` đọc
  `agents.toml` ở tầng nền cùng lúc với policy → `paired`, `agent_id`. Không có token hoặc không
  khớp → `paired = false` (không lỗi, trừ khi `require_pairing`).
- **Handler `agent.pair`** (`app/src/local_control/handlers/agent.rs`, `Pending` như remote):
  1. Token đã có trong file → `already_paired` ngay.
  2. Đã có một yêu cầu pairing đang chờ (toàn app) → lỗi `SessionBusy`-kiểu (`"another agent is
     waiting to be paired"`) — chống spam hộp thoại.
  3. Push vào `ApprovalQueue` với subject mới `ApprovalSubject::Pairing { name }` (session = none →
     đổi field `session` thành `Option<SessionId>`; `window_id` = cửa sổ đang active). Hộp thoại (cùng
     `AgentApprovalDialog`): title `Pair an agent with Warp?`, body: tên tự khai, câu cảnh báo
     `Pairing lets Warp show which agent sends each request. It does not stop other programs running
     as your user.`, nút Deny / Pair (ẩn "Allow in session"). Toast như 3.8.
  4. Approve → `pairing::add` (nền) → `paired`; Deny/hết giờ → `PolicyDenied("the user did not pair
     this agent")`.
- **Palette:** `Agent Ops: Forget all paired agents` (xoá file sau khi xác nhận bằng
  `WarpSyncConfirmDialog`-kiểu? — không: dùng toast kết quả, thao tác này chỉ **giảm** quyền, không
  cần xác nhận).
- **Client** (`crates/warp_cli/src/local_control/`):
  - `pairing.rs`: token file `~/.warp/agent-ops/agent-tokens/<name>.token` (`0600`, thư mục `0700`,
    ghi bằng `create_new` để không đè; đọc: từ chối file quyền rộng hơn `0600`). `<name>` = tên đã làm
    sạch của client (D24).
  - `send_action` (hoặc biến thể mới `send_action_as_agent`) nhận `Option<&AgentToken>` và đặt vào
    envelope. Đổi tối thiểu: không đổi chữ ký hàm của crate `local_control`.
  - MCP transport: sau `initialize` (biết tên), **lần tool call đầu tiên** của process: nạp/tạo token →
    gọi `agent.pair` (timeout `APPROVAL_TIMEOUT_SECS + 30 s`) → nhớ kết quả cho cả process. Lỗi / bị từ
    chối → vẫn gửi token (Warp coi là chưa ghép), in một dòng stderr, **không** thử lại trong process
    đó. Cờ `warpctrl mcp --no-pair`: không gửi token, không pairing.
  - `warpctrl remote …` (CLI người dùng gõ): không pairing, không token (người là "agent" ở đây).
- **Hiện danh tính:** `AgentLabel { claimed: Option<String>, agent_id: Option<String> }` trong
  `ApprovalRequest`; hộp thoại: `claude-code (paired)` / `claude-code (unverified name)`; audit thêm
  `agent_id`. Indicator header không đổi.

---

## 4. Các Phase

Thứ tự: 0 → 1 → CHECKPOINT P1 → 2 → 3 → CHECKPOINT P2 → 4 → CHECKPOINT P3. Cuối mỗi Phase: clippy 3
package + `./script/format` một lần (mục 0.8).

### Phase 0 — Chuẩn bị

- **0.1** `FeatureFlag::AgentOpsPolicy` + cargo feature `agent_ops_policy` + ánh xạ `features.rs`
  (skill `add-feature-flag`, mục 3.1). `APPROVAL_TIMEOUT_SECS` trong `protocol.rs`; hằng số của
  `agent_bridge/mod.rs`. Verify: `cargo check -p warp -p local_control -p warp_cli`, và
  `cargo check -p warp --features agent_ops_policy`.

### Phase 1 — Chính sách chặn/cho phép (chưa có Ask)

- **1.1** `agent_bridge/policy.rs` + `policy_tests.rs` (mục 3.2). Kiểm `app/Cargo.toml` có `sha2`
  (cần ở Phase 4) — ghi nhật ký. Verify: `cargo test -p warp --lib agent_bridge::policy`.
- **1.2** `ErrorCode::PolicyDenied`, `AgentBridgeError::PolicyDenied`, `From` (mục 3.3); sửa match theo
  compiler; test `error_tests.rs` (mã + message), `protocol_tests.rs` (serde `policy_denied`).
- **1.3** `authorize` **chỉ với Allow/Deny** (Ask tạm thời → Deny với reason `"approval is not
  available yet"` — thay ở 2.3): tầng nền nạp + evaluate, callback main; tách `start` thành
  `run_hidden_operation`; nối `start` (Exec/Write) và `exec_visible` (mục 3.6). Audit
  `policy_decision`/`policy_reason` (mục 3.10, phần không liên quan Ask). Test handler qua HTTP như
  `remote_tests.rs`: flag tắt → hành vi O1 (các test cũ vẫn pass); read không bị policy chặn kể cả
  `read_only`; lỗi `policy_denied` có message đúng (dùng `HOME` tạm — xem cách `audit_tests.rs`/
  `remote_tests.rs` đặt thư mục; nếu phải đổi `HOME` toàn cục thì dùng `#[serial]` như test khác trong
  repo, hoặc cho `load` nhận `home: &Path` và handler lấy từ một hàm có thể thay trong test).
- **1.4** Nới timeout client (mục 3.9, phần timeout) + test.
- Cuối Phase: `cargo test -p warp -p warp_cli --lib -- agent_bridge local_control`, clippy, format.

**⛔ CHECKPOINT P1** — checklist 5.P1 (CLI, không cần UI).

### Phase 2 — Hàng đợi duyệt (logic, chưa có UI)

- **2.1** `agent_bridge/approval.rs` + tests (mục 3.4): push/decide/remove/revoke/oldest; giới hạn 8;
  `wait_for_decision` (Approve trước timeout, timeout, kênh bị drop → Revoked); race timeout-thắng.
- **2.2** `AgentBridgeModel`: `approvals`, `AgentBridgeEvent`, detach → revoke; `Attachment.allowed_commands`
  + hết hạn xoá (mục 3.5). Sửa chỗ nghe model theo compiler. Test `attachments_tests.rs`/`model`.
- **2.3** `authorize` nhánh Ask đầy đủ (mục 3.6): push, audit `approval_requested` fail-closed, chờ,
  kiểm attach lại, AllowInSession. `ApprovalSubject` cho Exec/ExecVisible/Write (preview). Test handler:
  Ask → quyết định bằng `AgentBridgeModel::decide` trong test → chạy/không chạy; Revoke khi đang chờ →
  `policy_denied` ngay; detach giữa chừng rồi Approve → `session_not_attached`; `APPROVAL_TIMEOUT`
  rút ngắn trong test (tham số hoá `wait_for_decision(timeout)`; handler lấy timeout từ một hàm
  `#[cfg(test)]`-thay được hoặc field trong model — chọn cách ít xâm lấn, ghi Quyết định).
- **2.4** INSTRUCTIONS MCP + skill `SKILL.md` (mục 3.9). Test MCP: message `policy_denied` là `isError`.
- Cuối Phase: test, clippy, format.

### Phase 3 — UI duyệt

- **3.1** Đọc `gui-ui-guidelines`; đọc `confirm_dialog.rs` + chỗ gắn Workspace.
- **3.2** `approval_dialog.rs` (view + `content` thuần + tests nội dung: lệnh dài, lệnh có ký tự điều
  khiển, write text/binary/preview cắt, pairing chưa có ở phase này).
- **3.3** Gắn dialog vào Workspace (field, build, render overlay, show/close, event → `decide`, đóng khi
  `ApprovalsChanged` làm request biến mất, không chồng dialog Warp Sync).
- **3.4** Header: "N waiting" + nút Review (`MouseStateHandle` một lần), `TerminalAction::ReviewAgentRequest`
  → mở dialog (mục 3.8). Test nhãn trong `messages.rs` (không nhắc "claude").
- **3.5** Toast + `WorkspaceAction::AgentOpsReviewRequest { request_id }` (chuyển tab/pane nếu làm
  được) + 2 entry palette (mục 3.8).
- **3.6** Tự review: rust-reviewer + security-reviewer (agent `ecc:rust-reviewer`,
  `ecc:security-reviewer`), trọng tâm: không đường nào chạy lệnh ghi khi flag bật mà không qua
  `authorize`; lệnh visible không bao giờ được gõ trước Allow; race quyết định/timeout/revoke; lock
  `TerminalModel` (quy tắc bảng lock Phase 5 của Bridge vẫn áp dụng; code mới không được lock thêm);
  dialog không có Enter; không log token/nội dung. Sửa CRITICAL/HIGH, ghi phần bác bỏ vào nhật ký.
  Test: `agent_bridge`, `local_control`, `terminal::view`, `workspace` (song song, fail thì chạy lại
  `--test-threads=1`).
- Cuối Phase: clippy, format.

**⛔ CHECKPOINT P2** — checklist 5.P2.

### Phase 4 — Pairing token

- **4.1** Protocol: `AgentToken`, `RequestEnvelope.agent_token`, action `agent.pair` (Stub) + params/
  result + catalog + resolver + arm bridge; test serde (Debug che token, envelope cũ không có trường vẫn
  parse, token sai độ dài → lỗi).
- **4.2** `agent_bridge/pairing.rs` (kho `agents.toml`) + tests (parse/serialize/tên trùng/ghi atomic/
  quyền 0600/file hỏng → rỗng).
- **4.3** Bridge băm token → `token_sha256` xuống handler; `authorize` tra `paired`/`agent_id`;
  `require_pairing`; `AgentLabel` trong `ApprovalRequest`; audit `agent_id`. Test.
- **4.4** Handler `agent.pair` (`Implemented`), `ApprovalSubject::Pairing`, `session: Option<SessionId>`,
  một pairing chờ tại một thời điểm; dialog nhánh pairing; palette "Forget all paired agents". Test
  handler (already_paired, busy, approve → file có entry, deny).
- **4.5** Client: `warp_cli/src/local_control/pairing.rs` (token file), `send_action` mang token, MCP
  pairing lười ở tool call đầu, `--no-pair`, test với transport giả (pair một lần/process, lỗi pair
  không chặn tool call, token có trong envelope, `--no-pair` không gửi).
- **4.6** Mục 7 + skill: pairing, cách quên agent.
- **4.7** Review (rust + security; trọng tâm: token không bao giờ vào log/audit/lỗi; so sánh hash; file
  token quyền; spam pairing) + test + clippy + format.

**⛔ CHECKPOINT P3** — checklist 5.P3. Sau đó cập nhật roadmap (tick O2) và ghi nhật ký kết thúc.

---

## 5. Checklist test tay (cho người dùng)

**Môi trường:** như checklist 5.A–5.C của plan Bridge (VM có sshd, hostname khác máy local, sudo cần
mật khẩu). Build theo mục 0.12, bật Settings > Scripting, attach **Full** session `root@<host>`.
`W=/projects/github/warp-agent-bridge/target/debug/warp-oss`, `P=~/.warp/agent-ops/policy.toml`,
`S='<session id>'` (lấy bằng `$W --warpctrl remote sessions`).

**5.P1 — Chặn/cho phép qua CLI (CHECKPOINT P1)**

1. Build **không** có `agent_ops_policy`: `$W --warpctrl remote exec --session "$S" -- 'id -un'` chạy
   như O1 (không có policy).
2. Build có feature, **không có** `$P`: lệnh trên → lỗi `policy_denied` "approval is not available yet"
   (Phase 1 chưa có Ask; mặc định là `approve`).
3. `$P` = `[defaults] mode = "read_only"` (`chmod 600 $P`): `exec` → `policy_denied` "… is read-only
   for agents"; `$W --warpctrl remote read --session "$S" /etc/hostname` vẫn đọc được.
4. `mode = "allowlist"`, `allow = ["id -un"]`: `exec -- 'id -un'` chạy, in `root`; `exec -- ' id -un '`
   cũng chạy; `exec -- 'id'` → `policy_denied`.
5. Thêm `[deny] patterns = ['\bid\b']`: `exec -- 'id -un'` → `policy_denied` "matches the deny rule"
   (deny thắng allowlist).
6. `[deny] paths = ["/tmp/agent-ops-*"]`: `write --create /tmp/agent-ops-x` → `policy_denied`.
7. `chmod 644 $P` → mọi ghi `policy_denied` nêu quyền file. `chmod 600`, rồi xoá một dấu `"` → mọi ghi
   `policy_denied` nêu lỗi parse (có số dòng). Sửa lại → chạy lại được **không cần restart Warp**.
8. `allow = ["id; reboot"]` → lỗi nạp policy nêu metachar.
9. `tail -3 ~/.warp/agent-bridge/audit.jsonl` có `policy_decision`/`policy_reason` cho các bước trên.

**5.P2 — Hộp thoại duyệt (CHECKPOINT P2)**

1. `mode = "approve"`. Từ Claude Code (MCP `warp-bridge`): "chạy `uptime` trên server". Header pane
   hiện "Agents · root · 1 waiting" + nút Review; toast "An agent is waiting…". Chưa có gì chạy trên
   server.
2. Bấm Review → hộp thoại: title "Run on root@<host>?", agent, "Runs in the background", lệnh nguyên
   văn, giờ tự từ chối. Nhấn Enter → **không** có gì xảy ra. Esc → đóng, header vẫn "1 waiting".
3. Review lại → Approve → Claude nhận output `uptime`; header về "Agents · root".
4. Lặp lại, Deny → Claude nhận lỗi "the user denied it" và **không** thử lách (skill).
5. Lặp lại, không làm gì 5 phút → Claude nhận "no one approved it within 5 minutes"; header hết
   "waiting"; audit có `ask_timeout`.
6. `exec_visible` (nhờ Claude "chạy `df -h` cho tôi xem"): **không có gì gõ vào shell** trước khi
   Approve; sau Approve block hiện trong pane như O1, bản nháp đang gõ dở vẫn còn.
7. "Allow this command in this session" cho `uptime` → Claude gọi `uptime` lần hai: chạy ngay không hỏi.
   Revoke rồi attach lại → `uptime` phải hỏi lại.
8. Claude sửa một file (`edit_file`): hộp thoại "Write a file on root@<host>?" có path, size, "Replaces
   the existing file…", preview; không có nút "Allow in session".
9. Đang có request chờ → bấm Revoke trên header → Claude nhận lỗi ngay ("revoked…"), dialog (nếu mở)
   tự đóng.
10. Hai tab, hai session đã attach; nhờ Claude chạy lệnh ở session của tab **không** active → toast ở
    cửa sổ hiện tại, bấm Review → chuyển tới đúng tab (hoặc ít nhất mở đúng request), duyệt được.
11. Palette "Agent Ops: Deny all waiting agent requests" khi có 2 request chờ → cả hai bị từ chối.
12. Lệnh dài > 20 dòng / > 2 KiB (nhờ Claude chạy một heredoc dài qua `exec`) → Deny ngay "too long…".
13. (Nếu dùng Codex) request chờ 3 phút rồi Approve: Codex vẫn nhận kết quả — nếu Codex báo timeout,
    đặt giới hạn tool call theo mục 7 và thử lại; ghi kết quả vào nhật ký.

**5.P3 — Pairing (CHECKPOINT P3)**

1. Khởi động lại Claude Code (process `warpctrl mcp` mới), gọi `list_sessions`: hộp thoại "Pair an agent
   with Warp?" (tên `claude-code`) — Pair. `~/.warp/agent-ops/agents.toml` có entry (quyền 0600),
   `~/.warp/agent-ops/agent-tokens/claude-code.token` quyền 0600.
2. Lệnh cần duyệt từ Claude: hộp thoại ghi "claude-code (paired)"; audit có `agent_id`.
3. Khởi động lại Warp và Claude Code: **không** hỏi pairing lại.
4. `require_pairing = true`; chạy `$W --warpctrl remote exec …` (CLI, không token) → `policy_denied`
   "not paired"; Claude vẫn chạy được (sau duyệt).
5. Xoá khối của `claude-code` trong `agents.toml` → lệnh tiếp theo từ Claude: "unverified name", và
   (với `require_pairing = true`) bị Deny; restart Claude Code → hỏi pairing lại.
6. Deny hộp thoại pairing → Claude vẫn dùng được như client chưa ghép cặp (khi `require_pairing =
   false`), không hỏi lại tới khi process MCP khởi động lại.
7. Palette "Agent Ops: Forget all paired agents" → file rỗng; toast báo số agent đã quên.
8. `grep -r "<nội dung token>" ~/.warp/agent-bridge ~/.local/state/warp* 2>/dev/null` (log của bản
   dev — tìm đường log thật ở nhật ký Bridge) → không thấy token.

---

## 6. Rủi ro đã biết

| Rủi ro | Giảm thiểu / chấp nhận |
|---|---|
| Process cùng UID sửa `policy.toml`, đọc token, gọi thẳng broker | Giới hạn đã biết của mô hình (D21 của Bridge, roadmap): quyền 0600 chỉ chặn user khác. Giá trị thật: chặn agent "ngoan" nhưng bỏ qua permission, và hiện đúng lệnh trước mắt người |
| Denylist regex bị lách (`eval "$(echo … \| base64 -d)"`) | Roadmap: chỉ là gờ giảm tốc; ranh giới thật là `read_only`, hộp thoại, allowlist khớp nguyên văn không metachar |
| Duyệt theo phản xạ | Hộp thoại chỉ mở khi người bấm Review; không Enter; lệnh nguyên văn; giới hạn độ dài; "allow in session" chỉ đúng lệnh đó, mất khi detach |
| Script ghi bằng `write_file` (preview 40 dòng) rồi chạy bằng `exec` | Người duyệt thấy "Write a file … N more lines" rồi "Run `sh /tmp/x.sh`" — hai lần hỏi; diff đầy đủ là G4. Ghi trong skill: script dài → nói trước với user |
| Hộp thoại không che secret | Người duyệt là chủ server, cần thấy đúng lệnh; rủi ro khi chia sẻ màn hình — chấp nhận |
| MCP client timeout tool call ngắn hơn 5 phút | Mục 7 hướng dẫn cấu hình; checklist 5.P2.13 |
| Request của session cũ (sau khi rời `sudo -i`) vẫn nằm hàng tới hết giờ | Approve vẫn không chạy (kiểm attach lại); chấp nhận |
| Timeout HTTP dài thêm 5 phút cho mọi request ghi | Chỉ là trần; Allow/Deny trả lời ngay như O1 (0,183 s đo ở Checkpoint A) |
| Đổi `AgentBridgeModel::Event` từ `()` | Compiler chỉ ra mọi chỗ; test `terminal::view`/`workspace` |
| Spam hộp thoại pairing | Một pairing chờ tại một thời điểm; MCP chỉ pair một lần/process |
| `agents.toml` bền qua restart (khác attach) | Pairing không cấp quyền gì, chỉ gắn danh tính; quên được bằng palette/xoá khối |

---

## 7. Hướng dẫn dùng hằng ngày (hoàn thiện ở Task 2.4/4.6)

1. Build/bản release với feature `agent_ops_policy` (thêm vào lệnh `bundle` ở mục 3.11 của plan Bridge).
2. Tạo `~/.warp/agent-ops/policy.toml`, `chmod 600`. Bắt đầu `mode = "approve"`; thêm `[[hosts]]` +
   `allowlist` cho lệnh đã tin trên host lab; `[deny]` cho thứ không bao giờ được chạy.
3. Khi agent muốn ghi: header pane "N waiting" / toast → **Review** → đọc lệnh → Approve / Deny /
   Allow in session. Không làm gì = từ chối sau 5 phút. Revoke = từ chối mọi thứ đang chờ.
4. Timeout tool call của agent (điền tên cấu hình đã kiểm ở Task 2.4): Claude Code …; Codex
   `[mcp_servers.warp-bridge]` … ≥ 900.
5. Pairing (sau Phase 4): lần đầu agent kết nối sẽ hỏi "Pair an agent with Warp?". Quên một agent: xoá
   khối trong `~/.warp/agent-ops/agents.toml`; quên hết: palette "Agent Ops: Forget all paired agents".
   `require_pairing = true` để chỉ agent đã ghép cặp được ghi.

---

## 8. Tiến độ, quyết định, nhật ký

### Tiến độ

- [x] Plan v1 (2026-09-27) · [x] Plan v2 — chốt quyết định (2026-09-27)
- [x] 0.1 flag + hằng số
- [x] 1.1 `policy.rs` · [x] 1.2 lỗi `PolicyDenied` · [x] 1.3 `authorize` Allow/Deny · [x] 1.4 timeout client · [x] clippy + format
- [x] ⛔ CHECKPOINT P1
- [x] 2.1 `approval.rs` · [x] 2.2 model + allow-in-session · [x] 2.3 Ask đầy đủ · [x] 2.4 INSTRUCTIONS/skill · [x] clippy + format
- [ ] 3.1 đọc skill · [ ] 3.2 dialog · [ ] 3.3 Workspace · [ ] 3.4 header · [ ] 3.5 toast + palette · [ ] 3.6 review · [ ] clippy + format
- [ ] ⛔ CHECKPOINT P2
- [ ] 4.1 protocol · [ ] 4.2 `agents.toml` · [ ] 4.3 danh tính trong policy · [ ] 4.4 `agent.pair` · [ ] 4.5 client · [ ] 4.6 docs · [ ] 4.7 review + clippy + format
- [ ] ⛔ CHECKPOINT P3 · [ ] tick O2 trong roadmap

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| P1 | 2026-09-27 | Policy chỉ áp cho ghi: `remote.exec`, `remote.file.write`, `remote.exec.visible`. `read`/`output.recent`/`session.list` giữ như O1 | Roadmap: L0 tự chạy; đọc đã được attach ReadOnly/Full kiểm soát |
| P2 | 2026-09-27 | Một hàm `authorize` là chỗ duy nhất gọi `policy::evaluate`, gọi từ `start` (Exec/Write) và `exec_visible` | Đúng ý "một chỗ" của roadmap; `check_access` dùng chung cả đường đọc nên không đặt policy vào đó |
| P3 | 2026-09-27 | `begin_operation` chỉ gọi sau Allow; lệnh visible chỉ gõ vào shell sau Allow | Chờ người ≤ 5 phút không được chiếm slot của session; không bao giờ gõ trước rồi hỏi |
| P4 | 2026-09-27 | Duyệt bằng **hộp thoại modal cấp Workspace** mở khi người bấm **Review** (từ header pane "N waiting" hoặc toast), dialog là cửa sổ nhìn vào **hàng đợi trong `AgentBridgeModel`**. Không tự bật dialog, không đặt lệnh trên header | Header quá hẹp để đọc trọn lệnh; modal tự bật sẽ cướp phím người đang gõ; dialog kiểu Warp Sync là singleton nhưng khi request nằm trong model thì mở request khác không làm mất request cũ |
| P5 | 2026-09-27 | Chờ duyệt = oneshot + `Timer` (5 phút) trong `ctx.spawn`, như `visible::wait`; hết giờ = Deny | Khớp style async của Bridge; không cần state dọn dẹp riêng |
| P6 | 2026-09-27 | Nạp `policy.toml` mỗi request ghi, ở tầng nền; thiếu file = `approve` | Sửa file có hiệu lực ngay, không I/O trên main thread, mặc định an toàn nhưng dùng được |
| P7 | 2026-09-27 | Glob tự viết (`*`, `?`) cho `match` và `[deny] paths`; không thêm crate | Chỉ cần mẫu đơn giản; P6 của roadmap (patch nhỏ) |
| P8 | 2026-09-27 | File lỗi/quyền rộng hơn 0600 → Deny mọi ghi (fail-closed), message chỉ file + lỗi | Thà chặn nhầm còn hơn chạy nhầm dưới root |
| P9 | 2026-09-27 | Thứ tự luật: `require_pairing` → chọn host đầu tiên khớp → `[deny]` (luôn thắng) → mode; `allowlist` không khớp = Ask; mọi `write` = Ask (trừ `read_only`/deny); thêm `[deny] paths` | Roadmap: L3 luôn từ chối, "mọi lệnh ghi khác" là L2; sửa sshd/sudoers là L3 trong roadmap nhưng không phải lệnh nên cần deny theo path |
| P10 | 2026-09-27 | Hộp thoại hiện lệnh/nội dung **nguyên văn** (chỉ bỏ ký tự điều khiển), không redaction | Người duyệt phải thấy đúng thứ sẽ chạy; `****` sẽ che chính thứ đang được duyệt |
| P11 | 2026-09-27 | Lệnh cần duyệt dài > 2 KiB hoặc > 20 dòng → Deny; preview file 40 dòng | Không ai duyệt nổi lệnh 8 KiB; buộc agent chia nhỏ hoặc viết script (được hỏi riêng) |
| P12 | 2026-09-27 | Client (CLI + MCP) cộng vô điều kiện `APPROVAL_TIMEOUT_SECS` (hằng số trong `local_control::protocol`) vào thời gian chờ mọi request ghi | Client không biết trước có bị hỏi; một con số dùng chung hai phía |
| P13 | 2026-09-27 | Audit `started` của O1 ghi sau khi duyệt; thêm bản ghi `approval_requested` (fail-closed) và các trường `policy_decision`/`policy_reason`/`agent_id` | Không có bản ghi "started" mồ côi; vẫn truy được mọi lần hỏi |
| P14 | 2026-09-27 | `remote.exec` ẩn được duyệt vẫn chạy **ẩn** (không tự đổi sang visible); hộp thoại ghi rõ "Runs in the background" | Đổi sang visible sẽ đổi kiểu kết quả (stdout/stderr tách), mất `cwd`, và để lại `cd`/`export` trong shell của user. "Dùng lại exec.visible" của roadmap = lệnh visible chỉ gõ vào shell sau khi duyệt, thành block thật |
| P15 | 2026-09-27 | Token pairing đi trong `RequestEnvelope.agent_token` (newtype che `Debug`), Warp chỉ giữ `sha256`; action `agent.pair` do MCP gọi lười ở tool call đầu của process; không sửa broker | Một trường chung cho mọi action (G2 dùng lại), không đổi chữ ký `local_control::client`; người dùng không phải copy token |
| P16 | 2026-09-27 | Pairing **bền** qua restart (`~/.warp/agent-ops/agents.toml`, 0600), khác attach (chỉ RAM) | Pairing không cấp quyền, chỉ gắn danh tính; hỏi lại mỗi lần mở Warp sẽ làm người dùng bấm theo phản xạ |
| P17 | 2026-09-27 | Dùng danh tính: hiện trong hộp thoại + audit + `require_pairing` (mặc định false). Không có luật policy theo từng agent ở O2 | Đủ cho gate O2 và điều kiện AO7 của G2; luật theo agent để O6 |
| P18 | 2026-09-27 | "Allow this command in this session" lưu trong `Attachment` (RAM), khớp nguyên văn sau trim, chỉ cho lệnh; `[deny]` vẫn thắng | Roadmap liệt kê nút này; tự mất khi detach/hết hạn/rời `sudo -i` |
| P19 | 2026-09-27 | Flag riêng `AgentOpsPolicy` (cargo feature `agent_ops_policy`); tắt = y hệt O1 | Patch sau feature flag (P6 của roadmap); không đổi hành vi người đang dùng O1 |
| P20 | 2026-09-27 | Ba checkpoint: P1 (CLI, Allow/Deny) → P2 (UI duyệt) → P3 (pairing) | Bắt lỗi wiring trước khi làm UI; pairing tách riêng để O2 lõi dùng được sớm |
| P21 | 2026-09-27 | Phase 0 (chỉ Task 0.1) không chạy clippy 3 package cuối phase, chỉ `cargo check` như mục 4 (Phase 0) đã nêu; clippy+format thật sự chạy ở cuối Phase 1 | Hằng số `APPROVAL_*`/`POLICY_FILE`/`AGENTS_FILE`/`MAX_PENDING_APPROVALS_PER_SESSION` thêm ở 0.1 chưa được dùng tới Task 1.1 → `cargo clippy -D warnings` báo `dead_code` là lỗi thật, không phải lỗi code; mục 0.8 ("cuối mỗi Phase") là quy tắc chung, còn văn bản riêng của Phase 0 chỉ yêu cầu `cargo check` — theo văn bản riêng, cụ thể hơn |
| P22 | 2026-09-27 | `AGENTS_FILE` (Phase 4), `MAX_PENDING_APPROVALS_PER_SESSION` (Task 2.1), `APPROVAL_PREVIEW_LINES` (Task 3.7) thêm sẵn ở 0.1 nhưng chưa có chỗ dùng trong Phase 1 → gắn `#[allow(dead_code)]` (kèm comment nêu task nào sẽ dùng) ngay trước khi chạy clippy cuối Phase 1; xoá từng `allow` khi task tương ứng dùng tới hằng số đó | Đúng ý Task 0.1 của plan (thêm hằng số cả 4 phase một lần, tên `mod.rs` chung); `cargo check` không báo lỗi (chỉ warning) nên không chặn task 0.1–1.4, chỉ chặn ở bước clippy `-D warnings` cuối phase — vá đúng chỗ chặn, không đổi thiết kế hằng số |

### Nhật ký

- 2026-09-27 — Plan v1 (Claude Sonnet 5): khảo sát `handlers/remote.rs`, `bridge.rs`/`mod.rs`,
  `operations.rs`, `error.rs`, `protocol.rs`, `auth.rs`, `confirm_dialog.rs` + Workspace, `pane_impl.rs`,
  `mcp/jsonrpc.rs`, `app/Cargo.toml`; để 5 nhóm quyết định chờ duyệt.
- 2026-09-27 — Plan v2 (Claude Opus 5.5): người dùng giao quyền quyết định. Kiểm thêm: timeout client
  (`EXEC_CLIENT_MARGIN`), broker `issue_credential`, `RequestEnvelope` (không `deny_unknown_fields`),
  `send_action`, `DismissibleToast`/`ToastLink`, `focus_pane`/`activate_tab_by_pane_group_id`, inline
  banner của blocklist (cân nhắc, không dùng: cần sửa blocklist dưới lock model), `Attachment`,
  `AgentBridgeModel`. Chốt P1–P20, viết đầy đủ Phase 0–4 (pairing là Phase 4) và checklist 5.P1–5.P3.
  Sửa v1: đường dẫn `crates/warp_cli/src/local_control/mcp/jsonrpc.rs`; bỏ phương án banner-có-nút trên
  header (không đủ chỗ hiện lệnh). Chưa bắt đầu Phase 0.
- 2026-09-27 — Task 0.1 (Claude Sonnet 5): `FeatureFlag::AgentOpsPolicy` cạnh `AgentBridge`
  (`crates/warp_features/src/lib.rs`), cargo feature `agent_ops_policy` (`app/Cargo.toml`, không thêm
  vào `default`), ánh xạ `#[cfg(feature = "agent_ops_policy")]` (`app/src/features.rs`, cạnh
  `agent_bridge`); không thêm vào `DOGFOOD_FLAGS`. `APPROVAL_TIMEOUT_SECS = 300` trong
  `crates/local_control/src/protocol.rs`. Hằng số Phase 1+ trong `app/src/agent_bridge/mod.rs`:
  `APPROVAL_MAX_COMMAND_BYTES`, `APPROVAL_MAX_COMMAND_LINES`, `APPROVAL_PREVIEW_LINES`, `POLICY_FILE`,
  `AGENTS_FILE`, `MAX_PENDING_APPROVALS_PER_SESSION` (chưa dùng tới Task 1.x nên `cargo check` báo
  dead-code warning — không phải lỗi). `cargo check -p warp -p local_control -p warp_cli` và
  `cargo check -p warp --features agent_ops_policy` đều qua.
- 2026-09-27 — Task 1.1 (Claude Sonnet 5): `app/src/agent_bridge/policy.rs` (+ `policy_tests.rs`,
  44 test) đúng thiết kế mục 3.2: `Mode`/`Rule`/`HostRule`/`Policy`/`PolicyRequest`/`Decision`/
  `PolicyError` (thiserror), `Policy::parse`/`evaluate`, `load(home)`, glob `*`/`?` tự viết
  (thuật toán wildcard-matching kinh điển, so theo `char` chứ không theo byte). Xác nhận
  `app/Cargo.toml` đã có `toml`, `regex`, `sha2`, `thiserror`, `tempfile`, `uuid` — không cần thêm
  dependency. `cargo test -p warp --lib agent_bridge::policy`: 44 passed. `AGENTS_FILE`,
  `MAX_PENDING_APPROVALS_PER_SESSION`, `APPROVAL_PREVIEW_LINES` vẫn chưa dùng (Phase 2–4) — sẽ cần
  `#[allow(dead_code)]` tạm thời trước khi chạy clippy cuối Phase 1 (xem P22).
- 2026-09-27 — Task 1.2 (Claude Sonnet 5): `ErrorCode::PolicyDenied` (`crates/local_control/src/
  protocol.rs`, serde `policy_denied`) + `AgentBridgeError::PolicyDenied(String)` →
  `"Denied by Warp's agent policy: {reason}"` (`app/src/agent_bridge/error.rs`), map sang
  `ErrorCode::PolicyDenied`. Compiler chỉ ra đúng một match không tổng quát:
  `app/src/agent_bridge/ops.rs::commit_upload` (`scratch_may_remain`) — xếp `PolicyDenied` vào
  nhánh `false` (bị từ chối trước khi chạm script trên server nên không có gì để dọn). Test:
  `error_tests.rs` (mã + message), `protocol_tests.rs::remote_error_codes_serialize_as_machine_codes`
  (thêm case `policy_denied`). `cargo test -p warp -p local_control --lib`: pass.
- 2026-09-28 — Task 1.3 (Claude Sonnet 5): `authorize`/`deny_authorization`/`evaluate_policy`
  (`handlers/remote.rs`, mục 3.6) nối vào `start` (Exec/Write; Read bỏ qua — L0) và `exec_visible`.
  `begin_operation` tách khỏi `start`/`exec_visible` thành `run_hidden_operation`/
  `run_visible_operation`, chỉ gọi **sau** Allow (P3) — hai hàm này tự phát mọi lỗi (kể cả lỗi
  `begin_operation`) qua `sender`, vì `start`/`exec_visible` đã trả `receiver` cho caller trước khi
  hàng nền chạy xong. Đổi hình dạng closure so với gợi ý 3.6: một `continue_with:
  FnOnce(Result<Option<&'static str>, AgentBridgeError>, &mut ModelContext<..>)` thay vì
  `proceed`/`fail` riêng — hai closure riêng đều cần sở hữu `oneshot::Sender` (không `Clone`) nên
  không compile; mục 3.6 cho phép đổi hình dạng khi `'static` gây khó (lý do ghi ngay trong
  doc-comment của `authorize`, không thêm mục Quyết định riêng vì không đổi thiết kế, chỉ đổi hình
  dạng closure). Ask (Phase 2 mới có) tạm là Deny("approval is not available yet"). Audit: thêm
  `policy_decision`/`policy_reason` vào `AuditRecord` (`audit.rs`) và `Target.policy_decision`
  (`ops.rs`) để bản ghi "started" mang nhãn `allow`; request bị Deny chỉ có một dòng audit qua
  `ops::audit_policy_denied` (không có dòng "started" mồ côi — P13). Test: không có harness nào
  trong repo dựng được một session `WarpifiedRemote` thật để gọi qua HTTP tới `authorize` (mọi test
  `remote_tests.rs` hiện có dừng ở `StaleTarget`/`InvalidParams`, trước `check_access`) — viết test
  trực tiếp cho phần thuần của `handlers/remote.rs`: `Operation::policy_subject()` (Read → `None`,
  Exec/Write → `Some`) và `evaluate_policy()` với `$HOME` tạm (`#[serial_test::serial]`, lưu/khôi
  phục `HOME` như `git_credentials_tests.rs`). Logic `Policy::evaluate` đã có 44 test riêng ở 1.1.
  `cargo test -p warp -p warp_cli --lib -- agent_bridge local_control`: 104 passed. `cargo check`
  (thường và `--features agent_ops_policy`): qua (còn 3 warning dead-code đã biết, xem P22).
- 2026-09-28 — Task 1.4 (Claude Sonnet 5): `APPROVAL_CLIENT_MARGIN = Duration::from_secs(
  local_control::protocol::APPROVAL_TIMEOUT_SECS)` (`warp_cli/src/local_control/remote.rs`), cộng
  vô điều kiện vào `wait` của `exec`/`exec --visible`/`write` — CLI (`remote.rs`) và MCP
  (`mcp/tools.rs`: `exec`, `exec_visible`, `write_raw` dùng chung cho `write_file`/`edit_file`).
  `read`/`read_raw`/`recent`/`sessions` (L0) giữ nguyên timeout. Sửa 3 assertion timeout có sẵn
  trong `tools_tests.rs` + thêm assertion timeout cho `write_file_creates_a_missing_file`. `cargo
  test -p warp -p warp_cli --lib -- mcp::tools`: 24 passed (chạy riêng `-p warp_cli` build lỗi
  `yeslogic-fontconfig-sys`/`pkg-config` thiếu `fontconfig.pc` — không liên quan thay đổi này; né
  bằng cách build kèm `-p warp`).
- 2026-09-28 — Cuối Phase 1 (Claude Sonnet 5): gắn `#[allow(dead_code)]` (kèm task sẽ dùng, theo
  P22) cho `APPROVAL_PREVIEW_LINES`/`AGENTS_FILE`/`MAX_PENDING_APPROVALS_PER_SESSION`. `cargo
  clippy -p warp -p local_control -p warp_cli --all-targets --tests -- -D warnings`: sạch (exit 0).
  `./script/format` một lần — format lại `policy.rs`, `policy_tests.rs`, `handlers/remote.rs`,
  `mcp/tools.rs`, `warp_cli/local_control/remote.rs` (chủ yếu bọc dòng `wait = ... +
  APPROVAL_CLIENT_MARGIN` quá 100 cột); không chạy lại test/lint sau format theo mục 0.8. **⛔
  CHECKPOINT P1 đạt** — checklist 5.P1 sẵn sàng cho người dùng test tay (cần build với feature
  `agent_ops_policy`, xem mục 0.12).
- 2026-09-28 — Người dùng xác nhận checklist 5.P1 test tay đạt ("tôi test ok").
- 2026-09-28 — Task 2.1 (Claude Sonnet 5): `app/src/agent_bridge/approval.rs` + `approval_tests.rs`
  (27 test cộng với model_tests.rs bên dưới) đúng thiết kế mục 3.4: `ApprovalSubject`/
  `ApprovalRequest`/`ApprovalDecision`/`ApprovalQueue` (push/decide/remove/revoke_session/
  revoke_all/get/count_for_session/oldest_for_session/oldest_in_window), `wait_for_decision`
  bằng `futures::future::select(receiver, Timer::after(timeout))` như `visible.rs` — Canceled →
  `Revoked` theo đúng comment gợi ý trong plan. `revoke_where` dùng `Vec::partition` thay vì
  `retain` vì `retain` chỉ cho `&T`, không lấy được quyền sở hữu `Sender` để `send()`. Test race
  "timeout thắng" (mục 3.4): `decide` rồi `remove` trả `false` — không có race thật (single-
  threaded), chỉ kiểm hành vi khi hai lệnh gọi tới cùng lúc nhau. `agent: Option<String>` (tên tự
  khai) thay vì kiểu `AgentLabel` đầy đủ của bản vẽ mục 3.4 — đúng ghi chú trong mục 3.4 "trước
  khi có Phase 5 chỉ có tên"; `AgentLabel` sẽ thêm ở Task 4.3. Mở khoá
  `MAX_PENDING_APPROVALS_PER_SESSION` (không còn `#[allow(dead_code)]`, theo P22).
- 2026-09-28 — Task 2.2 (Claude Sonnet 5): `AgentBridgeModel` thêm field `approvals: ApprovalQueue`
  và `type Event = AgentBridgeEvent` (`ApprovalRequested { request_id, window_id }` /
  `ApprovalsChanged`) — đổi từ `()`; `ctx.observe` hiện có ở `terminal/view.rs:4227` không phụ
  thuộc kiểu `Event` (generic trên model, chỉ cần `notify()`) nên không phải sửa gì thêm.
  `AgentBridgeEvent` phải là `pub` (không phải `pub(crate)`) vì `AgentBridgeModel` là `pub struct`
  — Rust từ chối leak kiểu `pub(crate)` qua associated type của một struct `pub` (E0446).
  `detach`/`detach_all` gọi thêm `approvals.revoke_session`/`revoke_all` (mục 2.4: kill switch).
  `Attachment` thêm `allowed_commands: HashSet<String>` + `Attachments::allow_command`/
  `is_command_allowed` (mục 3.5); test trong `attachments_tests.rs`. Viết mới
  `app/src/agent_bridge/model_tests.rs` (chưa có trước đây) theo mẫu `App::test` của
  `active_agent_views_model_tests.rs` — test các wrapper mới của model
  (push/decide/remove/deny_all_approvals, pending/oldest theo session và window, detach/
  detach_all revoke, allow-in-session) thay vì cố chặn bắt sự kiện `emit` (không có helper thu sự
  kiện sẵn trong repo; gọi từng hàm với assertion trên trạng thái quan sát được là đủ, việc emit
  thật sẽ được test ở tầng Workspace lúc Phase 3 dùng tới).
- 2026-09-28 — Task 2.3 (Claude Sonnet 5): nối `Decision::Ask` vào `authorize`
  (`handlers/remote.rs`) — `ask_authorization` (kiểm allow-in-session trước, rồi `push_approval` +
  audit `approval_requested` fail-closed trong `ctx.spawn`, rồi `wait_for_decision`) và
  `finish_ask` (Approve/AllowInSession kiểm lại `check` rồi mới chạy; Deny/TimedOut/Revoked qua
  `deny_authorization` với nhãn `policy_decision` riêng — `ask_denied`/`ask_timeout`/`revoked`).
  `deny_authorization`/`ops::audit_policy_denied` thêm tham số `policy_decision: &'static str`
  (trước đây khoá cứng `"deny"`) để dùng chung cho cả nhánh Deny thẳng lẫn ba nhánh Ask thất bại,
  đúng danh sách nhãn ở mục 3.10. Thêm `ops::audit_approval_requested` +
  `AuditOutcome::ApprovalRequested` (`audit.rs`). `AuthorizeInput` thêm `session: SessionId`,
  `window_id: WindowId` (lấy qua `ViewHandle::window_id(ctx)`, gọi trước khi `snapshot` bị move —
  `SessionSnapshot` cho phép partial move nên vẫn đọc được `terminal_view` sau khi lấy
  `snapshot.session`/`.session_id`/`.cwd`). `PolicySubject::Write` thêm `content_base64`/`creates`
  (chỉ dùng khi dựng `ApprovalSubject::Write` cho hộp thoại — `evaluate_policy`/audit deny vẫn chỉ
  cần `path`) + hàm thuần `write_preview()` (decode base64, cắt `APPROVAL_PREVIEW_LINES` dòng,
  UTF-8 lỗi → "(binary, N bytes)"; base64 lỗi → preview rỗng thay vì lỗi cứng, vì `run_write` sẽ tự
  từ chối nội dung hỏng sau nếu request được duyệt). Không thêm audit riêng cho trường hợp
  "duyệt rồi nhưng session đã detach lúc đang chờ" (`finish_ask`'s recheck lỗi) — theo đúng cách
  `check_access` hiện có không audit khi từ chối trước khi chạm server, không phải quyết định mới.
  Không viết được test qua HTTP harness cho toàn bộ luồng Ask (giống phát hiện ở Task 1.3:
  `remote_tests.rs` không dựng được session `WarpifiedRemote` thật) — phần thuần/hàng đợi/model đã
  có 27+15 test ở 2.1/2.2; luồng đầy đủ (Ask → Approve/Deny/Timeout/Revoke → chạy hay không, detach
  giữa chừng → `session_not_attached`) để checklist 5.P2 test tay xác nhận, đúng như cách 1.3 đã
  làm với Allow/Deny. `cargo test -p warp -p local_control -p warp_cli --lib -- agent_bridge
  local_control`: 286 + 104 passed.
- 2026-09-28 — Task 2.4 (Claude Sonnet 5): thêm 2 câu vào `INSTRUCTIONS` (`mcp/tools.rs`) và
  `specs/agent-bridge/claude/SKILL.md` đúng nguyên văn mục 3.9. `policy_denied` là `isError` không
  cần sửa gì: `jsonrpc_tests.rs::a_failed_tool_is_a_result_not_a_protocol_error` đã kiểm chung cho
  mọi `ToolError` — chỉ xác nhận lại, không thêm test mới (đúng "chỉ kiểm" của mục 3.9).
- 2026-09-28 — Cuối Phase 2 (Claude Sonnet 5): `cargo clippy -p warp -p local_control -p warp_cli
  --all-targets --tests -- -D warnings` lần đầu báo `dead_code` cho phần chỉ dùng ở Phase 3 (biến
  thể/enum của `approval.rs`, wrapper của `model.rs` không ai gọi tới ngoài UI chưa viết) — khác
  Task 0.1: lần này `cargo check` thường (không `--tests`) đã thấy trước cùng lỗi vì các mục đó chỉ
  được gọi từ những hàm khác cũng chưa ai gọi (không giống P22, nơi hằng số chỉ chờ 1 task ngay
  sau); áp `#[allow(dead_code)]` mức field/variant/method (kèm task Phase 3 sẽ dùng, theo đúng tinh
  thần P22) cho `ApprovalDecision::Approve`/`Deny`, `ApprovalQueue::decide`/`get`/
  `oldest_for_session`/`oldest_in_window`, và 6 wrapper của `AgentBridgeModel`
  (`decide_approval`/`deny_all_approvals`/`approval`/`pending_approvals_for_session`/
  `oldest_approval_for_session`/`oldest_approval_in_window`). Riêng lần thử đầu dùng `let _ = queue
  .push(...).expect(...)` để im `#[must_use]` của `oneshot::Receiver` bị clippy báo lỗi
  `let_underscore_future` (`Receiver` là `Future`) — đổi sang `drop(queue.push(...).expect(...))`
  theo đúng gợi ý của clippy. `cargo clippy ... -D warnings`: sạch (exit 0). `./script/format` một
  lần; không chạy lại test/lint sau format theo mục 0.8. 4 commit riêng cho 2.1–2.4 (rule 9: mỗi
  task một commit) + `docs(agent-ops)` này; chưa `git push`.
