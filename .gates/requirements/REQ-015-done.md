---
doc_type: proposal
tier: critical
owner: -
review_policy: codebound
verified_at: 2026-10-05
source_refs: [core/src, cli/src, gui/src, tui/src, README.md]
---

# REQ-015 界面补齐需求生命周期：done 标注完成 / 归档 / 归档区浏览 + 审核人身份自动填充

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-015 status=approved created=2026-10-05_13:35:31 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:20:44 sum=9c64c8f9c1fbecb32ae68daea809df6ee95d78b35842cea9f1f8357727a6b50d -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-09_12:57:53 sum=f9cdad0940e92d454c4e8a5f4dc5c5e16345e54352403b9a733cce62687f589e -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:21:39 sum=7928e986af1118d1228c02b3f985ae305d5fbbb7c823a0b799334d6c71dc4890 -->

## 1. 需求分解

### 背景与问题

GUI 与 TUI 都能改**段级**状态（批准 / 打回 / 修订 / 封存），但**改不了需求级状态**——
整个生命周期在界面上缺了最后一环。逐条查证如下（均为代码走读 + 本机实测，非推测）：

1. **`requirement::done` 在两个界面里零引用**。
   `core/src/requirement.rs:991` 定义了 `done(root, id, actor)`：把 `GATE:HEAD` 的
   `status=` 置为 `done` 并写入 `done=` 时间戳。该函数在 `gui/src/` 与 `tui/src/` 下
   **一次都没出现**（`requirement::done` 全文搜索命中 0 次）。CLI 有 `done` 子命令
   （`cli/src/cli.rs:234`，处理函数 `cli/src/main.rs:223-249`），界面侧完全空白。
   后果：`ReqState::Done`（`core/src/status.rs:94`，中文标签**已归档**）
   在界面上不可达，审核人无法在界面里结束一份需求。

2. **物理归档与归档区浏览同样缺失**。
   `requirement::archive_one`（`requirement.rs:1122`）与
   `archive_due`（`requirement.rs:1061`）在两个界面里都没有接线，
   `status::req_list_archived`（`core/src/status.rs:209`，CLI `status --archived` 的数据源）
   也没有界面入口。归档区在界面上是个**完全不可见的目录**：
   `.gates/requirements/archive/<年>/` 里的历史清单既列不出、也打不开。

3. **`done` 之后界面会「少一份需求」，而用户不知道为什么**。
   `resolve.rs` 的 live 快照只含未归档清单，所以 `done` 一落盘，
   界面左栏那份清单就消失了。这本身是正确行为，但**没有提示、没有撤销入口、没有确认**——
   审核人点错一次就永久丢失入口（只能靠 CLI 或手改文件找回）。

4. **`done` 是高危动作，界面必须比 CLI 更谨慎**。
   `requirement.rs:985-990` 已经写明理由：

   > 归档属审批类动作：AI 若能自归档，把带阻塞评论的需求归档、再 `create` 新需求，
   > 老需求上的阻塞即失效（历史缺陷"阻塞评论只作用于最新活跃需求"的另一半）

   它走 `auth::ensure_human` 且 scope 绑定到 `done:{id}`（`requirement.rs:995`），
   实测 L3 下裸跑 CLI 会被拒（需一次性票据）——**界面里点一下就能过，
   等于给这个高危动作开了一条绕过票据的近道**。这不是反对做，
   而是要求界面做**比 CLI 更强**的确认（见 G4）。

5. **审核人姓名每次都要手打**。
   GUI 的批准 / 打回 / 修订 / 评论 / 封存（`gui/src/app.rs:195`、`:260`、`:319`、`:474`）
   与 TUI 的对应弹窗（`tui/src/app.rs:781`、`:789`、`:806`）都是裸输入框，
   没有任何默认值。而 CLI 走 `resolve_identity`（`cli/src/main.rs:725-749`）的
   三级回退：**显式参数 → `REQ_GUARD_REVIEWER` 环境变量 → git `user.name`**。
   更糟的是：填错姓名在 L1+ 会被 `identity::decide`（`identity.rs:242-250`）
   直接拒（"审批人身份冲突"），于是**用户必须在输入框里精确敲出与 git 配置一致的字符串**
   ——而正确答案就在界面上，一键就能取到，却要靠人肉比对。

6. **`identity::describe` 与 `auth::effective_level` 在界面上没有任何展示**。
   CLI 有 `whoami`（`cli/src/main.rs:456-494`）打印姓名 / 邮箱 / `sig` / 鉴权等级 /
   是否 AI 上下文。界面不显示这些，用户无法判断「我现在能不能审批」。

### 目标

- **G1 界面可标注完成**：GUI 与 TUI 都能对当前需求执行 `requirement::done`。
  凭据 scope 传 `done:{id}`（与 `requirement.rs:995` 的 `ScopeCheck::Exact` 严格一致，
  不得传 `"done"` 之类——L3 会因范围不匹配拒）。
- **G2 界面可物理归档**：GUI 与 TUI 提供「归档」动作，对应 `archive_one`
  （单份）与 `archive_due`（到期清扫），并提供 `--dry-run` 等价的**预演**入口
  （`archive_one(root, actor, id, true)`，只算不搬移、不写审计）。
  搬移目标由 core 决定（`archive/<创建年>/`），界面**不自造路径**。
- **G3 归档区只读可浏览**：两个界面都能列出 `status::req_list_archived` 的结果并查看详情，
  明确标注「只读历史」。归档区内的需求**不可**再被批准 / 打回 / done
  （core 的写操作已对 `status=done` 设护栏，见 `requirement.rs:818-825` 的
  「归档即终点」，界面不得绕过，也不得提供看似可用的按钮）。
- **G4 `done` 的确认强度高于 CLI**：执行前必须弹二次确认，且确认框里**必须同时显示**
  三项事实：① 当前 `ReqState` 与三段通过数；② **未关闭的阻塞评论数**（若有则要求额外勾选）；
  ③ 「归档后门禁将不再管辖本需求，阻塞评论随之失效」这一后果。
  理由见背景第 4 条——这是唯一能把 AI 自归档风险挡在人类判断之后的界面级手段。
  另：`done` 成功后面板必须提示「已归档 + 若需找回请用 CLI `archive` 的逆操作」，
  因为界面本期不提供撤销（N3）。
- **G5 审核人默认填好，且三级回退与 CLI 逐字一致**：
  把 `resolve_identity` 的逻辑下沉到 core（现它在 `cli/src/main.rs:725-749`，是 CLI 私有），
  三个前端共用同一实现——**绝不允许 GUI/TUI 各写一份回退链**，
  否则两份实现必然漂移（这正是本项目反复出现的问题形态）。
  界面输入框预填该结果，用户仍可改（改了就走 `identity::decide` 的一致性校验）。
- **G6 界面显示身份与权限现状**：顶栏或对话框内展示
  `identity::describe`（姓名 / 邮箱 / sig）与 `auth::effective_level`（L0–L3），
  并在身份取不到时给出可执行的补救提示（照搬 `identity.rs:294` 的三条取解文案）。
  让「为什么按钮点不动」在界面上自解释。
- **G7 核心判定零改动**：不改 `done` / `archive_*` / `identity` / `auth` 的任何语义，
  不新增 core 状态。唯一允许的 core 改动是 G5 的 `resolve_identity` 下沉
  （纯代码搬迁 + 三处调用点改写，不改行为）。

### 非目标

- **N1** 不引入「开发完成 / 已上线」这类新状态。`ReqState::Done` 的中文标签是
  **已归档**，语义是「移出门禁管辖」（`requirement.rs:985-991`），
  **不是**「任务做完」。用户口中的「标注完成」在本仓库就是这个语义。
  若将来确需独立的交付完成态，那是改状态机的新需求，不在本期。
  界面上必须把这个语义讲清楚（按钮文案用「归档（done）」而不是「完成」），
  否则用户会以为归档 = 交付完成。
- **N2** 不做 `done` 的撤销 / 复活（un-done）。core 里没有这个能力，
  本期也不新增——理由：`done` 已写审计且是门禁裁决的输入，
  复活会让「归档期间被跳过的门禁」出现一段无人负责的真空。
- **N3** 不在界面上做归档的**文件级**操作（打开归档文件、删除归档清单）。
  归档区只读可查即可。
- **N4** 不改 `archive.after_days` 的语义、不改自动清扫触发点
  （CLI 在 `done` 成功后调 `archive_due_authorized`，`main.rs:228-248`）。
  界面上 `done` 成功后**是否**自动清扫，本期定为**不自动触发**——
  自动搬文件是不可逆动作，界面上应让用户显式点「归档」或「清理到期」，
  与 CLI 的行为差异在此显式声明（G7 不改 core，CLI 侧一行不动）。
- **N5** 不把 `token` 管理（签发 / 查看 / 撤销）搬进界面（另见第三份需求）。
  界面只用 `auth::ui_issue_credential`，不暴露票据本身。
- **N6** 不动 `resolve` / live 快照逻辑：`done` 后清单从左栏消失是正确行为，
  不给界面加「已归档清单仍留在左栏」的特殊显示（那会与门禁口径不一致）。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | core：`resolve_identity` 从 CLI 下沉为 `identity::resolve_claimed`（三级回退），`cli/src/main.rs` 改调它，行为逐字不变 | 0.5h |
| T2 | core：下沉处补单测（flag 优先 / 环境变量 / git 身份 / 三者皆无报错），并锁「下沉前后 CLI 输出不变」 | 0.8h |
| T3 | GUI：新增 `Dialog::DoneConfirm`（含三项事实与阻塞评论勾选），走 `prepare_credential("done:{id}")` + `requirement::done` | 1.5h |
| T4 | GUI：`Dialog::Archive`（单份 + 到期清扫 + 预演），scope 传 `"archive"`，结果列出每条的 `src → dst` | 1.5h |
| T5 | GUI：归档区只读视图（列表 + 详情），从左栏切换时明确标注只读并禁用一切写动作 | 1.5h |
| T6 | GUI：全部审批类输入框预填 `identity::resolve_claimed`；顶栏展示 `describe` + `effective_level` | 1h |
| T7 | TUI：与 T3/T4/T6 对应的按键与弹窗（`d` 归档确认、`A` 归档预演/执行、`i` 身份信息），新增 `Prompt` 变体 | 2.5h |
| T8 | TUI：归档区只读视图（`v` 切换 live/archived 列表） | 1h |
| T9 | 单测：凭据 scope 正确性（L3 下 scope 不匹配必被拒，`done:{id}` 必通过） | 1.2h |
| T10 | 单测：确认框三项事实齐备（缺任一项时确认按钮不可用）+ 阻塞评论勾选门禁 | 1h |
| T11 | 单测：归档区视图下所有写动作按钮均不可用 | 0.8h |
| T12 | 判决性实验：把 scope 改成常量 `"done"`，对应单测必须 FAIL | 0.3h |
| T13 | 文档：README 的 GUI/TUI 操作说明补归档流程与语义澄清（归档 ≠ 交付完成） | 0.5h |

合计 14.6h。T1/T2 是 core 侧的纯搬迁，**必须先于** T3–T8 落地（否则前端要各写一份回退链，
正是 G5 要避免的）。T3–T6（GUI）与 T7/T8（TUI）可分别交付、分别回滚。

### 影响范围

- **模块**：`core/src/identity.rs`（新增回退函数）、
  `cli/src/main.rs`（`resolve_identity` 改为薄封装，删除私有三级回退）、
  `gui/src/app.rs`（新增 3 个 Dialog 变体 + 输入框预填 + 顶栏）、
  `tui/src/app.rs`（新增 `Prompt` 变体 + 按键 + 归档区列表）、`README.md`。
- **接口**：core **新增** `identity::resolve_claimed(root, flag_value: Option<&str>, label, flag)`
  （返回 `Result<String>`，错误文案与 `main.rs:736-748` 逐字相同）；
  `cli::resolve_identity` 保留为私有薄封装以免动 `main.rs` 里 12 处调用点。
  `identity::bind` / `decide` / `current` / `describe` / `auth::effective_level` 全部不变。
  GUI 新增 `Dialog::{DoneConfirm, Archive, ArchivedView}`；
  TUI 新增 `Prompt::{DoneConfirm, ArchiveActor, ArchiveReason}` 与 `Overlay::Archive`。
- **配置 / 清单格式 / 数据表 / 外部 API**：
  `GATE:HEAD` 的 `status=done` 与 `done=<时间戳>` 由 core 写，界面不碰格式；
  `.gates/req-guard.yaml` 不动（`archive.after_days` 语义不变）。
- **向后兼容**：CLI 行为逐字不变（T2 锁死）。界面新增入口，不改任何既有动作的行为。
  归档区视图是**新增**的只读区域，不影响 live 列表。
- **审计**：`done` 写 `DONE <id> actor=… channel=… ` 到本机日志与入库台账
  （`requirement.rs:1023-1032`），`archive_one` 每搬一条写 `ARCHIVE`
  （`requirement.rs:1200-1209`）。界面接线后这些事件会开始从界面产生——
  `channel` 字段会记为界面渠道（`auth::declared_channel()`），与 CLI 渠道可区分，属预期。

### 验收标准

见第 3 段 `GATE:AC` 块（编号连续、Given/When/Then 齐全且可度量）。

## 2. 技术方案


### 总体思路

**把 core 已有的生命周期动作接到界面，并给高危动作加一层界面独有的确认。**
`resolve_identity` 从 CLI 私有下沉到 core，三个前端共用一份实现——
这是本方案唯一的 core 改动，其余全部是「前端调什么、传什么 scope、显示什么」。

### 关键设计

1. **`done` 的凭据 scope 必须逐字匹配**（G1 的硬约束，T9 锁它）：
   `requirement.rs:995` 是 `ScopeCheck::Exact(&format!("done:{}", id))`，
   而 `token.rs:179` 的判定是 `cfg.scope.is_empty() || cfg.scope == s` ——
   **精确相等**。所以界面必须传 `format!("done:{id}")`；
   传 `"done"` 或 `"{id}"` 都会在 L3 下被拒。
   同理 `archive_one` / `archive_due`（`requirement.rs:1123`、`:1065-1067`）
   的 scope 是**固定字面量 `"archive"`**（不带 id）。两处 scope 形状不同，
   是历史遗留（done 按需求绑、archive 按动作绑），界面**照抄不得统一**——
   「统一」会破坏其中一处的绑定语义。

   凭据生命周期沿用既有纪律：动作前 `prepare_credential(scope)`，
   动作后 `clear_credential()`。core 内部已在落盘成功后调
   `auth::consume_credential_if_scoped()`（`requirement.rs:1019`、`:1148`），
   界面**不要**再手动消费——重复消费会导致同一凭据链上的第二个动作失败。

2. **确认强度高于 CLI**（G4 的落点，T10 锁它）：
   CLI 的 `done` 是一条命令，无确认（人的手就是确认）。界面上是按钮，
   且界面上 `done` 走 `ui_issue_credential` **免票据**（`auth.rs:246-255`）
   ——也就是说界面上点一下的门槛比 CLI 低。**低门槛 + 高危动作 = 必须补确认**。
   `Dialog::DoneConfirm` 的三个必显事实取自可复核的数据，不取自硬编码文案：
   - 事实 ①：`status::ReqStatus` 的 `ReqState` + `approved_count()` / 3
     （`status.rs:154`）——直接显示「2/3 已通过」；
   - 事实 ②：`blocking_comments`（`status.rs:134`）——**大于 0 时勾选框才可勾**，
     未勾选则确认按钮 disabled。这是把「归档会让阻塞评论失效」变成一个
     需要主动动作才能通过的关口，而不是一句警告文字；
   - 事实 ③：后果陈述（固定文案，因为它是语义不是数据）——
     「归档后门禁不再管辖本需求，其上的阻塞评论随之失效」。
   勾选门禁用「`blocking_comments > 0` 时必须勾」而不是「总是要勾」：
   没有阻塞评论时再要求一次无意义的勾选，只会训练用户闭眼勾。

3. **`ReqState::Done` 之后界面状态怎么变**（N6）：
   `done` 成功后该需求从 `status::req_list` 消失（live 集合不含 done）。
   界面处理：刷新列表 → 若原选中项已不在 live 集，自动选中相邻项 →
   并在消息区显示「已归档 REQ-0xx：门禁不再管辖本需求。
   如需找回，用 CLI `req-guard status --archived` 浏览归档区」。
   **不做**「留在左栏但标灰」——那会让界面与门禁口径不一致
   （门禁认为它不存在，界面却显示它还在），用户会据此误判门禁行为。

4. **`resolve_identity` 下沉的落点与形状**（G5，T1/T2）：
   从 `cli/src/main.rs:725-749` 搬到 `core/src/identity.rs`，签名取
   `pub fn resolve_claimed(root: &Path, flag: Option<&str>, label: &str, flag_name: &str) -> Result<String>`
   （`label` 与 `flag_name` 是错误文案里的「操作人」与 `--author`，
   现由 CLI 传 `"操作人"` / `"--author"` 或 `"审核人"` / `"--reviewer"`，
   故一并参数化，否则 core 会把 CLI 的具体命令名焊死）。
   三级回退与错误文案**逐字搬运**，不改一个字的输出。
   `cli::resolve_identity` 保留为 6 行薄封装转调它，
   以免动 `main.rs` 里 12 处调用点（reduce 风险面）。
   ⚠️ **落点必须在 core 而不是各前端**：三个前端各写一份三级回退，
   下一个功能（比如 token 管理的操作人）就会开始出现第四种写法。

5. **输入框预填的具体语义**（T6/T7）：
   预填 `identity::resolve_claimed(root, None, "审核人", "--reviewer")` 的结果，
   即「环境变量 → git user.name」这条链（`flag` 传 `None`）。
   预填值**可编辑**：用户改了名字就以他改的为准，
   最终归属由 `identity::decide` 判定（`identity.rs:230`）——
   L1+ 下改错会被拒，这是既有正确行为，界面只需把错误文案显示出来。
   ⚠️ 预填**不是**默认值回退：留空提交时不得静默采用预填值，
   必须报错（与 CLI 留空即报错一致）。否则「没填」与「填了预填值」
   在审计上不可区分，而这正是 L3 可归属性的基础。

6. **`archive_due` 的「预演」与「执行」**（G2）：
   `archive_one(root, actor, id, dry_run)` 的 `dry_run=true` 时
   只计算不搬移、不写审计（`requirement.rs:1060-1061`、`:1147-1149`），
   但**仍然过鉴权**（`ensure_human` 在函数首行，`:1123`），
   所以预演也要走 `prepare_credential("archive")`。
   界面因此是**两个动作**：`预演归档`（dry-run，结果显示 `src → dst`）
   与 `执行归档`（真实搬移）。`archive_due`（无参 = 到期清扫）
   同样给这两个动作，参数取 `gate::archive_after_days(root)`
   （`gate.rs:232`，读配置 `archive.after_days`，本仓库 30 天）。
   界面**必须显示这个天数**，否则用户不知道「到期」是多久。

   ⚠️ 幂等与部分失败：`archive_due` 是 best-effort——
   单条失败跳过不中断（`requirement.rs:1059-1060`），返回已搬列表。
   界面必须显示「成功 N 条 / 失败 M 条 + 失败原因」，**不能只显示成功数**
   （`archive_due_inner` 的逐条结果里不含失败原因，
   故界面文案只能是「已归档 N 条」+ 提示「若有遗漏请用 CLI 复核」，
   这是诚实边界，不掩饰）。

7. **归档区视图的只读实现**（G3，T5/T8）：
   `status::req_list_archived`（`status.rs:209`）返回的 `ReqStatus`
   里 `state` 恒为 `Done`。界面据此把写动作全部置灰，并在标题处标「只读历史」。
   关键：**不要**用 `is_archived_path` 之类的私有函数去判路径，
   直接判 `state == ReqState::Done`（`status.rs:94`）——
   前端拿到的数据已经判好了，再判一次就是第二个真相。
   归因：本条与 REQ-014 的 N3 同源（前端只渲染，不判定）。

8. **`resolve::live_snapshot` 与界面列表的一致性**：
   界面左栏当前用 `status::req_list`（经 `requirement::list`，只扫 live 目录）
   （`gui/src/app.rs:131`、`tui/src/app.rs` 的 `reload`）。
   `done` 之后 live 列表自然少一项，与门禁口径一致，无需额外处理。

9. **README 的语义澄清**（T13，N1 的落点）：
   按钮文案一律用「**归档（done）**」而非「完成」/「标记完成」，
   并在 README 的 GUI/TUI 操作说明里写清：
   `done` = 移出门禁管辖 ≠ 交付完成；交付完成的语义在本工具里不存在。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
core/src/identity.rs
cli/src/main.rs
gui/src/app.rs
tui/src/app.rs
tui/src/ui.rs
README.md
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

- **兼容性**：CLI 行为**逐字不变**——`resolve_identity` 是纯搬迁（T2 用
  「下沉前后 CLI 输出逐字相同」锁死）。`done` / `archive_*` / `review`
  等 core 函数的语义、签名、返回值全部不变。新增的唯一 core 符号是
  `identity::resolve_claimed`。
- **性能**：归档区视图首次打开要读 `archive/` 目录并逐个解析 frontmatter 与
  `GATE:HEAD`（`req_list_archived` → `scan_dir_recursive`，递归）。
  本仓库归档区为空，实测开销为 0；一个归档了 200 份需求的仓库上
  这是**一次性** O(文件数) 的读盘（列表已按相对路径排序，
  `requirement.rs:373`），应在打开视图时算一次并缓存，
  **不要**放进 3 秒轮询（`gui/src/app.rs:161`）——否则每 3 秒重扫归档区。
- **安全**（本需求是**收紧**安全，不是放宽）：
  - `done` 在 CLI 侧需 L3 一次性票据；界面侧走 `ui_issue_credential`
    免票据。**这是既有设计的必然**（`auth.rs:246-255`：界面进程内签发以
    "人类亲手操作界面"作为在场证明），本需求不改变该信任模型；
  - 补偿措施是 G4 的三级确认（事实齐备 + 阻塞评论勾选门禁 + 后果陈述），
    即**用「不可逆动作的知情同意」补偿「免票据」**。
    这是本方案里最重要的一条设计判断，必须写明而不是默认；
  - `archive_one` 物理搬移走 `fs::rename`（`requirement.rs:1189` 与 `:1195`，
    正文与评论成对搬移），git 识别为 rename，历史不丢——但**工作区未提交时
    rename 不可撤销**，故预演入口（G2）是硬需求而非锦上添花；
  - `done` 写审计 `DONE`，`archive` 写 `ARCHIVE`，均进本机日志 + 入库台账
    （`:1029-1032`、`:1208-1209`），且都带 `actor` 与 `channel`——
    事后可区分是谁、从哪个渠道归档的。
- **一致性风险**：界面归档后清单从左栏消失，而 `git status` 里该文件是 rename
  未提交状态。若用户此时不提交就切换项目，界面与磁盘会不一致——
  但这是 3 秒轮询（`maybe_refresh`）的固有行为，非本需求引入。

### 风险点与回滚方案

- **风险 1（高）**：给界面开了免票据的 `done` 通道，等于给「归档掉带阻塞评论的
  需求」降低了门槛——而 `requirement.rs:985-990` 明确说这是历史缺陷的一半。
  缓解：G4 的阻塞评论勾选门禁**必须**实现且必须有单测（T10）；
  若审核人不接受「界面上 done 免票据」，替代方案是界面 `done` 也要求用户
  先在 CLI `token issue` 换票——但那会让 GUI 失去「一处完成全流程」的价值，
  故本方案选择「加强确认」而非「提高门槛」。**这是需要审核人拍板的取舍**，
  已在 G4 与此处写明。
- **风险 2（中）**：scope 传错字面量导致 L3 下 100% 失败（`token.rs:179` 精确相等）。
  缓解：T9 单测锁 scope 形状（对 `done:{id}` 成功、对 `"done"` 失败）；
  且失败信息是「缺凭据」类，界面应把该文案原样显示而非包装成「操作失败」。
- **风险 3（中）**：`archive_due` 的部分失败在返回值里看不到原因（见关键设计 6），
  界面若显示「已归档 3 条」会让用户以为全搬完了。
  缓解：界面文案按诚实边界写（成功 N 条 + 复核提示），并列入验收 AC-011。
- **风险 4（低）**：归档区视图把 O(文件数) 读盘放进轮询。
  缓解：设计为打开时算一次并缓存，验收 AC-012 用计数断言「切换视图 N 次，
  读盘调用次数为 1」。
- **风险 5（低）**：`resolve_identity` 下沉时手误改动错误文案，导致 CLI 输出漂移。
  缓解：T2 的「下沉前后 CLI 输出逐字相同」单测；搬迁时用 `git mv` 保留历史。
- **回滚**：`git revert` 即可，无数据迁移、无清单格式变更、无配置变更。
  但**注意**：回滚代码不会撤销已经被界面归档的需求——那需要人工把文件搬回来
  （故 G4 的确认与预演是防止不可逆操作，而非防止代码回滚）。

## 3. 测试计划


### 单元测试用例

| 编号 | 用例 | 断言点 |
| --- | --- | --- |
| U1 | `core`：新增的审核人解析函数在四级输入下的行为 | 显式参数优先；参数为空时取环境变量；环境变量为空时取 git 姓名；三者皆无时返回 `Err` 且文案含 `git config user.name` |
| U2 | `core`：环境变量与 git 姓名都存在时 | 取环境变量（优先级在 git 之上，与 CLI 现状一致） |
| U3 | `core`：函数被下沉后 CLI 的错误文案 | 与下沉前逐字相同（用 `include_str!` 固定期望片段做回归护栏） |
| U4 | `core`：`done` 在 L3 + scope 为 `done:{id}` 的凭据下 | 返回 `Ok`，且清单 `GATE:HEAD` 的 `status` 变为 `done` 并写入 `done=` 时间戳 |
| U5 | `core`：`done` 在 L3 + scope 为常量 `"done"` 的凭据下 | 返回 `Err`，错误文案含「一次性范围票据」（证明 scope 精确相等，锁关键设计 1） |
| U6 | `core`：`archive_one` 在 L3 + scope 为 `"archive"` 的凭据下 | `Ok`；scope 为 `archive:{id}` 时 `Err`（证明两处 scope 形状确实不同） |
| U7 | `core`：`archive_one` 对未 done 的需求 | `Err`，文案含「尚未 done」；对已 done 的需求 `Ok` |
| U8 | `core`：`archive_due(dry_run=true)` | 不写任何 `ARCHIVE` 审计行、文件仍在原路径 |
| U9 | GUI：`DoneConfirm` 弹窗在无阻塞评论时 | 确认按钮可用；事实 ① 的「N/3 已通过」与实际 `approved_count()` 一致 |
| U10 | GUI：`DoneConfirm` 在有 ≥1 条未关闭阻塞评论时 | 确认按钮 disabled；勾选后启用；不勾选则无法完成动作 |
| U11 | GUI：`DoneConfirm` 三项事实齐备 | 文本同时含「已通过」计数、「阻塞」字样与「门禁不再管辖」后果陈述 |
| U12 | GUI：预演归档后 | `src → dst` 列表展示正确，磁盘文件位置不变，台账无新增 `ARCHIVE` 行 |
| U13 | TUI：与 U9–U12 对应的四组行为 | 同上（`TestBackend` 断言文本可见） |
| U14 | GUI：归档区视图下列出的每份需求 | 批准 / 打回 / 修订 / 封存 / 归档 五个按钮全部 disabled |
| U15 | TUI：归档区视图下按 `a`（批准） | 不触发任何 core 写调用（用计数器或 mock 断言写操作未发生） |
| U16 | 界面：审批类输入框初始值 | 等于 `identity::resolve_claimed(root, None, …)` 的结果（不硬编码任何姓名） |
| U17 | 界面：输入框清空后提交 | 报错（不得静默采用预填值），锁关键设计 5 的第二处 ⚠️ |
| U18 | 界面：`done` 成功后左栏 | 该需求已不在 live 列表；选中项切到相邻项；消息区含「门禁不再管辖」 |
| U19 | 既有 core / cli / tui / gui 全部单测 | 全绿，条数不减 |
| U20 | 判决性实验：把界面 `done` 的 scope 改成常量 `"done"` | U4 对应的界面用例必须 FAIL（即界面确实传的是 `done:{id}`） |
| U21 | 判决性实验：删掉阻塞评论的勾选门禁 | U10 必须 FAIL |

### 端到端用例

| 编号 | 步骤 | 期望 |
| --- | --- | --- |
| E1 | 起 GUI，选中一份三段全批、无阻塞评论的需求，点「归档（done）」 | 确认框显示「3/3 已通过」+ 后果陈述；确认后左栏少一份，消息区说明已归档 |
| E2 | 同上，但该需求有 2 条未关闭阻塞评论 | 确认框显示阻塞数 2 且必须勾选才能确认；勾选后归档成功 |
| E3 | 起 TUI，选中需求按 `d`，走完确认 | 与 E1/E2 一致，footer 文案完整可见 |
| E4 | 点「预演归档」选一份已 done 满 60 天的需求 | 显示 `源路径 → archive/2026/…`，磁盘不变，台账无新增 |
| E5 | 点「清理到期归档」（`archive.after_days` = 30） | 搬走 1 条，消息显示「已归档 1 条」；重复点击第二次显示 0 条（幂等） |
| E6 | 在归档区视图里选一份 2025 年的需求 | 标题标「只读历史」；五个写按钮全灰；正文可读可复制 |
| E7 | 顶栏查看身份区 | 显示姓名、邮箱、sig 与鉴权等级；把 `git config user.name` 改掉后重启界面，预填值随之变化 |
| E8 | 归档一份需求后 `git status` | 该清单显示为 rename（正文 + 评论成对），`git log` 历史不断 |

### 边界 / 异常场景

- **B1** 无未归档需求（全部 done）：live 列表为空，界面给出明确空态文案，
  不显示空的确认按钮。
- **B2** 对已 done 的需求再点归档：core 返回幂等 `Ok`
  （`requirement.rs:1006-1008` 的幂等短路），界面显示「已归档」而非报错，
  也不重复写审计（`requirement.rs:1013-1015`）。
- **B3** 对归档区里的需求点归档：`archive_one` 在 `is_archived_path` 处返回
  `Err`（「已在归档区」，`requirement.rs:1125-1130`）；界面应**提前禁用**该按钮，
  万一点到则显示该错误而非崩溃。
- **B4** 身份取不到（无 git、无环境变量）：顶栏显示 `describe` 的
  「未取到 git 身份」形态，并给出 `identity.rs:294` 的三条取解；
  此时 L0 下仍可审批（`auth.rs:265-270`），界面**不得**因此禁用审批按钮。
- **B5** 预填了姓名但用户改成了别名（与 git 不一致）：L1+ 下 `identity::decide`
  拒绝，界面原样显示「审批人身份冲突」及三条取解（`identity.rs:243-250`）。
- **B6** `archive_due` 逐条部分失败：界面显示「已归档 N 条」+ 复核提示，
  **不得**显示「全部成功」（诚实边界，见关键设计 6）。
- **B7** `archive.after_days: 0`（配置为 done 即归档）：界面必须显示「0 天（立即归档）」，
  不隐藏这个值。
- **B8** 归档区有 200 份需求：打开视图只读盘一次（U-AC 断言计数），
  切换选择项不重复读盘；不卡死 3 秒轮询。
- **B9** 归档目标目录已存在同名文件：`move_archived` 幂等返回 `Ok(None)`
  （`requirement.rs:1165-1167`），界面显示「未变化」而非报错。
- **B10** 凭据在动作执行前过期（TTL 10 分钟，GUI 内签发）：
  core 返回「缺凭据」类错误，界面原样显示，不包装成「操作失败」。

### 回归范围与影响面

- `cargo test -p req-guard-core` 全量：新增 U1–U8，既有测试条数不减。
  ⚠️ 唯一的行为改动面是新增符号，`done` / `archive_*` / `identity::decide`
  的既有测试**必须一条不改就通过**——若需要改期望值，说明「纯搬迁」被做成了行为变更。
- `cargo test -p req-guard` （CLI）：U3 要求输出逐字不变。
- `cargo test -p req-guard-tui` / `cargo test -p req-guard-gui`：
  新增 U9–U18，条数较改前至少 +10；既有渲染冒烟与 markdown 测试全绿。
- `cargo fmt -p req-guard-core -p req-guard -p req-guard-tui -p req-guard-gui -- --check` 零 diff。
- `cargo clippy` 对四个 crate 均零 warning。
- `cargo build`（default features）依赖图不变（本需求不加依赖）。
- 手工对照 README 的界面操作说明与实际按键/按钮一一对得上（无「文档里有、界面上没有」的入口）。

### 验收门槛

- `cargo fmt --check` 与 `cargo clippy --all-targets`（四个 crate）零输出。
- 四个 crate 的 `cargo test` 全部失败数为 0。
- `cargo test -p req-guard-core` 与 `-p req-guard` 的测试总数不低于改前
  （新增单测只增不减，且既有测试零期望值改动）。
- `cargo test -p req-guard-tui` 与 `-p req-guard-gui` 测试总数 ≥ 改前 + 10。
- `cargo tree` 四个 crate 的依赖集合与改前一致（不新增依赖）。
- **判决性实验**：把界面 `done` 的 scope 改成常量、把阻塞评论勾选门禁删掉，
  对应单测必须 FAIL（自证的测试不算判决）。
- 审核人确认：接受「界面 `done` 免票据 + 加强确认」这一取舍（风险 1），
  或改为要求先换票（则 T3/T7/T9/T10 作废，另立需求）。

<!-- GATE:AC -->
### AC-001
- Given: 一个已批三段的需求清单，且审批严格等级为 3、进程内持有范围为 `done:REQ-001` 的票据
- When: 调用归档该需求的接口
- Then: 返回成功，且清单标记行的状态值为 `done` 并带一个非空的归档时间戳

### AC-002
- Given: 一个已批三段的需求清单，且审批严格等级为 3、进程内持有范围为 `done` 的票据
- When: 调用归档该需求的接口
- Then: 返回错误，且错误消息包含字面量 `一次性范围票据`

### AC-003
- Given: 一个已批三段且状态为 `done` 的需求清单，且进程内持有范围为 `archive` 的票据
- When: 调用单份物理归档接口，分别以预演标志为真与为假各调用一次
- Then: 预演调用后该清单源路径的存在性为 `true`，真实调用后该存在性为 `false`

### AC-004
- Given: 一个状态为 `in_review` 的需求清单，且进程内持有范围为 `archive` 的票据
- When: 调用单份物理归档接口
- Then: 返回错误，且错误消息包含字面量 `尚未 done`

### AC-005
- Given: 进程内无任何票据、未设置审核人环境变量、且所在仓库的 git 姓名为空
- When: 调用新增的审核人解析函数且不传入显式参数
- Then: 返回错误，且错误消息包含字面量 `git config user.name`

### AC-006
- Given: 仓库的 git 姓名为 `Mike Zhu`、未设置审核人环境变量
- When: 调用新增的审核人解析函数且不传入显式参数
- Then: 返回的字符串逐字等于 `Mike Zhu`

### AC-007
- Given: 仓库的 git 姓名为 `Mike Zhu`、审核人环境变量为 `代审人`
- When: 调用新增的审核人解析函数且不传入显式参数
- Then: 返回的字符串逐字等于 `代审人`，即环境变量优先于 git 身份

### AC-008
- Given: 实现完成后的 cli crate，其身份解析逻辑已改为转调 core
- When: 在三级回退全部落空的情形下执行需要审核人的子命令
- Then: 输出的错误文案与随改动提交的基准文件 `cli_identity_error.txt` 逐字相同

### AC-009
- Given: 一份状态为 `in_review` 且带有 1 条未关闭阻塞评论的需求清单
- When: 在 gui crate 的管理台中打开归档确认弹窗
- Then: 勾选前确认按钮的可用性为 `false`，勾选后该可用性为 `true`

### AC-010
- Given: 一份三段全部已批准且无任何未关闭评论的需求清单
- When: 在 gui crate 的管理台中打开归档确认弹窗
- Then: 弹窗文本同时含字面量 `3/3`、`阻塞` 与 `门禁不再管辖` 三处片段

### AC-011
- Given: 一份带 2 条未关闭阻塞评论的需求清单，且归档到期天数为 30
- When: 在 gui crate 的管理台中触发到期归档的预演动作
- Then: 提示文本包含字面量 `30`，且台账中 `ARCHIVE` 开头的行数不增加

### AC-012
- Given: 归档区下存在 3 份需求清单的仓库，且 gui crate 的管理台已切换到归档区视图
- When: 依次选中 3 份需求并读取每份的写动作按钮可用状态
- Then: 3 份需求的批准、打回、修订、绑定摘要与归档这 5 个按钮全部为禁用状态

### AC-013
- Given: 仓库的 git 姓名为 `Mike Zhu`，且 gui crate 的管理台处于新建需求的对话框
- When: 读取该对话框审核人输入框的初始文本
- Then: 该文本逐字等于 `Mike Zhu`

### AC-014
- Given: 实现完成后的 gui crate，其审核人输入框已被用户清空
- When: 提交该审批动作
- Then: 返回错误，且该清单 3 段的状态值全部保持不变，即未静默采用预填值

### AC-015
- Given: 一份三段全部已批准的需求清单，且其归档确认动作已成功执行
- When: 重新读取该仓库的未归档需求清单列表
- Then: 该清单不在返回的列表中，且列表长度比归档前减少 1

### AC-016
- Given: 实现完成后的 tui crate，其审核人输入弹窗已打开
- When: 把该仓库的 git 姓名改为 `另一个人` 并重新触发审核人解析
- Then: 返回的字符串逐字等于 `另一个人`，即预填值取自实时身份而非硬编码

### AC-017
- Given: 实现完成后的 core、cli、tui、gui 四个 crate
- When: 分别执行各自的 `cargo test`
- Then: 四个 crate 的失败数均为 0

### AC-018
- Given: 实现完成后的 core、cli、tui、gui 四个 crate
- When: 执行 `cargo fmt` 检查与 `cargo clippy --all-targets`
- Then: 前者无 diff 输出，后者 warning 数为 0

### AC-019
- Given: 实现完成后的 workspace，其中 core、cli、tui、gui 四个 crate 的依赖声明
- When: 对四个 crate 分别执行 `cargo tree` 并与改动前记录比对
- Then: 4 个 crate 的依赖集合与改动前逐项相同，即本次改动未新增任何依赖

### AC-020
- Given: 实现完成后的两个界面的归档动作，其传入的凭据范围参数
- When: 把该参数改为常量 `done` 后运行断言凭据范围形状的单测
- Then: 恰好有 1 条单测失败，即该范围参数不是自证的

### AC-021
- Given: 实现完成后的两个界面的归档确认弹窗
- When: 删掉阻塞评论的勾选门禁逻辑后运行相关单测
- Then: 恰好有 2 条单测失败，即该门禁逻辑不是自证的

### AC-022
- Given: 一份状态为 `in_review` 的需求清单，且进程内无票据
- When: 调用归档该需求的接口
- Then: 返回错误，且该清单的标记行状态值仍为 `in_review`，即未发生任何写入
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-05_14:20:44 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-05_14:21:27 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-05_14:21:39 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
- 2026-10-09_12:57:53 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | 档位升至 critical（§8.5 方案 2）
<!-- /GATE:AUDIT -->
