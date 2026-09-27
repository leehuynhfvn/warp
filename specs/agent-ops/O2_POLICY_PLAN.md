# Policy + duyệt phía Warp — O2 — Implementation Plan (v1)

> Người thực thi: một coding agent (Claude Sonnet). Làm **tuần tự từng Phase**, dừng ở mọi
> **CHECKPOINT** để người dùng xác nhận. Không tự ý mở rộng phạm vi. Plan viết ngày 2026-09-27
> (Claude Sonnet 5) sau khi khảo sát code thật của `feature/agent-bridge` (O1, xong cùng ngày —
> xem mục 8 của `specs/agent-bridge/IMPLEMENTATION_PLAN.md`, D1–D32).
>
> Plan này là phase **O2** của `specs/agent-ops/ROADMAP.md` (mục "O2 — Policy + duyệt phía Warp"),
> cộng phần **pairing token** mà roadmap ghi chú "chuyển từ Phase 5 của plan Bridge, D27 ở đó, làm
> cùng O2" (mục "G", Phase 5 §"Pairing token"). Phạm vi **không** gồm G (Warp làm cổng SSH cho
> agent) — roadmap yêu cầu G2 chỉ bắt đầu **sau** O2 (AO7).
>
> **File này có một số quyết định thiết kế chưa chốt** vì roadmap mô tả ở mức mục tiêu, không phải
> API — mục 1.3 và 3.x đánh dấu rõ bằng **⚠ Cần duyệt**. Không code phần nào có đánh dấu đó cho tới
> khi người dùng chọn phương án.

---

## 0. Quy tắc bắt buộc cho agent thực thi

1. Đọc `AGENTS.md` trước. Skill liên quan trong `.agents/skills/`: `gui-ui-guidelines` (mọi việc
   đụng UI — đọc **trước** khi viết dialog/banner ở Phase 3), `add-feature-flag` (Phase 0),
   `rust-unit-tests`, `logging-and-error-reporting`. Đọc `SKILL.md` tương ứng trước khi dùng.
2. Làm tiếp trong worktree hiện có `/projects/github/warp-agent-bridge`, nhánh `feature/agent-bridge`
   (đã có O1 xong, không tạo worktree mới). Trước khi bắt đầu Phase 0: `git status` sạch, kiểm
   `git log -3` đúng là các commit Phase 5 của O1 (`884d30bd7` trở về trước).
3. Mọi lệnh cargo chạy với `export CARGO_TARGET_DIR=/projects/github/warp/target`, kèm `-p warp`
   (D14 của plan Bridge — thiếu nó `-p warp_cli`/`-p local_control` đứng riêng build fail vì
   `fontconfig-devel`). Ví dụ: `cargo test -p warp -p local_control -p warp_cli --lib -- agent_bridge`.
4. Build để test tay: `./script/run --features warp_control_cli,warp_sync,agent_bridge` (thêm feature
   mới của phase này nếu Task 0.1 quyết định thêm, vd `agent_ops_policy`) từ
   `/projects/github/warp-agent-bridge`. Binary MCP dùng để test:
   `/projects/github/warp-agent-bridge/target/debug/warp-oss` (không theo `CARGO_TARGET_DIR`, xem
   nhật ký Checkpoint B của plan Bridge).
5. Test `agent_bridge` (dùng `sh` thật) và `terminal::view` nhạy với tải khi chạy song song — trước
   khi kết luận là lỗi thật, chạy lại riêng `cargo test ... -- --test-threads=1`.
6. Sau MỖI task: `cargo check` cho package đã sửa. Không commit code không compile. Không
   `unwrap()`/`expect()` trên dữ liệu từ file policy, request local-control, hoặc input UI. Match
   exhaustive, không `_` nếu tránh được. `ctx` là tham số cuối. Comment chỉ giải thích "why". Format
   args inline. Không prefix `_` cho tham số thừa — xoá hẳn.
7. Unit test trong `<name>_tests.rs`, include bằng `#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;`.
8. Cuối mỗi Phase: `cargo clippy -p warp -p local_control -p warp_cli --all-targets --tests -- -D warnings`.
   `./script/format` chỉ chạy **một lần**, ở task cuối cùng của cả plan (như Task 5.10 của Bridge).
   Không chạy `./script/presubmit`.
9. Commit theo conventional commits (`feat(agent-ops): ...`), mỗi task một commit nhỏ.
10. Gặp API không tồn tại / chữ ký khác plan, hoặc gặp một mục đánh dấu **⚠ Cần duyệt** chưa có
    quyết định ghi ở mục 8 → **dừng và hỏi**, không bịa API, không tự chọn phương án.
11. File này là **nguồn sự thật duy nhất** của phase O2. Bắt đầu phiên: đọc mục 8. Sau mỗi task đã
    commit: tick checkbox + thêm 1 dòng "Nhật ký" ở mục 8 (commit cùng task). Lệch plan → ghi vào
    "Quyết định" kèm lý do. Không tạo `.context/`.
12. Sau khi Phase chính (0–4) xong và CHECKPOINT tương ứng đạt, **dừng lại hỏi người dùng** trước khi
    làm Phase 5 (pairing token) — mục "G" của roadmap và bảng 1.3(d) coi đây là phần có thể lùi.

---

## 1. Bài toán và kết quả khảo sát

### 1.1 Mục tiêu

O1 (Agent Bridge) để việc "duyệt từng thao tác" hoàn toàn cho **Claude Code** (permission prompt của
MCP client) — mục 2.3 của plan Bridge ghi rõ: "Do Claude Code đảm nhiệm". Điều đó có ba lỗ hổng mà
roadmap (P4, AO3) muốn đóng:

1. Agent khác (Codex, Gemini CLI, hay CLI chạy `--dangerously-skip-permissions`/tương đương) có thể
   bỏ qua permission prompt của chính nó — Warp không có lớp chặn nào độc lập.
2. Không có nơi nào để người dùng nhìn thấy **đúng lệnh sẽ chạy trên server, dưới quyền root** trước
   khi nó chạy — permission prompt của agent chỉ hiện tên tool, không phải nội dung đã redact/thật.
3. Không phân biệt được "lệnh đã biết là an toàn, cho chạy thẳng" (vd `systemctl reload nginx`) với
   "lệnh lạ, phải hỏi" — mọi `remote.exec`/`remote.file.write` đều chạy ngay khi agent gọi (miễn đã
   `Full` access).

O2 thêm một lớp **chính sách + hộp thoại duyệt phía Warp**, độc lập với agent gọi nó, cho các hành
động **ghi** (`remote.exec`, `remote.file.write`, `remote.exec.visible`). Theo mục 3 của roadmap:

| Mức | Hành động | O2 làm gì |
|---|---|---|
| L0 | Đọc (`remote.file.read`, `remote.output.recent`, `remote.session.list`) | Không đổi — vẫn tự chạy như O1, chỉ cần đã attach |
| L1 | Runbook đã duyệt trước, khớp **nguyên văn** | Tự chạy nếu khớp allowlist của host, có ghi audit |
| L2 | Mọi lệnh ghi khác | Hộp thoại duyệt trong Warp, timeout 5 phút → Deny |
| L3 | Denylist (`rm -rf /`, `mkfs`, `dd if=`, shutdown/reboot, flush firewall, `DROP DATABASE`, …) | Luôn từ chối, kể cả nếu khớp allowlist |

Cộng thêm (theo mục "G" của roadmap, chuyển sang đây theo D27 của plan Bridge):

- **Pairing token**: MCP client ghép cặp một lần với Warp, để trường `agent` trong hộp thoại/audit
  là danh tính đã xác minh thay vì tên tự khai (D12 của Bridge). Đánh dấu **⚠ Cần duyệt** — xem 1.3(d).
- Hộp thoại duyệt dùng lại `remote.exec.visible` (Phase 5 của Bridge): lệnh đã duyệt chạy thành
  **block thật** trong pane, người dùng thấy đúng những gì chạy.

**Gate → O3/O4** (roadmap): dùng hằng ngày ≥ 1 tuần trên host lab không sự cố; mọi lệnh ghi đều qua
hộp thoại hoặc allowlist; audit log đủ.

### 1.2 Hiện trạng code (đã kiểm chứng)

| Thành phần có sẵn | Ý nghĩa với O2 | Bằng chứng |
|---|---|---|
| `handlers/remote.rs::check_access` là **hàm duy nhất** kiểm quyền attach, gọi từ `start` (Exec/Read/Write), `output_recent`, `exec_visible` | Không có "một chỗ gọi policy duy nhất" theo nghĩa đen (3 hàm gọi vào). Nhưng chỉ **`start`** (khi `Operation::Exec`/`Operation::Write`) và **`exec_visible`** làm việc ghi — `output_recent`/`Operation::Read` là L0, không cần qua policy. Thiết kế: thêm `authorize()` bọc `check_access` + `policy::evaluate`, gọi từ đúng 2 chỗ đó (mục 3.5). | `app/src/local_control/handlers/remote.rs:96` (`Operation::needed_access`), `:296` (`check_access`), gọi từ `:116` (`start`), `:154` (`output_recent`, `Access::ReadOnly` — không đổi), `:184` (`exec_visible`) |
| `Operation::run`/`send_visible_command` là nơi lệnh **thật sự** rời main thread (Exec/Write) hoặc được gõ vào PTY (Visible) | Policy phải chặn **trước** hai điểm này. Với `exec_visible`, phải chặn **trước khi gõ vào shell** — không được gõ rồi mới hỏi. | `remote.rs:82` (`Operation::run`), `:265` (`send_visible_command`, gọi `try_execute_command_preserving_input`) |
| `BridgeResult::Pending{request_id, receiver}` đã có từ Warp Sync; `start`/`exec_visible` trả `Result<RemoteReceiver, ControlError>` rồi bridge bọc thành `Pending` | Không cần đổi protocol HTTP: việc "chờ duyệt" nằm **trong** future đã `ctx.spawn`, y hệt cách `visible::wait` (D29) chờ block xong bằng poll + `Timer::after`. Không cần thêm biến thể `BridgeResult` mới. | `app/src/local_control/bridge.rs:24-33`, `app/src/local_control/mod.rs:600-612` |
| `exec_visible` đã là **chuỗi `ctx.spawn` nhiều tầng**: ghi audit `started` (fail-closed) → callback main-thread gõ lệnh → `ctx.spawn` chờ block | Đúng khuôn để chèn thêm một tầng "chờ duyệt" ở **đầu** chuỗi, trước cả `begin_operation(Visible)`. `start` (Hidden) hiện là **một** `ctx.spawn` duy nhất — phải tách thành 2 tầng giống `exec_visible` để chèn chờ duyệt trước `begin_operation(Hidden)`. | `remote.rs:184-260` |
| `agent_bridge::operations::Operations` (Hidden/Visible, không cho visible chạy chồng bất kỳ gì khác) | Nếu `begin_operation` được gọi **trước** khi chờ duyệt, một yêu cầu đang "Ask" (có thể chờ tới 5 phút) sẽ **chiếm slot Visible** của session, khoá luôn mọi `exec`/`write` ẩn khác trong lúc chờ người bấm. Quyết định: `begin_operation` chỉ gọi **sau** khi có Allow (kể cả Allow do duyệt) — slot chỉ bị chiếm trong lúc lệnh **thật sự chạy**, không phải lúc chờ người. | `app/src/agent_bridge/operations.rs` (toàn file, xem trích đầy đủ dưới đây) |
| `agent_bridge/error.rs::AgentBridgeError` + `From<AgentBridgeError> for ControlError` là **một** hàm ánh xạ sang `ErrorCode` | Thêm biến thể mới (`PolicyDenied`) và một `ErrorCode` mới cùng cách. | `app/src/agent_bridge/error.rs:9-50` |
| `crates/local_control/src/protocol.rs::ErrorCode` (14 biến thể, exhaustive match ở nhiều nơi: `warp_cli` output, MCP `format.rs`, …) | Thêm biến thể mới nghĩa là sửa mọi match exhaustive theo compiler — như D1 của mục 3.1 Bridge đã làm với 4 mã lỗi mới. | `crates/local_control/src/protocol.rs:867-896` |
| `app/src/warp_sync/confirm_dialog.rs::WarpSyncConfirmDialog` — dialog **singleton theo Workspace** (`ViewHandle` giữ trong `WorkspaceView`, `is_warp_sync_confirm_dialog_open: bool`), `set_request()` **thay thế** request đang hiện nếu đã mở (trả lại request cũ để hàm gọi tự huỷ nó) | Là "hộp thoại xác nhận" thật duy nhất trong repo giống thứ O2 cần, nhưng: (a) **không có timeout** — phải tự thêm; (b) **chỉ một dialog tại một thời điểm cho cả Workspace** — nếu 2 session khác nhau cùng "Ask" một lúc, dialog thứ hai đá dialog thứ nhất (mất, không tự Deny request bị đá — phải tự làm). Bảng 1.3(a) cân nhắc dùng lại mẫu này hay không. | `app/src/warp_sync/confirm_dialog.rs:311-356`, chỗ gắn vào Workspace: `app/src/workspace/view.rs:1136,2030-2033,3137,19152-19270,28261-28266` |
| Pane header indicator của O1 (D26): `agent_bridge_access()` đọc `AgentBridgeModel::as_ref(app).status(session.id())`, vẽ icon + nhãn + nút Revoke — chỉ hiện khi session đó đang là **active session của pane đang mở** | Mẫu đúng để hiện trạng thái "đang chờ duyệt" theo session, nhưng có cùng giới hạn: nếu người dùng không đang nhìn đúng pane đó, không thấy gì. Cần thêm kênh khác (toast) cho trường hợp đó — xem mục 3.6. | `app/src/terminal/view/pane_impl.rs:1006-1090` |
| `crates/local_control/src/mcp/jsonrpc.rs::McpHandler::set_client_name`, gọi trong `initialize()` với `clientInfo.name` | Điểm duy nhất phía MCP server biết tên client tự khai — đúng chỗ để **thêm** bước ghép cặp (gửi/nhận secret) nếu làm pairing token, nhưng bản thân MCP `initialize` không có chỗ cho client gửi thêm secret ngoài `clientInfo` (không phải trường chuẩn của MCP) → phải nghĩ theo hướng khác (mục 1.3(d)/3.10). | `crates/warp_cli/src/local_control/mcp/jsonrpc.rs:43-55,113-140` |
| `crates/local_control/src/auth.rs::CredentialGrant` là credential theo **1 action, TTL ngắn** (broker cấp lại mỗi lần gọi), không phải danh tính bền theo agent | Pairing không phải là mở rộng của cơ chế này (khác tầng: mỗi request local-control vẫn xin credential riêng qua broker UID như cũ) — pairing là một **danh tính bền** nằm phía trên, được validate thêm trong request `remote.*`. | `crates/local_control/src/auth.rs` (toàn file) |
| `app/Cargo.toml` đã có `toml = "0.8.13"` và `regex.workspace = true` (D23 của Bridge đã dùng `regex` cho `mcp/redact.rs`, nhưng đó là crate `warp_cli`; `app` cũng có sẵn `regex` + `toml` — không cần thêm dependency mới cho `policy.rs`) | Đỡ một task thêm dependency. | `app/Cargo.toml:172,173,211` |
| Audit log cục bộ: `app/src/agent_bridge/audit.rs`, thư mục `~/.warp/agent-bridge/audit.jsonl` (khác `~/.warp-agent/backups` — đó là thư mục **trên server**, tạo bởi script ghi file, không liên quan) | Policy file theo roadmap nằm ở `~/.warp/agent-ops/policy.toml` — **khác thư mục** với audit (`agent-bridge` vs `agent-ops`). Giữ đúng như roadmap ghi (đây là namespace rộng hơn, G1 cũng dùng `~/.warp/agent-ops/hosts.toml`), nhưng ghi rõ để không nhầm hai thư mục. | `app/src/agent_bridge/audit.rs:15-16,62`; roadmap mục "O2" (khối `policy.toml`), mục "G1a" (`hosts.toml`) |
| `warp_sync::paths::create_private_dir_all` (`app/src/warp_sync/paths.rs:302`) | Dùng lại để tạo `~/.warp/agent-ops/` với quyền đúng (đã `pub(crate) mod paths` re-export từ Task 1.6 của Bridge, D16) thay vì viết lại. | `app/src/warp_sync/paths.rs:302`, `app/src/agent_bridge/audit.rs:14` (cách dùng) |
| `crates/warp_features/src/lib.rs:1011` (`FeatureFlag::AgentBridge`), `app/src/features.rs:110-111` (bật qua cargo feature `agent_bridge`) | Mẫu để thêm flag mới cho O2 (mục 3.1) — theo skill `add-feature-flag`. | như trên |
| `app/src/ai/blocklist/permissions.rs` — mô hình duyệt lệnh của **Agent Mode** (không phải Agent Bridge): `CommandExecutionPermission::{Allowed(reason), Denied(reason), Ask}`-kiểu, hiện UI **trong khối chat** (nút Run/Always Allow/Deny), không phải modal | Cùng ý tưởng ba trạng thái Allow/Ask/Deny nhưng **UI khác hẳn ngữ cảnh** (chat block của Agent Mode, không phải pane terminal của một session SSH). Không dùng lại được UI, nhưng xác nhận cụm từ `Allow/Ask/Deny` là mẫu quen thuộc trong code, không phải thuật ngữ tự bịa. | `app/src/ai/blocklist/permissions.rs:34-64,940-1000` |

### 1.3 Các phương án đã cân nhắc

**(a) Nơi hiện hộp thoại duyệt — ⚠ Cần duyệt**

| Phương án | Ưu | Nhược | 
|---|---|---|
| **A. Modal toàn cục kiểu `WarpSyncConfirmDialog`** | Có mẫu sẵn để copy gần nguyên (title/body/2 nút, focus, Esc không tự Confirm) | Một Workspace chỉ hiện được 1 dialog: 2 session khác nhau cùng "Ask" → dialog sau đá dialog trước; dialog che toàn bộ, không thấy pane đứng sau để đối chiếu ngữ cảnh |
| **B. Banner gắn trong pane header** (mở rộng D26: thêm trạng thái "Pending approval" cạnh "Full"/"ReadOnly", có nút Approve/Deny ngay trên header) | Theo đúng session, nhiều session hỏi cùng lúc không đụng nhau, thấy pane thật khi quyết định | Chỉ thấy khi đang mở đúng pane/tab đó; cần thêm kênh phụ nếu người dùng đang ở tab khác |
| **C. B + toast khi có request mới** (dùng lại `WorkspaceView` toast, Task 2.6 của Bridge) đưa người dùng chú ý, click toast → focus đúng tab/pane | Giải quyết nhược điểm chính của B | Thêm việc: theo dõi tab nào chứa pane nào để focus đúng |

**Khuyến nghị: C.** B là nơi hiển thị chính (nhất quán với "operator đang ngồi trước Warp", không che
màn hình như modal); toast chỉ để không bỏ sót khi đang ở tab khác. Modal (A) giữ lại cho luồng pairing
(mục 3.10) vì đó là sự kiện hiếm, không theo session cụ thể, không xung đột với nhiều request cùng lúc.

**(b) Cách "chờ duyệt" trong code**

| Phương án | Ưu | Nhược |
|---|---|---|
| **A. oneshot channel + đua với `Timer::after` trong future đã `ctx.spawn`** (giống `visible::wait`, D29) | Nhất quán với code Bridge hiện có; timeout tự nhiên; không cần thêm state trong model để dọn khi hết giờ | — |
| B. Lưu `PendingId` + state trong model, resume bằng gọi method khi Approve (kiểu Warp Sync) | Có mẫu sẵn | Không có timeout sẵn (phải tự thêm race riêng); model phải tự quản lý dọn dẹp khi Deny/timeout, dễ rò rỉ nếu quên một nhánh |

**Khuyến nghị: A** — nhất quán với style async hiện có của Agent Bridge (mọi chờ đợi đều nằm trong
future, không phải trong state của model UI).

**(c) Vị trí file chính sách**

Giữ nguyên như roadmap: `~/.warp/agent-ops/policy.toml`, quyền `0600`. Đây là namespace dùng chung
với G1 (`hosts.toml`) — khác thư mục audit `~/.warp/agent-bridge/` của O1 (bảng 1.2). Không cần duyệt
thêm, đã đủ rõ trong roadmap.

**(d) Pairing token — ⚠ Cần duyệt (mục 3.10, có thể lùi thành Phase riêng sau)**

Roadmap tự nhận: "Không chặn được process cùng UID (đọc được token), nên giá trị nằm ở danh tính".
Ba câu hỏi thiết kế chưa có câu trả lời rõ trong roadmap:

1. **Bền qua khởi động lại Warp hay không?** Attach (D4 của Bridge) cố tình chỉ ở RAM. Pairing token
   nếu cũng chỉ-RAM thì mỗi lần mở Warp, agent phải ghép cặp lại (hộp thoại mới) — an toàn hơn nhưng
   phiền nếu dùng hằng ngày. Nếu bền (ghi ra đĩa phía Warp) thì cần một file registry mới
   (`~/.warp/agent-ops/pairings.toml`?) và một lệnh revoke độc lập với "Revoke all" của attachment.
2. **Cơ chế trao đổi bí mật.** MCP `initialize` không có trường chuẩn nào để client gửi một secret
   ngoài `clientInfo.name` (bảng 1.2). Hai hướng: (i) `warpctrl mcp` tự sinh secret, ghi file cục bộ
   `0600` (giống cách O1 không lưu gì), rồi gọi một **action local-control mới** (`agent.pair`) qua
   HTTP broker sẵn có (đã có UID-check + credential ngắn hạn) để "trình" secret cho Warp lần đầu —
   Warp hiện hộp thoại (Modal A ở trên) "Agent muốn ghép cặp, tên nó tự khai là X — Cho phép?",
   duyệt xong Warp nhớ `(secret_hash, agent_id do Warp đặt)`; (ii) không tự sinh, mà **người dùng** tạo
   token trước trong Warp (Settings hoặc palette "Agent Ops: Create pairing token") rồi dán vào cấu
   hình MCP client (biến môi trường hoặc arg `warpctrl mcp --pairing-token-file <path>`) — không cần
   hộp thoại lúc runtime, nhưng người dùng phải tự copy/paste, giống mẫu Claude Code API key.
3. **Dùng `agent_id` đã ghép cặp để làm gì trong policy?** Nếu chỉ để audit đẹp hơn (thay `agent` tự
   khai) thì giá trị thấp so với công sức. Nếu policy.toml có thể giới hạn theo `agent_id` (vd "chỉ
   agent đã ghép cặp mới được allowlist L1") thì giá trị cao hơn nhưng cần thêm cú pháp trong
   policy.toml chưa có trong ví dụ của roadmap.

**Khuyến nghị:** làm **(i)** cho câu 2 (action `agent.pair`, dùng lại hộp thoại kiểu Modal A) vì tận
dụng được broker UID-check sẵn có và không bắt người dùng copy/paste; **không bền qua restart** cho
câu 1 (nhất quán với D4, đơn giản hơn, và giá trị pairing chủ yếu là "trong phiên làm việc này, đúng
là agent X" chứ không phải kiểm soát truy cập dài hạn); để câu 3 cho **sau O2** (audit ghi `agent_id`
đã xác minh thay `agent` tự khai là đủ giá trị cho v1; mở rộng cú pháp policy theo agent là việc của
G2/O6 khi có nhiều operator/agent thật). Đây vẫn là 3 quyết định người dùng nên tự chốt trước Phase 5
— plan để **Phase 5 tách riêng, chỉ bắt đầu sau khi được hỏi lại** (mục 0.12).

---

## 2. Kiến trúc

### 2.1 Sơ đồ

```
 MCP client (Claude Code / Codex / …)
   └─ remote.exec / remote.file.write / remote.exec.visible ─► warpctrl mcp ─► broker ─► Warp
                                                                                  │
                                          handlers/remote.rs::start / exec_visible
                                                                                  │
                                                        check_access (đã có, O1)  │  Full/ReadOnly?
                                                                                  ▼
                                                        policy::evaluate(host, user, command)  [MỚI]
                                                          │             │              │
                                                        Deny          Allow           Ask
                                                          │             │              │
                                                 lỗi PolicyDenied   begin_operation   đăng ký chờ duyệt
                                                    (ngay)          + chạy như O1     (banner/toast)
                                                                                       │        │
                                                                                   Approve    Deny/
                                                                                   (≤5 phút)  timeout
                                                                                       │        │
                                                                                 begin_operation  lỗi
                                                                                 + chạy như O1  PolicyDenied
```

### 2.2 Luồng một lệnh `remote.exec` khi chính sách là "hỏi"

1. `handlers/remote.rs::start` parse `Operation`, `check_access` như O1 (không đổi).
2. **[MỚI]** `authorize()`: đọc policy đã nạp (cache trong `AgentBridgeModel` hoặc model riêng, xem
   3.2), gọi `policy::evaluate(host, user, &command_text, mode)` — hàm **thuần**, không I/O.
3. Kết quả `Ask(description)`: đăng ký một `PendingApproval` (mục 3.4) gắn với `SessionId` +
   `request_id`, trả `receiver` chờ ở **tầng ctx.spawn đầu tiên** (không gọi `begin_operation` lúc
   này — mục 1.2 giải thích lý do). Banner ở pane header đổi sang "Agent muốn chạy: `<command>` —
   Approve / Deny"; nếu pane không active, toast xuất hiện.
4. Người dùng bấm Approve/Deny (dispatch một `TerminalAction` mới) **hoặc** hết 5 phút
   (`Timer::after(APPROVAL_TIMEOUT)` đua với oneshot receiver, giống `visible::wait`).
5. Callback (main thread): Approve → `begin_operation(Hidden)` → `ctx.spawn(operation.run(...))` như
   O1 từ đây. Deny/timeout → trả lỗi `PolicyDenied`, ghi audit `result: "error", error_code:
   "policy_denied"`.
6. Audit ghi thêm quyết định chính sách (`policy_decision: "allow" | "ask_approved" | "ask_denied" |
   "ask_timeout" | "deny"`), xem 3.9.

### 2.3 Định dạng `policy.toml`

Giữ đúng ví dụ của roadmap, làm rõ luật ưu tiên (roadmap không nói thứ tự, đây là điểm đã tự quyết
theo nguyên tắc an toàn "deny thắng"; ghi vào mục 8 khi thực hiện):

```toml
[defaults]
mode = "approve"                 # "read_only" | "approve" | "allowlist"

[[hosts]]
match = "lab-*"                  # glob trên hostname (dùng crate glob đã có? — kiểm ở Task 1.1)
mode = "allowlist"
allow = ["systemctl restart php-fpm", "systemctl reload nginx"]   # khớp NGUYÊN VĂN, sau khi trim

[deny]
patterns = ['\brm\s+-rf\s+/', '\bmkfs', '\bdd\s+if=', '\b(shutdown|reboot|halt)\b',
            'iptables\s+-F', 'nft\s+flush', '(?i)drop\s+(database|table)']
```

**Luật (`policy::evaluate`, thứ tự cố định):**

1. Lệnh khớp bất kỳ pattern nào trong `[deny]` (regex, case theo pattern) → **Deny**, luôn luôn,
   **bất kể mode của host**. Đây là L3 — không có ngoại lệ (kể cả `allowlist` khớp allow list).
2. Không có host nào khớp `match` (glob so với hostname của session) → dùng `defaults.mode`.
3. Có host khớp (nếu nhiều host khớp, dùng **host đầu tiên khớp**, thứ tự trong file — ghi rõ trong
   lỗi parse nếu người dùng cần biết) → dùng `mode` của host đó.
4. Theo `mode` đã chọn ở bước 2/3:
   - `read_only`: mọi hành động ghi → **Deny** ngay (không hỏi — đúng nghĩa "chỉ đọc").
   - `approve`: mọi hành động ghi → **Ask**.
   - `allowlist`: lệnh khớp nguyên văn (sau `trim()`) một entry của `allow` → **Allow**; không khớp
     → **Ask** (roadmap: "Mọi lệnh ghi khác" ở L2, không phải tự-Deny khi không khớp allowlist).
5. `remote.file.write` không có "lệnh" — dùng gì để so với `allow`/hiển thị trong hộp thoại? **⚠ Cần
   duyệt nhỏ**: đề xuất coi allowlist chỉ áp dụng cho `remote.exec`/`remote.exec.visible`; mọi
   `remote.file.write` trong mode `allowlist`/`approve` đều **Ask** (không có khái niệm "ghi file đã
   duyệt trước" ở v1 — diff thật để review nằm ở G4/Warp Sync). Trong `read_only` vẫn Deny.

### 2.4 Mô hình an toàn (bổ sung cho bảng 2.3 của plan Bridge)

| Lớp | Cơ chế |
|---|---|
| Có sẵn (O1) | Attach theo session, TTL 30 phút, `Full`/`ReadOnly`, Revoke all, audit fail-closed |
| **Flag mới** | `AgentOpsPolicy` (Task 0.1). Tắt flag → hành vi y hệt O1 hôm nay (mọi ghi tự chạy nếu đã `Full`) — không phá vỡ người đang dùng O1. |
| **File chính sách** | `~/.warp/agent-ops/policy.toml`, quyền `0600`, tạo thư mục qua `create_private_dir_all`. Thiếu file → mặc định **`approve`** (an toàn nhất, không phải `read_only` vì sẽ khoá luôn write ngoài ý muốn, không phải `allowlist` vì rỗng nguy hiểm nếu người dùng gõ nhầm mode). File lỗi cú pháp → **fail-closed**: coi như `read_only` toàn bộ + toast lỗi rõ ràng (không dùng `approve` khi không đọc được policy, vì `approve` vẫn cho léo qua nếu người dùng bấm nhầm Approve hàng loạt trong lúc không biết file hỏng). |
| **Đúng một chỗ gọi** | `authorize()` trong `handlers/remote.rs`, gọi từ `start` (khi `Operation::Exec`/`Write`) và `exec_visible` — không nơi nào khác được gọi `policy::evaluate` trực tiếp (test bằng cách chỉ `pub(super)` hàm này trong module `remote`). |
| **Duyệt = Ask** | Đăng ký trong `ApprovalQueue` (RAM, không bền qua restart — nhất quán với Attachments D4), timeout 5 phút → Deny, không có "nhớ lựa chọn" ngoại trừ nút tường minh "Approve lệnh này cho session này" (roadmap) — xem 3.6. |
| **Kill switch** | "Revoke all" (đã có) + hành động mới "Deny tất cả yêu cầu đang chờ" (mục 3.7) — không tự động gộp vào Revoke all vì ngữ nghĩa khác (Revoke huỷ quyền tương lai, Deny-all-pending chỉ xử lý các yêu cầu đang treo). |
| **Truy vết** | Audit ghi `policy_decision`; không ghi nội dung `policy.toml` (có thể chứa hostname nội bộ — coi như dữ liệu nhạy vừa phải, không phải bí mật, nên không redact, nhưng không log nguyên file ra đâu khác). |

### 2.5 Ngoài phạm vi O2

- G2 (agent tự mở session), G3 (transport ssh trực tiếp), G4 (sửa file qua mirror Warp Sync + diff
  trong hộp thoại) — vẫn đứng sau O2 theo AO7/D27.
- Sửa policy qua Settings UI — v1 chỉ sửa file tay (giống cách roadmap mô tả); Settings UI đọc để
  hiện trạng thái là **có thể làm nếu rẻ** (mục 4, Phase 4) nhưng không bắt buộc.
- Nhiều operator/Hub, chia allowlist theo operator — O6.
- Cú pháp policy theo `agent_id` đã pairing — sau O2 (mục 1.3(d) câu 3).
- L1 tự động cho **prod** — roadmap: "prod chỉ sau gate O4"; O2 chỉ chạy L1 trên host lab theo
  `match` glob mà người dùng tự cấu hình (không có khái niệm "prod" trong code, chỉ trong cách người
  dùng đặt `match`).

---

## 3. Thiết kế chi tiết

### 3.1 Feature flag mới

`FeatureFlag::AgentOpsPolicy` trong `crates/warp_features/src/lib.rs` (cạnh `AgentBridge`), cargo
feature `agent_ops_policy` trong `app/Cargo.toml` + ánh xạ trong `app/src/features.rs` (theo đúng mẫu
`AgentBridge`, skill `add-feature-flag`). **Không** thêm vào `DOGFOOD_FLAGS` cho tới CHECKPOINT cuối
(giữ tắt mặc định trong lúc phát triển, giống cách `AgentBridge` được bật thủ công qua feature trong
suốt O1). Khi tắt: `authorize()` luôn trả `Allow` ngay (hành vi giống hệt O1 hôm nay) — **không** trả
lỗi `UnsupportedAction` (khác với `AgentBridge` tắt) vì đây là một tính năng bổ sung, không phải một
action mới.

### 3.2 `app/src/agent_bridge/policy.rs` (thuần, test dày)

```rust
pub(crate) enum Mode { ReadOnly, Approve, Allowlist }

pub(crate) struct HostRule { pub match_glob: String, pub mode: Mode, pub allow: Vec<String> }

pub(crate) struct Policy {
    default_mode: Mode,
    hosts: Vec<HostRule>,
    deny_patterns: Vec<Regex>,          // biên dịch một lần lúc nạp file
}

pub(crate) enum Decision {
    Allow,
    Ask { reason: AskReason },          // vd "no allowlist match", "mode is approve"
    Deny { reason: String },            // vd "matches deny pattern '...'"
}

pub(crate) enum Operation<'a> { Exec(&'a str), Write, ExecVisible(&'a str) }  // xem 2.3.5

impl Policy {
    pub(crate) fn load(path: &Path) -> Result<Self, PolicyError>;   // parse toml, biên dịch regex
    pub(crate) fn evaluate(&self, hostname: &str, operation: Operation<'_>) -> Decision;  // thuần
}
```

Test: mọi tổ hợp mode × operation × khớp/không khớp allow × khớp/không khớp deny; glob nhiều host
khớp (host đầu tiên thắng); regex compile lỗi ở `load` → `PolicyError` rõ pattern nào hỏng; file rỗng
→ `default_mode = Approve`, không host, không deny (roadmap không nói rõ default cho `defaults` thiếu
— coi `mode` bắt buộc trong `[defaults]`, thiếu thì lỗi parse, không tự đoán). Crate glob: kiểm
`Cargo.toml` gốc có sẵn `glob`/`globset` chưa — nếu không, đề xuất so khớp glob đơn giản tự viết
(chỉ cần `*` ở đầu/cuối như ví dụ `lab-*`, không cần glob đầy đủ) thay vì thêm dependency; **⚠ nếu
compiler/khảo sát cho thấy cần glob phức tạp hơn, dừng hỏi trước khi thêm crate mới**.

### 3.3 `ErrorCode` mới (`crates/local_control/src/protocol.rs`)

Thêm `PolicyDenied` (không dùng lại `InsufficientPermissions` vì đó là do **attach** không đủ quyền,
khác nguyên nhân với "bị chính sách/host từ chối" — agent cần phân biệt để biết có nên hỏi lại người
dùng attach lại hay không). Sửa mọi match exhaustive theo compiler báo (như D1 mục 3.1 của Bridge).
Message ví dụ: `"Denied by policy: matches a deny pattern (rm -rf /). This command is never allowed."`
hoặc `"Denied by policy: no one approved it within 5 minutes."`.

### 3.4 `app/src/agent_bridge/approval.rs` (mới)

```rust
pub(crate) struct PendingApproval {
    pub request_id: Uuid,
    pub session: SessionId,
    pub host: String,
    pub user: String,
    pub agent: Option<String>,
    pub description: ApprovalDescription,   // Command(String) | FileWrite{path, bytes}
    sender: Option<oneshot::Sender<ApprovalDecision>>,
}

pub(crate) enum ApprovalDecision { Approve, Deny }

#[derive(Default)]
pub(crate) struct ApprovalQueue { by_id: HashMap<Uuid, PendingApproval> }  // sống trong AgentBridgeModel

impl ApprovalQueue {
    pub(crate) fn request(&mut self, ...) -> oneshot::Receiver<ApprovalDecision>;
    pub(crate) fn decide(&mut self, request_id: Uuid, decision: ApprovalDecision) -> bool; // false nếu đã hết hạn/không tồn tại
    pub(crate) fn deny_all_for_session(&mut self, id: SessionId) -> usize;   // kill switch mục 3.7
    pub(crate) fn pending_for_session(&self, id: SessionId) -> Option<&PendingApproval>;  // banner mục 3.6
}
```

Đặt trong `AgentBridgeModel` (cạnh `Attachments`/`Operations`, đã là `SingletonEntity` sẵn — không
tạo model mới) hay tách model riêng? **⚠ Cần duyệt nhỏ, khuyến nghị: cùng `AgentBridgeModel`** — đơn
giản hơn, và banner/toast của mục 3.6 vốn đã đọc `AgentBridgeModel` cho trạng thái attach.

Chờ duyệt trong future đã `ctx.spawn` (mục 2.2 bước 3–4):

```rust
let (rx, request_id) = AgentBridgeModel::handle(ctx).update(ctx, |m, ctx| {
    let rx = m.approval_queue.request(...);
    ctx.notify();   // banner/toast vẽ lại
    rx
});
ctx.spawn(async move {
    futures::select! {
        decision = rx.fuse() => decision.unwrap_or(ApprovalDecision::Deny),  // kênh đóng (Warp tắt?) → Deny
        _ = Timer::after(APPROVAL_TIMEOUT).fuse() => ApprovalDecision::Deny,
    }
}, move |_, decision, ctx| { /* dọn queue nếu timeout thắng đua; rồi begin_operation nếu Approve */ });
```

Test: `approval` module (thuần, dùng oneshot + executor test như `visible_tests.rs` đã làm cho
`Timer`), `AgentBridgeModel` (request/decide/timeout/deny_all_for_session), và test tích hợp qua
handler HTTP (giả lập Approve tới trước timeout, Deny, không ai trả lời).

### 3.5 Wiring trong `handlers/remote.rs`

- `start()`: tách logic hiện tại của Task "một `ctx.spawn`" thành theo mẫu `exec_visible` (mục 1.2):
  1. `check_access` (không đổi).
  2. `authorize()` — gọi `policy::evaluate`. `Deny` → trả lỗi `PolicyDenied` **ngay, đồng bộ**, không
     tạo receiver nào (không có gì để chờ).
  3. `Allow` → y hệt code O1 hôm nay (`begin_operation(Hidden)` rồi `ctx.spawn(operation.run(...))`).
  4. `Ask` → KHÔNG `begin_operation` ngay. `ctx.spawn` tầng 1 chờ quyết định (mục 3.4); callback: Deny
     → trả lỗi, ghi audit; Approve → `begin_operation(Hidden)` rồi `ctx.spawn` tầng 2 chạy
     `operation.run(...)` (y hệt code Allow ở bước 3, tránh trùng lặp bằng một hàm dùng chung
     `run_operation(...)`).
- `exec_visible()`: chèn bước `authorize()` ngay sau `check_access`, **trước** `begin_operation(Visible)`
  hiện có ở đầu hàm (dòng `remote.rs:198-201` theo bản O1) — thứ tự còn lại giữ nguyên.
- `output_recent()`: **không đổi** — L0, không qua policy.
- Audit `started` (fail-closed, D20 của Bridge) ghi **trước khi chạy lệnh**, nghĩa là với `Ask`, thời
  điểm ghi "started" phải là **sau khi Approve**, không phải lúc đăng ký chờ duyệt (nếu ghi sớm hơn,
  một request bị Deny/timeout sẽ để lại bản ghi "started" không bao giờ "chạy" — gây hiểu nhầm khi đọc
  audit). Thêm bản ghi audit riêng cho chính sự kiện chờ duyệt/bị từ chối (mục 3.9), tách khỏi
  "started"/"ok"/"error" của O1.

### 3.6 UI — banner pane header + toast (phương án C của 1.3a)

- Mở rộng `pane_impl.rs`: `agent_bridge_access()` (dòng 1007) đọc thêm
  `AgentBridgeModel::as_ref(app).approval_queue.pending_for_session(id)`. Khi có pending: banner thay
  thế nội dung của `render_agent_bridge_indicator` bằng "Agent wants to run: `<command đã cắt ngắn +
  redact secret>`" (dùng `secret_redaction`/`mcp::redact`-kiểu che secret **trước khi hiện**, không
  chỉ trước khi gửi model — người dùng cũng không nên thấy secret thô trong UI nếu tránh được **⚠ Cần
  duyệt nhỏ**: liệu redact luôn, hay chỉ theo cùng setting redact của O1? Khuyến nghị: theo cùng
  setting, để nhất quán với những gì agent thấy — nếu user tắt redact cho agent, họ cũng đã chấp nhận
  thấy secret) + hai nút Approve/Deny (theme theo `gui-ui-guidelines`: dùng `PrimaryTheme`/
  `DangerPrimaryTheme` có sẵn, **không** tự chế theme mới).
- `TerminalAction` mới: `ApproveAgentRequest`, `DenyAgentRequest` (giống mẫu
  `RevokeAgentBridgeAccess`), dispatch qua `PaneHeaderAction::CustomAction`.
- Toast: dùng lại cơ chế toast của `workspace/view.rs` (đã dùng cho O1 Task 2.6) khi có pending mới và
  pane chứa nó **không phải** pane đang active của tab hiện tại; toast có nút "Xem" → focus đúng
  tab/pane (cần tra `pane_group`/`window_index` như `metadata::session_entries` đã làm — dùng lại,
  không viết lại logic tìm pane).
- Nút "Approve lệnh này cho session này" (roadmap) — phạm vi hẹp hơn allowlist toàn host: chỉ nhớ
  **trong RAM, cho đúng session đó, tới khi detach/hết hạn attach** (không ghi ra `policy.toml`).
  Lưu trong `AgentBridgeModel` như một tập `HashSet<(SessionId, String /* lệnh đã trim */)>`, kiểm
  trước khi vào `authorize()` bước Ask — khớp thì tự Allow không cần hỏi lại. **⚠ Cần duyệt**: có làm
  nút này ở v1 hay để `Ask` luôn hỏi từng lần (đơn giản hơn, an toàn hơn, nhưng phiền hơn khi lặp lại
  một lệnh chẩn đoán nhiều lần)? Khuyến nghị: làm, vì roadmap liệt kê nó tường minh trong danh sách nút
  của hộp thoại.

### 3.7 Kill switch

Thêm hành động `TerminalAction::DenyAllPendingAgentRequests` (nút mới cạnh Revoke, hoặc gộp vào overflow
menu như Revoke — theo `pane_impl.rs:718-727`) gọi `approval_queue.deny_all_for_session`. `revoke_agent_bridge_access`
(dòng 1083) gọi thêm `deny_all_for_session` (Revoke ý là "ngưng mọi quyền" — pending Ask cũng nên bị
Deny theo, không để nó tự Allow sau khi đã Revoke).

### 3.8 CLI / MCP

Không có tool MCP mới cho Approve/Deny — roadmap: "duyệt ở phía Warp", agent chỉ nhận kết quả
(Allow/Deny) hoặc chờ (agent thấy tool call chưa trả lời, tự nhiên là "đang chờ người duyệt" từ phía
agent — không cần thông báo gì đặc biệt qua MCP, timeout của MCP client đã có sẵn timeout dài hơn 5
phút do request `remote.exec`/`.visible` vốn cấu hình `timeout_secs + 30s`, D... của Bridge — **kiểm
lại**: nếu người dùng đặt `timeout_secs` mặc định 120s nhưng duyệt tốn 4 phút, HTTP client (30s cộng
thêm) có thể timeout **trước** khi người dùng kịp bấm. **⚠ Cần duyệt**: thời gian chờ duyệt (5 phút)
phải nằm **trong** `timeout_secs` của request, hay là một hạn mức **riêng, cộng thêm**? Khuyến nghị:
riêng, cộng thêm — thời gian "chờ người" không nên tính vào ngân sách thời gian "chạy lệnh" mà agent
khai báo; nghĩa là HTTP client (CLI `warpctrl`, MCP transport) phải dùng timeout =
`APPROVAL_TIMEOUT + timeout_secs + 30s` khi biết trước hành động có thể bị Ask — nhưng client **không
biết trước** liệu có bị hỏi hay không. Giải pháp: nới **timeout HTTP mặc định của mọi `remote.exec`/
`write`/`exec.visible`** thêm `APPROVAL_TIMEOUT` (5 phút) vô điều kiện, vì các request đó vốn đã có
`timeout_secs` (tối đa 600s theo O1) cộng thêm 30s — nới thêm 300s là chấp nhận được và đơn giản hơn
"biết trước có bị hỏi không". Việc này sửa ở **client** (`warpctrl remote`, `mcp/tools.rs`), không
phải ở app.
Tuỳ chọn thêm `warpctrl remote policy show` (đọc + hiện policy đang nạp, chỉ để debug) — không bắt
buộc theo gate roadmap, để cuối Phase 4 nếu còn thời gian, không phải một Task riêng.

### 3.9 Audit (`app/src/agent_bridge/audit.rs`)

Thêm trường `policy_decision: Option<&'static str>` (`"allow" | "ask_approved" | "ask_denied" |
"ask_timeout" | "deny"`) vào `AuditRecord`; với `deny`/`ask_denied`/`ask_timeout`, ghi thêm
`policy_reason: Option<String>` (lý do từ `Decision::Deny{reason}`/`AskReason`, không phải nội dung
lệnh — lệnh đã có trường `command` sẵn). Một bản ghi audit riêng cho lúc bắt đầu chờ duyệt (`result:
Started`-kiểu nhưng có thể không bao giờ có bản ghi kết thúc tương ứng nếu Warp tắt giữa chừng — chấp
nhận được, giống rủi ro (a) đã ghi ở D21 của Bridge).

### 3.10 Phase 5 (tuỳ chọn) — Pairing token

Chi tiết thiết kế nằm ở mục 1.3(d) (khuyến nghị: action `agent.pair` mới, hộp thoại kiểu Modal A,
không bền qua restart). **Không viết thêm task cụ thể ở đây** — sau khi Phase 0–4 xong và được duyệt,
quay lại hỏi người dùng chốt 3 câu ở 1.3(d), rồi mới viết task chi tiết (giống cách Phase 5 của plan
Bridge được viết **sau** Checkpoint C, không viết trước).

---

## 4. Các Phase

**Thứ tự:** 0 → 1 → 2 → 3 → 4 → CHECKPOINT chính → (hỏi lại) → 5 (pairing, tuỳ chọn) → CHECKPOINT phụ.

### Phase 0 — Chuẩn bị

- 0.1 `FeatureFlag::AgentOpsPolicy` (mục 3.1). Test: flag mặc định tắt, bật qua cargo feature.
- 0.2 Xác nhận `git status` sạch, `cargo check -p warp -p local_control -p warp_cli` pass trước khi
  sửa gì (đảm bảo bắt đầu từ trạng thái O1 hoàn tất, không có gì dở dang).

### Phase 1 — Chính sách (thuần, không UI, không async)

- 1.1 `agent_bridge/policy.rs`: `Mode`, `HostRule`, `Policy::load`, `Decision`, `Operation`,
  `Policy::evaluate` (mục 3.2). Quyết định cách so khớp glob (tự viết `*` đầu/cuối hay thêm crate —
  dừng hỏi nếu cần hơn thế). ~25-30 test theo bảng tổ hợp ở 3.2.
- 1.2 `ErrorCode::PolicyDenied` + `AgentBridgeError::PolicyDenied` + `From` (mục 3.3). Sửa match
  exhaustive theo compiler báo (`warp_cli` output/format, MCP `format.rs`, …).
- 1.3 Nạp `policy.toml` lúc cần (không cache vĩnh viễn — đọc lại mỗi request là đơn giản và đủ nhanh
  cho tần suất dùng thực tế; **⚠ nếu review sau thấy cần cache, bàn ở đó, không tối ưu sớm**), quyền
  `0600`, thư mục `~/.warp/agent-ops/` qua `create_private_dir_all`. Test: file thiếu → `Approve`
  default; file lỗi cú pháp → fail-closed `read_only` + thông báo rõ dòng/lý do lỗi.
- Cuối Phase: `cargo test -p warp --lib agent_bridge`, clippy 3 package.

### Phase 2 — Chờ duyệt (async, chưa có UI — dùng test resolve thủ công)

- 2.1 `agent_bridge/approval.rs`: `PendingApproval`, `ApprovalDecision`, `ApprovalQueue` (mục 3.4).
  Test thuần: request/decide/timeout race/deny_all_for_session/pending_for_session, id không tồn tại.
- 2.2 Gắn `ApprovalQueue` vào `AgentBridgeModel` (quyết định 3.4, khuyến nghị dùng chung model).
- 2.3 `authorize()` trong `handlers/remote.rs`, wiring `start`/`exec_visible` theo mục 3.5 (tách
  `start` thành 2 tầng `ctx.spawn`, hàm dùng chung `run_operation`). Audit `started` dời tới sau
  Approve (mục 3.5, đoạn cuối). Test handler qua HTTP: Allow chạy ngay (không đổi hành vi O1 khi flag
  tắt hoặc mode=allowlist khớp), Deny trả lỗi ngay không tạo `Pending`, Ask + resolve thủ công (gọi
  thẳng `ApprovalQueue::decide` trong test) → Allow tiếp tục chạy / Deny trả lỗi, Ask + hết
  `APPROVAL_TIMEOUT` (rút ngắn hằng số qua `#[cfg(test)]` hoặc tham số — theo mẫu `visible_tests.rs`
  đã rút ngắn `VISIBLE_POLL_INTERVAL`) → tự Deny.
- 2.4 Nới timeout HTTP client thêm `APPROVAL_TIMEOUT` cho `remote.exec`/`.write`/`.visible` (mục 3.8,
  cả `warpctrl remote` và `mcp/tools.rs`). Test: timeout tính đúng.
- Cuối Phase: test `agent_bridge`/`local_control` + clippy 3 package.

### Phase 3 — Kill switch + audit

- 3.1 `deny_all_for_session` gọi từ `revoke_agent_bridge_access` (mục 3.7); hành động mới
  `DenyAllPendingAgentRequests` nếu tách riêng khỏi Revoke (quyết định lúc code: có thể gộp im lặng
  vào Revoke, ghi vào mục 8 nếu làm khác plan).
- 3.2 Audit: `policy_decision`, `policy_reason` (mục 3.9). Test: mỗi nhánh Allow/Ask-approved/
  Ask-denied/Ask-timeout/Deny ghi đúng dòng.
- Cuối Phase: test + clippy.

### Phase 4 — UI (banner + toast + nút)

- 4.1 Đọc kỹ `gui-ui-guidelines` trước khi bắt đầu (mục 0.1).
- 4.2 Mở rộng `pane_impl.rs`: banner khi có pending (mục 3.6), nút Approve/Deny, `TerminalAction`
  mới, dispatch, handler trong view. Chữ trung lập với agent (D11 của Bridge — không nêu tên agent cụ
  thể, chỉ "Agent"/"Agents" như nhãn hiện có).
- 4.3 Nút "Approve lệnh này cho session này" (mục 3.6, nếu quyết định làm ở v1).
- 4.4 Toast khi pending mới và pane không active (mục 3.6); focus đúng tab/pane khi bấm.
- 4.5 (tuỳ chọn, làm nếu rẻ) `warpctrl remote policy show` để debug (mục 3.8).
- 4.6 Tự review (rust-reviewer + security-reviewer): đặc biệt kiểm race giữa Approve/Deny/timeout (ai
  thắng khi cả 3 xảy ra gần nhau), kiểm `deny_all_for_session` không bỏ sót request đang ở giữa hai
  tầng `ctx.spawn`, kiểm banner không lộ secret khi setting redact bật.
- 4.7 `./script/format` **một lần**, cuối cùng của Phase 0–4. Commit.

**⛔ CHECKPOINT chính** — người dùng bật `AgentOpsPolicy`, thử: mode `read_only` chặn mọi ghi; mode
`approve` hiện banner + toast, Approve chạy được (kể cả visible — chạy thành block thật), Deny/hết 5
phút trả lỗi cho agent; mode `allowlist` với 1 entry đúng tự chạy, lệch một ký tự thì Ask; deny pattern
(thử một lệnh vô hại đại diện, KHÔNG thử `rm -rf /` thật) luôn chặn dù đang `allowlist`; Revoke dọn
sạch cả pending đang chờ.

**Sau CHECKPOINT chính:** dừng, hỏi người dùng có làm Phase 5 (pairing token) ngay không, hay để đó
cho một phiên khác / gộp vào lúc bắt đầu G2.

### Phase 5 — Pairing token (tuỳ chọn, viết task chi tiết SAU khi được hỏi lại — mục 3.10)

---

## 5. Checklist test tay (cho người dùng)

Môi trường: kế thừa từ checklist 5.A/5.B/5.C của plan Bridge (VM có sshd, hostname khác máy local,
sudo cần mật khẩu). `W=/projects/github/warp-agent-bridge/target/debug/warp-oss`.

**D. Chính sách cơ bản (Checkpoint chính)**

1. `~/.warp/agent-ops/policy.toml` chưa tồn tại → gọi `remote.exec` qua Claude Code → kỳ vọng: **Ask**
   (default an toàn), banner hiện đúng lệnh.
2. Tạo file với `[defaults] mode = "read_only"` → mọi `remote.exec`/`remote.file.write`/
   `remote.exec.visible` → lỗi `PolicyDenied` ngay, không có banner (không phải Ask).
3. Đổi `mode = "approve"`, gọi `remote.exec -- 'echo hi'` → banner "Agent wants to run: echo hi" +
   nút Approve/Deny; bấm **Deny** → agent nhận lỗi `PolicyDenied` với lý do "denied by the user".
4. Lặp lại, bấm **Approve** → lệnh chạy, kết quả trả về agent bình thường như O1.
5. Lặp lại, **không bấm gì** > 5 phút → tự Deny, banner tự ẩn, audit ghi `ask_timeout`.
6. `[[hosts]] match = "<hostname thật của VM>" mode = "allowlist" allow = ["echo hi"]` → `remote.exec
   -- 'echo hi'` chạy thẳng không hỏi; `remote.exec -- 'echo hi '` (thêm khoảng trắng cuối, agent tự
   gửi) vẫn chạy thẳng (đã `trim()`); `remote.exec -- 'echo hi2'` → Ask.
7. `[deny] patterns` thêm một pattern vô hại để test (vd `'\bwhoami\b'`) → `remote.exec -- whoami`
   luôn Deny ngay cả khi đang ở host `allowlist` — xác nhận L3 thắng L1.
8. `remote.exec.visible -- 'echo hi'` ở mode `approve`, Approve → block thật hiện trong pane (như
   Checklist 5.C của Bridge), không phải chạy ẩn.
9. Trong lúc một request đang "Ask", dùng Attach → **Revoke** trên pane đó → banner biến mất, request
   đang chờ nhận lỗi `PolicyDenied` (không phải treo tới khi timeout).
10. Sửa file thành cú pháp sai (xoá dấu `"`) → request tiếp theo → Deny với thông báo lỗi cú pháp rõ
    ràng (fail-closed, không phải im lặng coi như `approve`).
11. Toast: mở 2 tab, mỗi tab một session SSH khác nhau đã attach; gửi `remote.exec` vào session của
    tab **không active** → toast xuất hiện ở tab đang active, bấm "Xem" nhảy đúng tab/pane.
12. Nút "Approve lệnh này cho session này" (nếu làm ở 4.3): Approve theo cách này cho `ls`, gọi lại
    `remote.exec -- ls` lần hai trong cùng session → không hỏi lại, tự Allow; detach rồi attach lại →
    phải hỏi lại (không nhớ qua lượt attach mới).

---

## 6. Rủi ro đã biết

| Rủi ro | Giảm thiểu |
|---|---|
| `policy.toml` đọc/ghi được bởi bất kỳ process nào cùng UID | Không mạnh hơn mô hình tin cậy đã chấp nhận ở O1 (D21 (f) tương tự) — quyền `0600` chỉ chặn user khác, không chặn process cùng UID; đây là giới hạn đã biết, không phải lỗi thiết kế |
| Regex denylist bị lách bằng shell obfuscation (`eval $(echo ... | base64 -d)`) | Roadmap đã ghi nhận: denylist chỉ là "gờ giảm tốc"; ranh giới thật là read-only/hộp thoại/allowlist khớp nguyên văn, không phải denylist |
| Một dialog/banner cho nhiều session cùng lúc → dễ bỏ sót | Banner theo session (không phải modal toàn cục) + toast; vẫn có thể bỏ sót nếu nhiều toast dồn dập — chấp nhận ở v1, cải thiện (danh sách "pending approvals" tổng hợp) để dành cho O5 nếu cần |
| Người dùng bấm Approve theo phản xạ mà không đọc kỹ | Hiện đầy đủ lệnh/host/user mỗi lần, không có "auto-approve trong N phút tới"; chỉ có "approve lệnh **này**, session **này**" theo đúng roadmap, không mở rộng hơn |
| Nới timeout HTTP thêm 5 phút cho mọi request ghi (mục 3.8) làm agent chờ lâu hơn cả khi không có gì để duyệt | Chỉ ảnh hưởng khi request thật sự bị Ask; Allow/Deny trả lời gần như ngay (không đổi độ trễ đo được ở Checkpoint A của Bridge, 0,183 s) |
| Pairing token (Phase 5) không chặn được process cùng UID | Đã ghi rõ trong roadmap và mục 1.3(d); giá trị chỉ là danh tính, không phải cách ly — nhắc lại trong hộp thoại ghép cặp để người dùng không hiểu lầm là một lớp bảo mật mạnh |
| File chính sách lỗi cú pháp giữa lúc đang dùng (sửa tay, gõ nhầm) | Fail-closed (`read_only`) thay vì fail-open — thà chặn nhầm còn hơn cho chạy nhầm |

---

## 7. Hướng dẫn dùng hằng ngày (viết đầy đủ sau khi xong Phase 4)

Phác thảo — hoàn thiện thành hướng dẫn thật ở Task 4.7 cùng lúc `./script/format`:

1. Bật `AgentOpsPolicy` (feature + Settings nếu có công tắc).
2. Tạo `~/.warp/agent-ops/policy.toml` (quyền `0600`), bắt đầu với `mode = "approve"`.
3. Thêm `[[hosts]]` cho host lab quen thuộc khi đã tin tưởng một số lệnh, chuyển `mode = "allowlist"`.
4. Không tự ý thêm allowlist cho lệnh có thể phá huỷ — dùng `[deny]` cho nhóm lệnh không bao giờ muốn
   chạy dù ai duyệt.
5. Duyệt trong Warp: banner trên pane header của session đó; Deny/không trả lời > 5 phút = an toàn
   mặc định.

---

## 8. Tiến độ, quyết định, nhật ký

### Tiến độ

- [ ] Phase 0 — flag + xác nhận trạng thái sạch
- [ ] 1.1 `policy.rs` · [ ] 1.2 `ErrorCode`/`AgentBridgeError` · [ ] 1.3 nạp file
- [ ] 2.1 `approval.rs` · [ ] 2.2 gắn vào model · [ ] 2.3 wiring `authorize()` · [ ] 2.4 nới timeout client
- [ ] 3.1 kill switch · [ ] 3.2 audit
- [ ] 4.1 đọc skill · [ ] 4.2 banner + nút · [ ] 4.3 "approve cho session này" · [ ] 4.4 toast · [ ] 4.5 CLI debug (tuỳ chọn) · [ ] 4.6 review · [ ] 4.7 format
- [ ] ⛔ CHECKPOINT chính
- [ ] Hỏi lại người dùng về Phase 5 (pairing)
- [ ] Phase 5 (nếu làm)

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| DP1 | 2026-09-27 | Chỉ 2 hàm gọi `policy::evaluate` (`start` cho Exec/Write, `exec_visible`), không phải nghĩa đen "một hàm" — `output_recent`/`Operation::Read` không qua policy vì là L0 | `check_access` vốn đã dùng chung cho 3 handler nhưng chỉ 2 trong số đó làm việc ghi; roadmap tự mô tả policy áp cho "hành động" (write), không phải đọc |
| DP2 | 2026-09-27 | `begin_operation` (Hidden/Visible) chỉ gọi **sau** khi có Allow, không gọi lúc đăng ký "Ask" | Nếu gọi trước, một yêu cầu đang chờ người (tới 5 phút) sẽ chiếm slot và khoá các yêu cầu ẩn khác của cùng session trong lúc chờ |
| DP3 | 2026-09-27 | Audit "started" dời tới sau khi Approve; thêm bản ghi audit riêng cho sự kiện chờ duyệt | Ghi "started" lúc đăng ký Ask sẽ để lại bản ghi mồ côi nếu Deny/timeout, gây hiểu nhầm khi đọc log |
| DP4 | 2026-09-27 | Hộp thoại duyệt hiện theo banner trên pane header (mở rộng D26) + toast khi pane không active, không dùng modal toàn cục kiểu `WarpSyncConfirmDialog` | Modal của Workspace chỉ hiện được 1 cái tại một thời điểm — 2 session cùng Ask sẽ đá nhau; banner theo session tránh xung đột và giữ ngữ cảnh pane |
| DP5 | 2026-09-27 | Timeout HTTP của mọi `remote.exec`/`.write`/`.visible` nới thêm `APPROVAL_TIMEOUT` (5 phút) vô điều kiện, thay vì chỉ khi biết trước sẽ bị Ask | Client không biết trước một request có bị Ask hay không; nới vô điều kiện đơn giản hơn và không ảnh hưởng độ trễ khi Allow/Deny trả lời ngay |
| DP6 | 2026-09-27 | Pairing token tách thành Phase 5, chỉ viết task chi tiết sau khi hỏi lại người dùng 3 câu ở mục 1.3(d) | Roadmap mô tả pairing ở mức mục tiêu, không phải API; 3 câu hỏi (bền qua restart? cơ chế trao đổi bí mật? dùng để làm gì trong policy?) cần người dùng chốt trước khi viết code, đúng quy tắc "API không chắc thì dừng hỏi" |

*(Các dòng trên là quyết định đề xuất trong lúc viết plan — cần người dùng duyệt trước khi Phase 0
bắt đầu code. Nhật ký thực thi sẽ nối tiếp bên dưới khi bắt đầu Phase 0.)*

### Nhật ký

- 2026-09-27 — Plan v1 viết sau khi khảo sát code thật hậu-O1: `handlers/remote.rs` (điểm gọi
  `check_access`, cấu trúc `ctx.spawn` của `start`/`exec_visible`), `bridge.rs`/`mod.rs`
  (`BridgeResult::Pending`), `operations.rs`, `error.rs`, `crates/local_control/src/protocol.rs`
  (`ErrorCode`), `crates/local_control/src/auth.rs` (credential theo action, không phải danh tính
  bền), `warp_sync/confirm_dialog.rs` + chỗ gắn vào `workspace/view.rs` (mẫu dialog singleton, không
  timeout), `pane_impl.rs` (indicator D26), `mcp/jsonrpc.rs` (`set_client_name`, không có trường
  pairing chuẩn), `app/Cargo.toml` (đã có `toml`/`regex`, không cần dependency mới). Chưa bắt đầu
  Phase 0. Chờ người dùng duyệt các mục **⚠ Cần duyệt** ở mục 1.3 và các quyết định DP1–DP6.
