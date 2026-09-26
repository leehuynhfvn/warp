# Agent Ops — Warp làm trung tâm cho agent và operator vận hành hệ thống — Roadmap (v1)

> Viết ngày 2026-09-25 (Claude Opus). Đây là **roadmap**, không phải implementation plan: mỗi phase
> O2+ sẽ có `IMPLEMENTATION_PLAN.md` riêng, viết khi tới lượt (sau khi gate của phase trước đạt).
> Phase O1 dùng plan có sẵn `specs/agent-bridge/IMPLEMENTATION_PLAN.md`.
> Tham khảo: fork `cesaryuan/warp-refined` (BYOK + endpoint OpenAI-compatible riêng, mục 6).

---

## 1. Mục tiêu

1. Nhiều agent của các hãng (Claude Code, Codex, Gemini CLI, Antigravity `agy`, …) làm việc sysadmin
   / DevOps **trên server thật**, qua session SSH/`sudo -i` đã Warpify, với quyền và danh tính của
   operator đang ngồi trước Warp.
2. Vận hành liên tục: alert/log từ Grafana (Loki, Prometheus) được agent **tự chẩn đoán**, đề xuất
   hành động; con người duyệt; Warp thực thi và ghi vết.
3. Một nhóm vài operator dùng chung: hộp incident, phân công, runbook chung, audit chung.

### Không làm (non-goals)

- Không dựng lại Warp Drive / Oz cloud / orchestration server của Warp (closed source, cần backend
  của Warp). Runbook và skill chia sẻ bằng **repo git**.
- Không dùng Warp GUI làm tiến trình chạy 24/7 (app desktop: laptop ngủ = vòng lặp chết).
- Không cho agent tự chạy lệnh **ghi** trên production trước khi có lớp policy/duyệt (O2) và số
  liệu tin cậy từ O4.
- Không tích hợp riêng API từng hãng. Cổng chung duy nhất là **MCP** (`warpctrl mcp`).

---

## 2. Nguyên tắc kiến trúc

| # | Nguyên tắc | Lý do |
|---|---|---|
| P1 | Warp = **bàn điều khiển của người + nơi thực thi**; bộ não chạy liên tục nằm ở **runner** ngoài Warp | Warp có session root, UI duyệt, danh tính operator; nhưng không chạy 24/7 |
| P2 | Mọi agent vào Warp qua **một cổng MCP** (Agent Bridge) | Claude Code, Codex, Gemini CLI đều là MCP client → thêm agent mới gần như miễn phí |
| P3 | **Tách agent đọc và agent ghi.** Agent đọc log/metric không có công cụ thực thi; chỉ xuất *đề xuất có cấu trúc* | Log là dữ liệu không tin cậy: một dòng log có thể chứa prompt injection ("run `curl … \| sh`") |
| P4 | **Duyệt ở phía Warp**, không chỉ dựa vào permission prompt của từng agent | Agent-agnostic; không bị bỏ qua bởi `--dangerously-skip-permissions`; một UX cho mọi agent và cho runner |
| P5 | Mức tự động tăng dần theo số liệu (bảng mục 3) | Tự động hoá chỉ sau khi đo được độ chính xác đề xuất |
| P6 | Patch vào Warp nhỏ, tách module, sau feature flag | Upstream đổi rất nhanh; warp-refined duy trì được nhờ giữ patch set nhỏ |

```
Grafana Alerting ──webhook──► Ops Runner + Hub (VM nhỏ, 24/7)             [O4, O6]
                               ├─ agent chỉ-đọc: mcp-grafana, Loki/Prometheus
                               ├─ Proposal JSON (chẩn đoán + lệnh + mức L)
                               └─ SQLite: incident, proposal, audit, phân công
                                         │ MCP / REST
   Warp của từng operator ◄──────────────┘                                 [O5]
     ├─ Claude Code / Codex / Gemini ── MCP ──► Agent Bridge (`warpctrl mcp`) [O1]
     ├─ Policy + hộp thoại duyệt (L0–L3)                                   [O2]
     └─ session SSH → sudo -i (Warpify) ──in-band──► server
```

---

## 3. Mức tự động

| Mức | Loại hành động | Cách chạy | Có từ phase |
|---|---|---|---|
| L0 | Chẩn đoán chỉ-đọc (`systemctl status`, `journalctl`, `ss`, `df`, đọc file) | Tự chạy (session attach read-only hoặc allowlist đọc) | O1 |
| L1 | Runbook đã duyệt trước, khớp **nguyên văn** allowlist (vd `systemctl restart php-fpm`) | Tự chạy, ghi audit, báo operator | O2 (host lab); prod chỉ sau gate O4 |
| L2 | Mọi lệnh ghi khác | Hộp thoại duyệt trong Warp | O2 |
| L3 | Denylist: `rm -rf /…`, `mkfs`, `dd if=`, `shutdown/reboot`, flush firewall, `DROP DATABASE`, sửa `sshd`/network khi không có console dự phòng | Luôn từ chối, kể cả khi được duyệt | O2 |

Denylist bằng regex chỉ là **gờ giảm tốc** (shell có thể che giấu: `eval "$(echo … | base64 -d)"`).
Ranh giới thật là: chế độ read-only, hộp thoại duyệt, và allowlist L1 so khớp **toàn bộ lệnh** +
từ chối mọi metachar shell (`; | & $ \` > <` và xuống dòng).

---

## 4. Các phase

### O0 — Hoàn tất Warp Sync (đang làm)

Còn CHECKPOINT E (user test extension VS Code, checklist 20–25). Không chặn O1: Bridge được rebase
lên `feature/warp-sync`; fix sau này của Warp Sync thì rebase lại.

### O1 — Agent Bridge v1

Làm theo `specs/agent-bridge/IMPLEMENTATION_PLAN.md` (Phase 0–4), **cộng 2 điều chỉnh** đã ghi vào
plan đó (D11, D12):

- **D11** — Chữ trên palette/toast/lỗi trung lập với agent ("Allow agents to control this
  session"), không chỉ nói Claude Code. Mục 7 của plan có thêm lệnh setup cho Codex / Gemini CLI.
- **D12** — Params `remote.*` có trường tuỳ chọn `agent: Option<String>` (MCP adapter điền từ
  `initialize.params.clientInfo.name`, CLI điền `"warpctrl-cli"`); audit log ghi `agent` và
  `request_id`. Thêm bây giờ thì rẻ; thêm sau phải đổi protocol.

**Gate → O2:** CHECKPOINT B đạt với Claude Code; thử thêm ít nhất 1 MCP client khác (Codex hoặc
Gemini CLI) gọi `list_sessions` + `exec` thành công; độ trễ in-band đo ở CHECKPOINT A chấp nhận được.

### O2 — Policy + duyệt phía Warp

Tương ứng mục "duyệt từng lệnh phía Warp" trong Phase 5 của plan Bridge, được nâng lên thành phase
riêng. Phạm vi dự kiến:

- File policy `~/.warp/agent-ops/policy.toml` (quyền `0600`), ví dụ:
  ```toml
  [defaults]
  mode = "approve"                 # read_only | approve | allowlist

  [[hosts]]
  match = "lab-*"                  # glob trên hostname
  mode = "allowlist"
  allow = ["systemctl restart php-fpm", "systemctl reload nginx"]   # khớp nguyên văn

  [deny]
  patterns = ['\brm\s+-rf\s+/', '\bmkfs', '\bdd\s+if=', '\b(shutdown|reboot|halt)\b',
              'iptables\s+-F', 'nft\s+flush', '(?i)drop\s+(database|table)']
  ```
- Hàm thuần `policy::evaluate(host, user, command, attach_access) -> Decision {Allow, Ask, Deny(reason)}`
  (unit test dày), gọi **một chỗ duy nhất** trong handler `remote.*` trước `ops::exec`/`write`.
- `Ask` → request ở trạng thái `BridgeResult::Pending` (đã có từ Warp Sync 7.1) chờ **hộp thoại
  duyệt** trong Warp: host, user, agent (D12), lệnh đầy đủ (hoặc diff với write/edit), nút
  Approve / Deny / "Approve this exact command for this session". Không trả lời sau 5 phút → Deny.
- Kill switch sẵn có (Revoke all) + trạng thái policy hiển thị trên chỉ báo attach (Task 4.2 Bridge).
- Tuỳ chọn: bỏ allowlist `exec` khỏi permission Claude Code khi đã duyệt phía Warp (tránh duyệt hai lần).
- **Pairing token** (chuyển từ Phase 5 của plan Bridge, D27 ở đó): ghép cặp từng MCP client với Warp để
  `agent` trong hộp thoại duyệt/audit là danh tính đã xác minh (không phải tên tự khai, D12 của Bridge),
  revoke được theo agent. Không chặn được process cùng UID (đọc được token), nên giá trị nằm ở danh tính,
  là điều kiện của G2 (AO7). Hộp thoại ghép cặp dùng chung UI với hộp thoại duyệt.
- Hộp thoại duyệt nên dùng lại `remote.exec.visible` của Bridge (Phase 5): lệnh đã duyệt chạy thành block
  thật trước mắt operator.

**Gate → O3/O4:** dùng hằng ngày ≥ 1 tuần trên host lab không có sự cố; mọi lệnh ghi đều đi qua
hộp thoại hoặc allowlist; audit log đủ.

### G — Warp làm cổng SSH cho agent (thêm 2026-09-25, chưa có plan chi tiết)

Mục tiêu: operator khai báo server **một lần** trong Warp (gồm credential, lưu an toàn); bất kỳ agent
local nào (qua MCP, P2) cũng có thể **tự mở một hoặc nhiều session** tới server đã khai báo, sửa file
qua **mirror Warp Sync** (có lịch sử Git ở local), mọi thao tác nằm trong audit. Kết luận khảo sát:
**làm được**, phần lớn dựa trên thứ đã có:

| Đã có | Dùng cho |
|---|---|
| `warpui_extras::secure_storage` (Keychain / libsecret, fallback file owner-only) | Lưu mật khẩu SSH/sudo, passphrase |
| Local control `tab.create` + Warpify SSH + Agent Bridge `remote.*` | Mở session và điều khiển nó |
| Warp Sync: mirror + Git baseline + upload có backup/kiểm xung đột + hộp thoại xác nhận | Sửa file có lịch sử, diff, rollback |
| Audit của Bridge (`request_id`, `agent`) | Dòng thời gian thao tác theo server |
| Generator autocompletion `ssh` của `warp-command-signatures` (`SSH_CONFIG_CMD` đọc `~/.ssh/config` + `Include`, `known_hosts_file`) | Mẫu đọc ssh config. **Không phải kho host**: chỉ chạy `cat` trong shell lúc gợi ý lệnh `ssh`, không lưu gì (kiểm 2026-09-26) → danh bạ phải làm mới (G1) |
| `crates/warp_tui` (front-end headless) | Tuỳ chọn về sau: chạy cổng này trên máy trung gian 24/7 |

**Thứ tự bắt buộc:** G2 trở đi làm **sau O2**. Agent tự mở session root nghĩa là bỏ bước "người
bấm Attach" — lớp an toàn chính của O1 — nên phải có policy + hộp thoại duyệt và pairing token trước.

- **G1 — Danh bạ server** (mở rộng 2026-09-26). Do **người dùng** thao tác nên không cần O2 — làm
  được ngay sau O1, trước G2. UI: trang Settings + palette "Agent Ops: Add server".
  - **G1a — Kho host.** `~/.warp/agent-ops/hosts.toml` (`0600`, không chứa bí mật). Mỗi host:
    `alias`, `source` (`ssh_config` | `warp`), `tags`, cách xác thực (`key` / `agent` / `password`),
    cách lên root (`root_login` / `sudo_nopasswd` / `sudo_password` / `none`), `requiretty`,
    transport ưu tiên, `mirror_key` + `machine_id` (G1d). Host `source = ssh_config` **không chép**
    HostName/User/Port/ProxyJump/IdentityFile: khi dùng thì hỏi `ssh -G <alias>` (OpenSSH tự giải
    `Include`/`Match`/wildcard), nên không bao giờ lệch với file gốc. Bí mật (mật khẩu SSH, mật khẩu
    sudo, passphrase) chỉ lưu khi user nhập, trong secure storage theo khoá `host:<alias>:ssh_password`
    / `…:sudo_password`.
  - **G1b — Tự nhập từ `~/.ssh/config`.** Đọc `~/.ssh/config` + các file `Include` lúc khởi động, khi
    file đổi và khi mở picker. Alias cụ thể mới (bỏ `Host *`, pattern có `*`/`?`/`!`, khối `Match`) →
    thêm với `source = ssh_config`, chưa có tag; toast "Found N new SSH hosts" để user gắn tag / thêm
    mật khẩu. Alias biến mất khỏi file → đánh dấu `missing`, giữ metadata (tag, mật khẩu) tới khi user
    xoá.
  - **G1c — Ghi ngược ra ssh config chuẩn + tag.** Host tạo trong Warp (`source = warp`) ghi vào **file
    riêng của Warp** `~/.ssh/config.d/warp.conf` (`0600`, ghi atomic, backup bản trước, kiểm lại bằng
    `ssh -G` sau khi ghi, lỗi thì khôi phục). Warp thêm **một lần** dòng `Include config.d/warp.conf`
    vào đầu `~/.ssh/config` (hỏi user, có backup). Warp **không sửa** khối user tự viết: giữ nguyên
    comment/thứ tự/`Match`, và mỗi host chỉ có **một nơi sở hữu** nên không có vòng lặp đồng bộ hai
    chiều. Sửa host `ssh_config` trong Warp chỉ đổi metadata của Warp; muốn Warp quản lý luôn thì
    "Move to Warp" (chuyển khối sang `warp.conf`, có xác nhận). Nhờ vậy `ssh <alias>`, `scp`, VS Code
    Remote-SSH, Ansible và agent dùng Bash vẫn chạy khi không có Warp. **Mật khẩu không ghi ra** (ssh
    config không có trường mật khẩu, và không để process khác đọc được) → ngoài Warp, host chỉ có mật
    khẩu vẫn phải gõ tay. Tag: trong `warp.conf` là comment máy đọc được ngay trên khối
    (`# warp:tags=prod,project-x`; ssh bỏ qua, người/agent `grep` được); host `ssh_config` giữ tag
    trong `hosts.toml`. Không dùng từ khoá `Tag` của OpenSSH ≥ 9.4: nó chỉ nhận một giá trị và dùng
    cho `Match tagged` (đổi cách kết nối), không phải để phân loại.
  - **Quick connect.** Palette "Connect to server…" (tìm theo alias/tag): mở tab, `ssh <alias>` (mật
    khẩu qua `SSH_ASKPASS` như G2, không gõ qua PTY), lên root theo cấu hình, Warpify, tuỳ chọn Attach
    luôn (user tự bấm nên vẫn đúng mô hình O1). "Reconnect" cho tab SSH bị rớt.
  - **G1d — Nối với Warp Sync.** Mirror không đặt theo alias ssh mà theo **máy**: Warp Sync
    (`transfer.rs::resolve_host_key`) dùng thư mục `host_key(hostname)` (hostname server tự báo, vd
    `draff3`); nếu thư mục đó đã thuộc máy khác (manifest lưu `/etc/machine-id`) thì dùng
    `<host_key>-<sha256(machine_id) rút gọn>`. Vì vậy kho **không** suy mirror từ hostname: sau lần
    Sync/kết nối đầu, kho ghi `mirror_key` (tên thư mục mirror đã giải) + `machine_id` của host. Hệ quả:
    hai server trùng hostname → hai mirror riêng; hai alias cùng trỏ một máy (IP công khai và qua jump
    host) → dùng chung một mirror (đúng ý). Lần kết nối sau, machine-id khác bản đã lưu → cảnh báo
    (server cài lại, hoặc alias giờ trỏ tới máy khác) và **không** dùng mirror cũ cho tới khi user xác
    nhận. Giới hạn đang có của Warp Sync, G1 không sửa: (a) chỉ có 2 ứng viên, máy thứ ba cùng hostname
    bị từ chối; (b) máy không có machine-id (BusyBox, một số container) chỉ dùng được thư mục trần;
    (c) VM clone từ cùng template mà không reset `/etc/machine-id` thì Warp không phân biệt được — có
    thể thêm vân tay host key SSH làm tín hiệu phụ (cũng hay bị clone). MCP tool chỉ-đọc `list_hosts` (không
    cần attach): alias, tag, `user@host`, transport, session đang mở/attach, thư mục mirror và các path
    đã sync → agent biết sửa file của server đó qua mirror (G4) thay vì ghi thẳng. Không trả bí mật.
    Lưu ý: agent đọc mirror bằng Read tool local thì **không** qua redaction/audit của Bridge → policy
    của G4 phải loại path bí mật khỏi mirror.
- **G2 — Agent tự mở session.** MCP tool `open_session {host, access, purpose}` → policy (O2) quyết
  định: tự mở (lab), hỏi (prod), từ chối. Warp mở tab trong nhóm "Agents" (luôn **hiển thị**, user
  nhìn và giành lại quyền được), ssh bằng credential trong kho, lên root theo cấu hình, Warpify, tự
  attach với mức quyền policy cho phép; `close_session`; giới hạn số session theo host/agent. Nhiều
  session cùng host → agent làm **song song** (giải quyết hạn chế một lệnh một lúc của in-band).
  Bí mật **không bao giờ** tới agent, audit hay log:
  - Mật khẩu SSH: `ssh` local gọi `SSH_ASKPASS` (`SSH_ASKPASS_REQUIRE=force`) trỏ tới helper
    `warp --warpctrl askpass`, helper lấy mật khẩu qua broker — không gõ qua PTY.
  - Mật khẩu sudo (chạy trên server nên askpass local không dùng được): Warp chỉ tự điền **một lần**,
    ngay sau khi chính nó gửi `sudo -i` trong session nó vừa mở, khi thấy prompt khớp, có timeout —
    chống chương trình giả prompt `[sudo] password for` để lấy mật khẩu.
- **G3 — Transport theo host, cùng một API `remote.*`.** Agent không cần biết bên dưới:

  | Server | Transport | Song song |
  |---|---|---|
  | Key + `sudo NOPASSWD` (có hoặc không `requiretty`) hoặc SSH thẳng root | **Kênh exec trực tiếp**: Warp chạy `ssh` (ControlMaster) cho từng lệnh, `-tt` khi `requiretty`; không qua PTY của user | Có |
  | Cần mật khẩu sudo, hoặc chỉ có mật khẩu | **Session PTY in-band** (Bridge hiện tại), Warp tự điền mật khẩu như G2 | Theo số session |
  | Server user đã tự mở tay | Attach thủ công như O1 | Không |
- **G4 — Sửa file qua mirror Warp Sync** (cũng là hạng mục "tích hợp Warp Sync" trong Phase 5 của plan
  Bridge, chuyển sang đây theo D27 ở đó). `edit_file`/`write_file` của MCP: tải file vào mirror (nếu
  chưa có) → áp thay đổi trong mirror → upload bằng Warp Sync (backup trên server, kiểm xung đột) →
  commit Git trong mirror với message ghi `host`, `agent`, `request_id`, người duyệt. Hộp thoại duyệt
  O2 hiển thị **diff** (Sync đã có hộp thoại xác nhận). Rollback = `git revert` + upload. Lưu ý:
  mirror giữ bản sao file chỉ root đọc được → thư mục `0700`, khuyến nghị mã hoá đĩa, policy loại
  trừ đường dẫn bí mật (`/etc/shadow`, khoá SSH, `*.key`).
- **G5 — Dòng thời gian theo server.** Gộp audit Bridge + `git log` của mirror + block trong session
  theo `request_id`: "agent nào đã làm gì trên server X, lúc nào, ai duyệt".

**Rủi ro riêng của G:** (1) Warp thành kho credential + tự mở phiên root → process local nào gọi được
MCP là mối đe doạ lớn hơn O1: pairing token + policy là điều kiện tiên quyết. (2) Khó nhất không phải
*lưu* mật khẩu mà là *dùng* nó mà không lộ (prompt giả, lịch sử shell, log `safe_info!` của executor
in-band — D21 của Bridge). (3) Mirror tích luỹ bản sao file nhạy cảm ở máy local.
**Lời khuyên:** với server mình quản lý được, tài khoản `ops` riêng dùng key + `sudo NOPASSWD` (có log
sudo phía server) đơn giản và dễ kiểm toán hơn lưu mật khẩu; kho mật khẩu của G1 dành cho server
không đổi được cấu hình.

**Gate G:** G1 dùng được cho ≥ 3 server thật (host tạo trong Warp `ssh <alias>` được từ terminal
thường; tag còn nguyên sau khi sửa ở cả hai phía; `~/.ssh/config` do user viết không bị đổi byte nào
ngoài dòng `Include`); G2 chỉ bật sau gate O2; kênh exec trực tiếp (G3) đo
được song song trên host `sudo NOPASSWD`; G4 có lịch sử + rollback thử trên host lab.

### O3 — Bộ công cụ quan sát + runbook (hầu như không sửa Warp)

- Gắn **`grafana/mcp-grafana`** (MCP chính thức của Grafana) cho Claude Code / Codex / Gemini CLI.
  Service account Grafana **role Viewer** (quyền đọc được Grafana cưỡng chế phía server, không dựa
  vào prompt). Máy này đã cấu hình sẵn các server `grafana-*` trong Claude Code nhưng đang lỗi
  `CONNECTION_CLOSED` — sửa trước.
- Repo git `ops-runbooks/` dùng chung: mỗi runbook là Markdown (triệu chứng → truy vấn Loki/PromQL →
  lệnh chẩn đoán L0 → hành động L1/L2 → kiểm tra sau), kèm skill cho agent (vd `triage`,
  `nginx-5xx`, `disk-full`). Thay đổi runbook đi qua PR (người review).
- Quy trình tương tác: operator dán alert → agent truy vấn Grafana (đọc) → chẩn đoán → đề xuất →
  thực thi qua Bridge (O2 duyệt).

**Gate → O4:** ≥ 10 incident thật xử lý theo quy trình này; ghi lại chẩn đoán agent đúng/sai.

### O4 — Runner nhận alert (ngoài Warp, chạy 24/7)

- **Spike trước (1–2 ngày): HolmesGPT** (robusta-dev/holmesgpt, CNCF sandbox) — agent điều tra alert
  chỉ-đọc, có sẵn toolset Prometheus/Loki/Grafana, cấu hình model qua LiteLLM. Chỉ tự viết runner
  nếu HolmesGPT không hợp.
- Runner tự viết (nếu cần): Grafana contact point **webhook** → hàng đợi → `claude -p` /
  `codex exec` với **chỉ MCP đọc** (không Bash, không Bridge) → xuất `Proposal` theo JSON schema,
  **validate schema**; output không hợp lệ → loại.
  ```json
  { "incident_id": "…", "alert": "…", "host": "web-3", "summary": "…",
    "evidence": [{"source": "loki", "query": "…", "excerpt": "…"}],
    "actions": [{"command": "systemctl restart php-fpm", "level": "L1", "rationale": "…",
                 "runbook": "php-fpm-hang"}] }
  ```
- Log trong prompt luôn được bọc như **dữ liệu** (khối có delimiter, dặn không làm theo chỉ dẫn bên
  trong); `level` do policy tính lại, không tin giá trị model tự gán.
- Model: API key qua gateway (LiteLLM) có **ngân sách** theo ngày; model rẻ cho bước phân loại, model
  mạnh cho điều tra sâu. Thông báo Telegram/Slack kèm tóm tắt.

**Gate → O5:** chạy "shadow" ≥ 2 tuần (chỉ đề xuất, không ai bắt buộc làm theo); đo tỉ lệ đề xuất
đúng theo đánh giá operator. Chưa đạt ngưỡng đã thống nhất → không bật L1 cho prod.

### O5 — Hộp incident trong Warp

- **O5a (rẻ, làm trước):** thêm MCP tools `list_incidents` / `get_incident` / `post_result` (trong
  `warpctrl mcp` hoặc một MCP server riêng của Hub). Operator nói với Claude Code "xử lý incident
  #42" → agent lấy proposal, chạy qua Bridge (O2 duyệt), gửi kết quả về Hub.
- **O5b (GUI, chỉ khi O5a chứng minh giá trị):** panel Incidents trong Warp, nút "mở session tới
  host", "chạy đề xuất" (đi qua đúng `policy::evaluate` + hộp thoại của O2).

### O6 — Làm việc theo nhóm

Hub nhiều người dùng: token/OIDC theo operator, phân công + on-call, audit tập trung (Warp đẩy dòng
audit lên Hub), policy phát từ Hub (operator không tự nới quyền prod), bật L1 cho prod theo từng
runbook dựa trên số liệu O4. Mỗi operator dùng tài khoản agent **của chính mình** (mục 5).

### Tuỳ chọn — Agent sẵn có của Warp chạy qua gateway riêng (kiểu warp-refined)

Chỉ cần nếu muốn dùng Agent Mode của Warp (không phải Claude Code/Codex) mà không qua server Warp.
Upstream đã có `app/src/ai/custom_endpoints.rs`; phần warp-refined thêm là gọi thẳng
`/v1/responses` của endpoint OpenAI-compatible (bỏ qua `/ai/multi-agent`) và giữ reasoning items qua
nhiều lượt. Ưu tiên thấp; mỗi lần rebase upstream sẽ tốn công.

---

## 5. Dùng tài khoản thuê bao hay API key

| Agent | Thuê bao dùng được khi | Nên dùng API key / cloud khi |
|---|---|---|
| Claude Code | Pro/Max hoặc seat Team/Enterprise; chạy **chính binary `claude`** (tương tác hoặc `claude -p`) | Runner 24/7 dùng chung → API key, Bedrock hoặc Vertex |
| Codex | Đăng nhập ChatGPT; `codex exec` | Tự động hoá kiểu CI/runner |
| Gemini CLI / Antigravity `agy` | Tài khoản Google, có quota | Điều khoản cho dùng tự động chưa rõ → kiểm tra trước |
| Agent Mode của Warp | Credit Warp | BYOK / custom endpoint |

- Operator tương tác (O1–O3, O5a): mỗi người đăng nhập agent bằng tài khoản **của mình**; Bridge chỉ
  là MCP server nên không đụng tới credential của agent.
- **Không** trích OAuth token của gói thuê bao để gọi API từ công cụ khác (Anthropic đã chặn).
- Runner (O4): API key qua gateway, có ngân sách + log. Gói thuê bao có hạn mức theo cửa sổ 5 giờ và
  theo tuần → không hợp vận hành liên tục.
- Điều khoản các hãng đổi thường xuyên: đọc lại ToS trước O4 và O6.

---

## 6. Rủi ro

| Rủi ro | Giảm thiểu |
|---|---|
| Prompt injection qua log/alert dẫn tới lệnh nguy hiểm | P3 (agent đọc không có công cụ ghi), schema validate, `level` do policy tính, duyệt O2, denylist L3 |
| Agent chạy lệnh phá huỷ với quyền root | Read-only attach, O2 duyệt, allowlist khớp nguyên văn, backup trước khi ghi (Bridge D5) |
| Lộ secret server cho model/nhà cung cấp | Redaction ở MCP adapter (Bridge D8); runner chỉ đọc qua Grafana role Viewer; ghi chú: output vẫn có thể lọt secret không khớp regex |
| Process local khác gọi Bridge | Attach thủ công theo `SessionId`, TTL, Revoke all, O2 duyệt; pairing token (Bridge v2) trước O6 |
| Chi phí token runner vượt kiểm soát | Gateway có ngân sách/ngày, dedupe alert, model rẻ cho phân loại |
| Fork khó rebase upstream | P6; phần lớn O3–O6 nằm **ngoài** repo Warp |
| Operator tin agent quá mức | Gate có số liệu trước mỗi bước tăng tự động; audit + review định kỳ |

---

## 7. Tiến độ và quyết định

### Tiến độ

- [ ] O0 Warp Sync — CHECKPOINT E
- [ ] O1 Agent Bridge v1 (theo plan riêng, gồm D11, D12) · [ ] gate O1
- [ ] O2 Policy + duyệt phía Warp (plan: `specs/agent-ops/O2_POLICY_PLAN.md`, chưa viết)
- [ ] G1 danh bạ server (G1a kho · G1b tự nhập từ ssh config · G1c ghi ngược + tag · quick connect · G1d nối Warp Sync) · [ ] G2 agent tự mở session (sau O2) · [ ] G3 transport theo host · [ ] G4 sửa file qua mirror Warp Sync · [ ] G5 dòng thời gian (plan: `specs/agent-ops/G_GATEWAY_PLAN.md`, chưa viết)
- [ ] O3 mcp-grafana + `ops-runbooks` · [ ] gate O3
- [ ] O4 spike HolmesGPT · [ ] runner · [ ] shadow 2 tuần
- [ ] O5a MCP incident tools · [ ] O5b panel GUI
- [ ] O6 Hub nhóm

### Quyết định

| # | Ngày | Quyết định | Lý do |
|---|---|---|---|
| AO1 | 2026-09-25 | Warp là console + nơi thực thi; runner 24/7 nằm ngoài Warp | App desktop không chạy liên tục được |
| AO2 | 2026-09-25 | Cổng tích hợp agent duy nhất = MCP của Agent Bridge | Đa hãng, không phụ thuộc Oz cloud |
| AO3 | 2026-09-25 | Duyệt lệnh ghi ở phía Warp (O2), không chỉ dựa vào prompt của agent | Agent-agnostic, không bị bỏ qua bằng cờ của agent |
| AO4 | 2026-09-25 | Runbook/skill chia sẻ bằng repo git, không dựng lại Warp Drive | Có review qua PR, không cần backend |
| AO5 | 2026-09-25 | Thử HolmesGPT trước khi tự viết runner | Tránh viết lại thứ đã có |
| AO6 | 2026-09-25 | Thêm nhánh G: Warp làm cổng SSH cho agent (danh bạ server + kho credential, agent tự mở session, transport theo host, sửa file qua mirror Warp Sync có lịch sử) | Người dùng muốn đơn giản hoá quản lý server; giải quyết giới hạn một-lệnh-một-lúc của in-band mà vẫn giữ credential, policy, audit ở một chỗ |
| AO7 | 2026-09-25 | G2 (agent tự mở session) chỉ làm sau O2 và pairing token | Bỏ bước Attach thủ công là bỏ lớp an toàn chính của O1 |
| AO8 | 2026-09-25 | Bí mật không bao giờ tới agent: mật khẩu SSH qua `SSH_ASKPASS` local, mật khẩu sudo chỉ tự điền ngay sau `sudo -i` do Warp gửi | Chống lộ qua MCP/log/lịch sử shell và chống prompt giả |
| AO9 | 2026-09-26 | Danh bạ đồng bộ hai chiều với ssh config theo **quyền sở hữu từng host**: host của user nằm trong `~/.ssh/config` (Warp chỉ đọc), host của Warp nằm trong `~/.ssh/config.d/warp.conf` (Warp ghi), nối bằng một dòng `Include` | Không phá file user tự viết; không vòng lặp đồng bộ; công cụ ngoài Warp vẫn dùng được host |
| AO10 | 2026-09-26 | Host nhập từ ssh config chỉ lưu alias + metadata của Warp; giá trị kết nối luôn lấy từ `ssh -G` | OpenSSH giải `Include`/`Match`/wildcard đúng hơn parser tự viết; không lệch dữ liệu |
| AO11 | 2026-09-26 | Tag lưu bằng comment `# warp:tags=…` (trong `warp.conf`) hoặc `hosts.toml` (host của user); không dùng `Tag` của OpenSSH | `Tag` chỉ một giá trị và đổi hành vi `Match tagged` |
| AO12 | 2026-09-26 | Host nối với mirror Warp Sync bằng `mirror_key` + `machine_id` ghi lại sau lần Sync đầu (không suy từ hostname); machine-id đổi → cảnh báo, không dùng mirror cũ; MCP `list_hosts` trả thư mục mirror, không trả bí mật | Warp Sync đặt mirror theo máy (hostname, thêm hậu tố hash machine-id khi trùng tên), không theo alias ssh |
