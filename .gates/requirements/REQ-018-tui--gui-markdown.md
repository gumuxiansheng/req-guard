---
doc_type: proposal
tier: standard
owner: -
review_policy: codebound
verified_at: 2026-10-05
source_refs: [Cargo.toml, Cargo.lock, mdast, mdast/src, gui, gui/src, tui, tui/src, docs/设计]
---

# REQ-018 TUI 补齐 GUI 已有能力：Markdown 渲染视图 / 切换项目根 / 批准后自动跳段 / 越序拒绝 / 色板对齐

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-018 status=approved created=2026-10-05_13:47:48 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:23:30 sum=58039eae798224085c02a2b16a4dd0477b5683b89a7ab7d15f75a7f83eca1b07 -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:23:37 sum=946c75566aada03b3a365fe75eeaa48e0568482ef10f891877bfe41f0a6345f4 -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:23:45 sum=fb9542efef5cf01d2df7610d972bc0c0e0a86aaf9c8c7666148f5d43bbd7241f -->

## 1. 需求分解

### 背景与问题

GUI 是功能更全的一端，TUI 是**同等写权限、更少呈现**的一端。五处差距，
按对审核效率的影响排序。逐条查证（均为代码走读，非推测）：

1. **TUI 的正文是裸 Markdown 源，且把 `GATE` 标记行直接糊在审核人脸上**。
   `render_body`（`tui/src/ui.rs:227-228`）把 `app.body` 逐行塞进 `Paragraph`：
   ```rust
   let lines: Vec<Line> = app.body.iter().map(|l| Line::from(l.clone())).collect();
   ```
   `app.body` 是 `requirement::section_of`（`tui/src/app.rs` 的 `reload`）的切片，
   即**含 `<!-- GATE:HEAD … -->` / `<!-- GATE:STEP … -->` 的原始 Markdown**。
   后果有两条：
   - 审核人看到的是 `- [ ]` 而不是渲染后的任务列表、
     `**粗**` 与 `` `码` `` 的字面反引号、裸表格竖线与分隔行；
   - **门禁内部标记直接可见**。GUI 侧的 Markdown 渲染器刻意丢弃
     `Event::Html` / `HtmlBlock`（`gui/src/markdown.rs:28-31` 的安全纪律 2：
     「模板里的 `<!-- GATE:… -->` 是给门禁工具看的标记行，不该糊在正文里」），
     TUI 没有这层，**两个界面对同一份内容的呈现不一致**。
   GUI 侧的 Markdown 渲染已在 REQ-011 / REQ-013 两轮迭代后成熟
   （`parse` 为 egui-free 纯函数、`show` 为渲染层、`Cache` 为缓存）。

2. **TUI 无法切换项目根目录**。
   GUI 顶栏有「切换目录」（`gui/src/app.rs:594` 的 `pick_root`，走 `rfd` 文件夹选择器）。
   TUI 的 `root` 由 `req-guard ui --tui` 启动时传入
   （`tui/src/lib.rs` 的 `run(root)`），**进程内不可变**。
   后果：在多仓工作的人必须退出界面、cd、再重开。

3. **TUI 批准后光标停在已通过的旧段**。
   GUI 的 `approve` 成功后调 `jump_to_reviewable`（`gui/src/app.rs:218` → `:242`），
   把段光标移到第一个未通过的段，注释写明原因：
   > 自动前进到第一个未通过的段：批准完阶段 1 后界面应立即可审阶段 2，
   > 否则光标停在已通过的旧段，后续段永远无法进入审核。

   TUI 的 `review`（`tui/src/app.rs:875`）成功后**只刷新 + 写消息**，不动 `self.step`。
   实测后果：审完第一段后按 `a`，它会对**同一段**再次发起批准，
   而该段已 approved —— 用户会以为自己按错了。

4. **TUI 对越序审核只给一行临时提示**。
   `status::ReqStatus::can_review`（`status.rs:169`）是 core 已有的判定
   （强制顺序下前置段未通过则不可审）。
   GUI 用 `add_enabled` 把按钮**置灰 + tooltip**（`gui/src/app.rs:897-904`），
   TUI 只在 `begin_approve` 里弹一行 footer 提示
   （`tui/src/app.rs:382-388` 附近的顺序提示），**提示会被下一条消息覆盖**，
   且三段列表本身不标出哪段可审——用户只能靠「试一次」。

5. **TUI 的着色是裸 `ratatui::Color`，与 GUI 的 WCAG AA 色板不同口径**。
   GUI 有专门的 `palette.rs`（251 行，`Tone` 语义色 + 浅/深底两套实测对比度，
   单测锁住）。TUI 直接用 `Color::Red` / `Color::Green` / `Color::Yellow` /
   `Color::Cyan`（`ui.rs` 全文）。后果：
   - 两个界面**视觉语言不一致**：GUI 的 Danger 与 TUI 的 `Color::Red`
     未必是同一个色；
   - TUI 的颜色**未经对比度验证**——终端主题多样，裸色在某些主题下对比度不足。
   ⚠️ 注意口径差异：`Tone` 是**为 egui 画的**（有 `light()` / `dark()` 两套，
   `palette.rs:76-118`），ratatui 的 `Color` 是**终端 ANSI 色**，
   由终端主题决定实际 RGB。所以本需求**不可能也不应该**让 TUI「用同一个 `Tone`」——
     可对齐的是**语义映射关系**（Danger→红系、Warning→黄系、Muted→灰系），
     而非具体色值。这是必须先说清的前提，否则会写出做不到的目标。

6. **附带两处小差距**（一并纳入，成本极低）：
   - 内容冻结徽标只有标记没有提示：GUI 的 `seal_badge`（`gui/src/app.rs:952`）
     配 `on_hover_text` 说明每个徽标意味着什么；TUI 的
     `seal_badge`（`ui.rs:49-56`）只有 `[未绑定]` / `[已改动]` / `[无法校验]`。
   - 需求列表的 `●` / `○` 只区分「被卡住 / 未被卡住」（`ui.rs:117`），
     不区分需求级状态；GUI 的左栏还有颜色区分。

### 目标

- **G1 TUI 正文渲染 Markdown**：TUI 正文区默认渲染 Markdown
  （标题 / 段落 / 列表 / 任务列表 / 围栏代码块 / 表格 / 引用 / 分隔线
  + 行内 `**粗**` `*斜*` `~~删~~` `` `码` ``），并保留一个**原文视图**可切换
  （对齐 GUI 的「正文: 渲染 / 原文」toggle，`gui/src/app.rs:641`）。
  **两条安全纪律必须在 TUI 侧同样成立**：① 链接渲染成带下划线纯文本、
  **永不激活**；② HTML 注释（含 `GATE:…` 标记行）**整块隐藏**。
  理由不是「照抄 GUI」，而是：清单正文由 AI 生成，
  一个能点开的 `[点我](https://evil.tld)` 等于给 AI 留了一条骗人类点击的通道。
- **G2 TUI 可切换项目根**：提供路径输入（终端里没有文件夹选择器，
  用文本输入 + 校验），切换后重新载入全部数据。
  校验要求：路径必须存在、必须是目录、且**含 `.gates/requirements/`**
  ——否则明确拒绝（这与 `install` 的存在性检查同款思路）。
- **G3 TUI 批准后自动跳段**：批准成功后把段光标移到第一个未通过的段
  （`ReqStatus::next_step`，`status.rs:161`），与 GUI 的
  `jump_to_reviewable`（`gui/src/app.rs:242`）**同一判定**。
  全通过时保持不动。
- **G4 TUI 三段列表标出可审性**：用 `ReqStatus::can_review`（`status.rs:169`）
  在三段列表上标注「可审 / 等待前置段」，且对不可审的段发起审批时
  **明确拒绝并说明是哪一段挡着**（用 `next_step()` 给出段名），
  而不是让 core 抛错后原样显示。
  ⚠️ 注意：`can_review` 返回的是「该段是否允许被审核」，
  而**已 approved 的段也应显示为不可重复审批**——
  现状 `can_review`（`status.rs:169-179`）对已 approved 的段返回 `true`
  （它只检查前置段，不检查自己是否已通过）。故界面上要显示的状态是
  `can_review(step) && !state.is_approved()`，**两个条件都要**。
  这是 core 的 API 与界面语义之间的一处真实落差，必须在方案里写明处理方式，
  不许悄悄只用一个条件。
- **G5 色板语义对齐**：TUI 建立自己的 `Tone → ratatui::Color` 映射表
  （**新文件** `tui/src/palette.rs`），语义档位与 GUI 的 `Tone` 一一对应
  （`Danger` / `Warning` / `Success` / `Muted` / `Accent`），
  并在模块注释里写明「色值不可跨端对齐，语义可对齐」这条口径（背景第 5 条）。
- **G6 冻结徽标补提示文案**：TUI 的冻结徽标在帮助浮层里加一段
  「徽标含义」对照（`Frozen` 不显示 / `NotSealed` / `Changed` / `Unverifiable`
  各代表什么），弥补无 hover 的差距。
- **G7 需求列表区分需求级状态**：左栏在 `●` / `○` 之外，
  给 `changes_requested`（待修改）与 `draft`（未开始）加不同的标记
  （数据来自 `ReqStatus::state`，`status.rs:86-95`），
  让用户不必点进去才知道这份清单待改。

### 非目标

- **N1** 不让 TUI 正文**可编辑**。两个界面的正文都是只读
  （GUI `gui/src/app.rs:48` 的注释、TUI `ui.rs:229` 的「正文（只读）」），
  这是 `docs/设计/UI架构细化方案.md` 的「需要你确认的细节」一节第 2 问的既有决定
  （正文交给 AI / 编辑器）。⚠️ 该节标题是 `## 9. …` 形态，按 REQ-004 的引用判定
  （只认 `§N.M`，`core/src/touch.rs:265-283`）无法被机器校验，故此处刻意不写 `§` 号。
  本需求不动这个决定，只改**呈现**。
- **N2** 不把 `gui/src/markdown.rs` 整个搬进 tui crate（复制一份）。
  见关键设计 1：要么下沉成共享小 crate，要么各写各的——本需求选前者，
  理由写在那里。
- **N3** 不在 TUI 里加语法高亮、不加图片、不加外链打开（与 GUI 的
  N1/N4 取舍一致，`docs/设计/UI架构细化方案.md §4.4`「正文渲染器宁少勿全」）。
- **N4** 不改 `core` 的任何 API。`next_step` / `can_review` / `seal_state`
  已 `pub` 且够用；G4 提到的「`can_review` 不看自己是否已批」是**界面组合两个条件**
  即可解决，不需要改 core（N5）。
- **N5** 不改 `ReqStatus::can_review` 的语义。
  它叫 `can_review`（能不能进入审核），不是 `should_review`（现在该不该审）。
  改它会波及 core 既有语义与测试；界面组合 `can_review && !is_approved()`
  是正确做法，且**必须在界面上显示两者的区别**（否则用户会问「为什么这段不可审」）。
- **N6** 不做「多项目并行管理台」（同时盯多个仓）。切根是「一次只看一个」。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | 决策并落地：把 `Block` / `Inline` / `parse` / `Cache` 下沉为共享 crate `mdast`（见关键设计 1），`gui` 改为依赖它 | 2.5h |
| T2 | `mdast` crate 的单测迁移（从 gui 移过去，条数不减）+ `escape_table_code_pipes` 三条竖线用例继续成立 | 1.5h |
| T3 | `tui/src/palette.rs`：`Tone` → `ratatui::Color` 映射表 + 语义档位与 GUI 一一对应的单测 | 1h |
| T4 | TUI 渲染层：`Block` → `Vec<Line>` 转换（标题分级样式 / 列表缩进 / 任务列表勾选态 / 表格 / 围栏代码块） | 3h |
| T5 | TUI 正文区加「渲染 / 原文」切换（复用 GUI 的 toggle 形态），`GATE` 标记在渲染视图下不可见 | 1h |
| T6 | TUI 链接纪律：渲染成带下划线纯文本，**不可激活**（无 OSC 8 / 无鼠标捕获） | 0.5h |
| T7 | TUI 切换项目根：路径输入弹窗 + 三项校验（存在 / 是目录 / 含 `.gates/requirements/`）+ 成功后全量重载 | 1.5h |
| T8 | TUI 批准后跳段（复用 `next_step`），全通过时不动 | 0.5h |
| T9 | TUI 三段列表标可审性 + 不可审时给出「哪一段挡着」的具体段名 | 1h |
| T10 | TUI 冻结徽标对照文案进帮助浮层（G6） | 0.3h |
| T11 | TUI 左栏需求级状态标记（G7） | 0.5h |
| T12 | 单测：`Block` → `Line` 的几何/结构断言（含 GATE 隐藏、链接不激活、竖线三例） | 2h |
| T13 | 单测：根目录切换的三项校验（不存在 / 不是目录 / 无 `.gates/requirements/`） | 1h |
| T14 | 单测：跳段与可审性组合（`can_review && !is_approved()`）的 6 种段状态组合 | 1h |
| T15 | 判决性实验：去掉「隐藏 GATE 标记」这一步，对应单测必须 FAIL | 0.3h |
| T16 | 文档：`docs/设计/UI架构细化方案.md §4.4` 补「TUI 正文渲染」与 `mdast` 下沉的取舍记录 | 0.7h |

合计 17.3h。**T1/T2 是共享层前置**，必须先于 T4–T6；T3 独立可先行；
T7–T11 为纯 TUI 增量，与正文渲染无耦合，可分别交付。

### 影响范围

- **模块**：新增 `mdast/`（第四个 workspace 成员）、
  `tui/src/{palette.rs（新增）, ui.rs, app.rs}`、`gui/src/markdown.rs`（改为转调 `mdast`）、
  `gui/Cargo.toml` + `tui/Cargo.toml` + `Cargo.lock`、`Cargo.toml`（workspace members）、
  `docs/设计/UI架构细化方案.md`。
- **接口**：
  - **不改 `core` 任何 API**（N4/N5）；
  - 新增 crate `mdast`：`pub` 导出 `Block` / `Inline` / `Align` / `List` / `ListItem` /
    `Table` / `parse` / `Cache` / `inline_text`，**签名与现有 `gui::markdown` 逐字相同**，
    使 `gui/src/markdown.rs` 的 `show()` 与 `gui/src/app.rs:873` 的调用点**零改动**；
  - 新增 `tui::palette::tone_of(Tone) -> Color`（`Tone` 从 `req-guard-gui` 引入会
    造成 tui→gui 的依赖，**不可接受**，故见关键设计 1 的说明）。
- **配置 / 清单格式 / 数据表 / 外部 API**：无。
- **向后兼容**：
  - `cargo build`（default features = `core` + `cli`）**依赖图仍无 `mdast`**
    ——`mdast` 只在 `--features gui` / `--features tui` / `--features full` 下进入。
    这与 `docs/设计/UI架构细化方案.md §1.1`「default 构建零 UI 依赖」的铁律一致，
    **必须验收**；
  - GUI 侧行为**逐字不变**（`show()` 签名与渲染逻辑不动，只换 AST 来源）；
    `gui/src/markdown.rs` 里 `show()` 之外的代码不动，
    `pulldown-cmark` 依赖从 `gui` 移到 `mdast`；
  - TUI 正文默认切到**渲染视图**——这是一次**可见的行为变更**。
    缓解：保留原文视图一键切换；帮助浮层与 README 都写明切换键。
    如实记录：这是本需求唯一影响老用户的变更。
- **CI**：`gui` 已有平台排除（`gui/Cargo.toml` 注释里的
  `default-features = false` 与 `compile_error!` 约束）。
  新增 `mdast` 是纯 Rust + `pulldown-cmark`，**无平台依赖**，
  故 `ubuntu-latest` 的 `--workspace` lint / test 可以包含它
  （比 `gui` 更宽松，不是更严）。

### 验收标准

见第 3 段 `GATE:AC` 块（编号连续、Given/When/Then 齐全且可度量）。

## 2. 技术方案


### 总体思路

**正文渲染的 AST 与解析器下沉到一个新的零平台依赖小 crate，GUI 与 TUI 共用一份；
ratatui 侧新写一层 `Block → Line` 渲染；其余四点是纯 TUI 增量。**
不改 core 一个字，不改 GUI 的渲染逻辑与调用点。

### 关键设计

1. **为什么下沉成 `mdast` 而不是复制或各写各的**（G1 的落点，N2 的取舍）：
   现状 `gui/src/markdown.rs` 的形态已经很适合复用：
   - `parse(src) -> Vec<Block>` 是 **egui-free 纯函数**（模块文档第 34-36 行明说
     「解析（[`parse`]，纯函数、不碰 egui、可单测）与渲染（[`show`]）分离」）；
   - `Block` / `Inline` / `Align` / `List` / `ListItem` / `Table` 是纯数据类型；
   - `Cache`（`:820`）也只依赖 `(usize, String, Vec<Block>)`；
   - 只有 `show(ui, &mut egui::Ui, &[Block])` 依赖 egui。
   所以边界本来就是干净的，只是**物理上住在 gui crate 里**，而 tui crate 无法引用
   （引用它就等于 `req-guard-tui → req-guard-gui → eframe`，
   于是 TUI 会被拖进 18–64MB 的 GUI 依赖树，`--features tui` 的轻量构建当场失效）。

   三个选项对比：
   | 选项 | 结论 |
   | --- | --- |
   | A. TUI 直接依赖 `req-guard-gui` | ❌ 拖入 eframe，破坏 feature 门控的初衷 |
   | B. 把 `markdown.rs` 复制进 tui | ❌ 两份解析器必然漂移。REQ-013 的全部教训（竖线三例、竖线归一化、HTML 隐藏）都要维护两遍 |
   | C. 下沉为 `mdast` crate | ✅ 唯一依赖 `pulldown-cmark`，无平台依赖，两端共用 |

   选 C。代价是 workspace 多一个成员（4 个），且要改 `Cargo.toml` 的
   `members` 与 `default-members`（`mdast` **不进** `default-members`，
   否则 default 构建会拉进 `pulldown-cmark`，破坏「零 UI 依赖」）。
   归属说明：`.gitattributes` / CI 脚本里若有 crate 白名单需同步检查。

   ⚠️ `Tone` 的处理与 `Block` 不同：`Tone`（`gui/src/palette.rs`）
   是 **egui 专属的**（返回 `egui::Color32`，`palette.rs` 里有 `light()` / `dark()`）。
   所以 `tui` **不能**引用 `req-guard-gui::palette::Tone`（同上，拖入 eframe）。
   故 G5 的「语义档位一一对应」是**人工维护的对照表**，
   由单测锁住「档位名集合相同」而非「色值相同」——见关键设计 3。

2. **`Block → Line` 的渲染映射要点**（T4）：
   ratatui 没有 markdown 渲染器，`ratatui-markdown` 不引
   （理由同 GUI 的 N2 取舍：宁少勿全、且要自己控安全纪律）。
   映射表：
   - 标题 → 加粗 + 按层级缩进（`Heading(l)` 的 `l` 决定缩进与是否加粗）；
   - 段落 → 按版心宽度折行的 `Line`（ratatui `Wrap { trim: false }`，
     与 GUI 的「版心唯一」纪律同向）；
   - 列表 → 符号 + **按层级递增缩进**（照 GUI 的 `LIST_INDENT_W` 逐层加，
     `markdown.rs` 的 `list_block` 已有该纪律，TUI 侧必须同样逐层递增——
     这是 REQ-013 记录的真实缺陷，不可重犯）；
   - 任务列表 → 勾选态用 `☐` / `☑`，且**只读**（不响应任何按键）；
   - 表格 → 用空格对齐的等宽表（ratatui 无 `Grid`）；列宽按最长单元格分配，
     超宽时截断并加 `…`（**不横向滚动**——终端里横向滚动体验极差）；
   - 围栏代码块 → 保留缩进与原始字符，`monospace` 不可得则用普通样式但**不折行**
     （`Wrap { trim: false }` 关闭）——代码折行会误导读者；
   - 分隔线 → 一行 `─`。
   渲染范围**与 GUI 完全一致**（同一个 AST 决定了这一点，这正是下沉的价值）。

3. **色板对齐只能对齐语义，不能对齐色值**（G5 的落点）：
   `palette::Tone` 的色值是 `egui::Color32`（明确的 sRGB 值，
   且 `palette.rs:76-118` 有浅/深底两套实测对比度）；
   ratatui 的 `Color` 是**终端 ANSI 色索引**，实际 RGB **由终端主题决定**——
   同一份 xterm 配置下 `Color::Red` 在不同主题里是不同的红。
   所以：
   - 可对齐：语义档位集合（`Danger` / `Warning` / `Success` / `Muted` / `Accent`）
     与「什么情况用哪档」；
   - 不可对齐：具体 RGB 值。TUI 想控制对比度只能靠**终端主题**，
     程序侧做不到——这一点必须写进 `tui/src/palette.rs` 的模块注释，
     否则下一个修改者会误以为可以「把 TUI 的红调到和 GUI 一样」。
   单测的形态因此是：断言**两端的档位名集合相同**（`mdast` 或一个
   极小的共享枚举，见下），而不是断言色值。
   ⚠️ 档位名集合要不要也下沉到 `mdast`？**不要**——`mdast` 的职责是正文 AST，
     塞一个色板枚举进去是职责混淆。折中：两个 crate 各自定义档位，
     由**本文档 + 一条单测（列出 gui 的 Tone 变体名与 tui 的变体名，断言集合相等）**
     锁住。这个「跨 crate 的字符串集合断言」是脆弱的，但比没有强；
     更好的做法留待将来把 `Tone` 也下沉（届时 TUI 与 GUI 共用同一个枚举，
     本条的脆弱性自动消失）。**这一点如实写明，不假装它是稳固的。**

4. **两条安全纪律在 TUI 侧的落法**（G1 的核心）：
   ① **链接不激活**：TUI 侧**根本不给链接任何交互**——
     不做 OSC 8 超链接（终端会把它变成可点链接）、不做鼠标捕获。
     渲染成 `Span` 带 `Modifier::UNDERLINED` 的纯文本。
     这比 GUI 还简单：GUI 要「画成下划线文本且不调 `hyperlink_to`」，
     TUI 只要不主动发转义序列就天然安全。
     单测断言渲染产出的 `Line` 里**不含任何含 `OSC`/`\x1b` 的 Span**。
   ② **HTML 注释隐藏**：由 `mdast::parse` 保证（丢弃 `Event::Html` /
     `InlineHtml` / `Tag::HtmlBlock`，`gui/src/markdown.rs` 已如此），
     TUI 只消费 AST，不接触原文，**天然继承**。
     ⚠️ 但**原文视图**会显示 `GATE` 标记行——这是用户主动切换的、要看源文本的视图，
     属预期（GUI 的原文视图同样显示）。单测要区分这两个视图：
     渲染视图不含 `GATE`，原文视图含。

5. **切换项目根的三项校验**（G2）：
   - 路径存在（`Path::exists`）；
   - 是目录（`is_dir`）；
   - 含 `.gates/requirements/`（`status::req_list` 对缺目录返回空集，
     `status.rs:194-196`——即「看起来像空仓库」而不报错。
     **必须显式校验**，否则用户敲错路径只会看到空列表，
     以为这个项目没有需求）。
   失败时**保留原 root**（不切到无效路径），消息给出「路径不存在 / 不是目录 /
   未找到 .gates/requirements/」三种**分别**的文案（修法不同，理由同
   `touch.rs:302-308` 对交叉引用的处理）。
   切换成功后：清空选中项与所有浮层，重新 `reload`。
   ⚠️ 切换 root **不清除界面凭据**——凭据是用户级（存在 `$HOME`，
     `token.rs:92` 的 `guard_dir`），切仓不改变它；但已打开的对话框应关闭，
   避免用新仓的需求 id 去消费旧仓签发的 scope 票据。

6. **跳段与可审性的判定必须组合两个条件**（G3/G4，N5 的落点，T14）：
   ```rust
   // 界面要显示的「现在该不该审这一段」
   let reviewable = r.can_review(step) && !st.state.is_approved();
   ```
   `can_review`（`status.rs:169-179`）只检查**前置段**是否通过，
   对一个已 approved 的段它返回 `true`；`is_approved` 是段自身状态。
   两者缺一都会出错：
   - 只用 `can_review` → 已通过的段仍显示「可审」，按 `a` 会重复批准；
   - 只用 `!is_approved` → 越序段显示「可审」，按 `a` 被 core 拒。
   跳段目标用 `next_step()`（`status.rs:161`，第一个非 approved 段；
   全通过返回 `None` → 保持不动，与 GUI 的 `jump_to_reviewable`
   在全通过时行为一致）。
   不可审时给出**具体阻塞段名**：`next_step()` 就是要审的那段，
   文案如「技术方案需先完成需求分解」（用 `step_label`，`requirement.rs:53`）。

7. **`GATE:TOUCH` 声明块的原样搬运**（`mdast` 迁移的一个坑）：
   `mdast` 里不含 `core`，所以它不知道什么是 `GATE:TOUCH`——
   但**也不需要知道**：它只是 `Block::HtmlBlock`（HTML 注释）的一种，
   会被丢弃。这正是我们要的。
   ⚠️ 反过来要确认：`escape_table_code_pipes`（表格竖线预归一化，
   `gui/src/markdown.rs` 的 REQ-013 关键设计 6）必须**一起下沉**，
   否则 TUI 侧会丢单元格——这是 REQ-013 实测过的失效形态
   （「直接喂原文会静默丢单元格」）。T2 专门锁这一条。

### 涉及的文件与模块清单

新 crate `mdast` 的内部切分：`lib.rs` 放**数据类型**（`Block` / `Inline` / `Align` /
`List` / `ListItem` / `Table` / `Cache`）与 `parse` / `inline_text` 的入口，
`parse.rs` 放**解析实现**（`pulldown-cmark` 事件映射 + `escape_table_code_pipes`
表格竖线预归一化）。**`parse.rs` 必须与预归一化器同文件**——
REQ-013 的关键设计 6 明确「换库后『不丢内容』这条从解析器转移到了预归一化器」，
把它拆到另一个文件就是为了让它可被单独绕过，不得给这种拆法开口子。

<!-- GATE:TOUCH -->
Cargo.toml
Cargo.lock
mdast/Cargo.toml
mdast/src/lib.rs
mdast/src/parse.rs
gui/Cargo.toml
gui/src/markdown.rs
tui/Cargo.toml
tui/src/lib.rs
tui/src/app.rs
tui/src/ui.rs
tui/src/palette.rs
docs/设计/UI架构细化方案.md
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

- **兼容性**：
  - **GUI 侧逐字不变**：`gui/src/app.rs:873` 的 `markdown::show(ui, &blocks)` 调用点
    不改；`show()` 的签名与内部渲染逻辑不改；`parse` 的签名不改。
    只是实现搬到了 `mdast`，`gui/src/markdown.rs` 变成「egui 渲染层 +
    对 `mdast` 的转调」；
  - `pulldown-cmark` 依赖从 `gui` **移到** `mdast`，`Cargo.lock` 随之变化，
    但传递依赖集合（`bitflags` / `memchr` / `unicase`）不变（REQ-013 AC-014 的结论
    在新位置继续成立，需在新 crate 上重跑一次）；
  - `cargo build`（default features）依赖图**必须仍无 `mdast` 与 `pulldown-cmark`**
    ——把 `mdast` 加进 workspace `members` 时**不要**加进 `default-members`；
  - TUI 正文默认切渲染视图是**唯一影响老用户的可见变更**（G1），已如实记录。
- **性能**：
  - `mdast::parse` 与 GUI 侧同源，`Cache` 复用同一形态（键 = `(段下标, 原文)`）。
    TUI 侧必须**同样缓存**——TUI 也会每帧重绘，`parse` 若每帧调用就是持续开销。
    这条容易漏：`render_body` 是纯渲染函数，`Cache` 需要 `&mut`，
    故 `render` 要接收 `&mut App`（当前签名是 `render(f, app: &App)`，
    `ui.rs:13`）——**这是一个必须改的函数签名**，理由就是缓存，
    不是为了别的。改动只影响 `tui/src/lib.rs` 的调用点；
  - 表格列宽分配是 O(单元格数)，每帧只算一次即可（不缓存也应足够，
    但表格多的清单要留意）；
  - `Block → Line` 转换每帧做一次全文转换是 O(正文长度)——
    可接受（GUI 侧 `show` 也是每帧走渲染，缓存的只有 AST）。
    但**渲染结果本身**也建议缓存（转成 `Vec<Line>` 缓存），
    否则大清单滚动时会掉帧。列为 T4 的实现约束。
- **安全**（本需求**收紧** TUI 的安全面）：
  - 链接不激活（G1 纪律 ①）——TUI 侧靠「不发转义序列」天然达成，
    单测断言渲染输出里没有 `ESC` 字节（关键设计 4）；
  - `GATE` 标记在渲染视图下不可见（纪律 ②）——顺带修掉一个**信息泄露面**：
    现状 TUI 把门禁内部标记（含 reviewer、email、`sig` 的标记行）直接显示给
    任何人看到屏幕的人。隐藏它们是安全改进，不只是观感改进；
  - 原文视图**仍然显示** `GATE` 行——这是用户主动要看源文本的视图，
    与 GUI 一致。如实记录这个边界，不假装「TUI 完全不显示标记」。
- **CI / 跨平台**：新增 `mdast` 是纯 Rust + `pulldown-cmark`，
  **无 eframe / 无平台依赖**，故 `ubuntu-latest` 上 `--workspace` 的
  fmt / clippy / test 可以包含它（比 `gui` 的平台排除更宽松，不是更严）。
  `tui` 本身在 CI 上已参与（ratatui 纯 Rust）。

### 风险点与回滚方案

- **风险 1（高）**：下沉 `mdast` 时漏掉 `escape_table_code_pipes`（REQ-013 的核心），
  导致表格单元格静默丢失——**这是 REQ-013 明确记录过的失效形态**
  （「对门禁工具来说，看不见比难看严重得多」）。
  缓解：T2 把 REQ-013 的竖线三条验收（AC-006 / AC-007 / AC-008）迁到 `mdast` 上，
  并新增 TUI 侧经公开入口的同款用例；T15 的判决性实验直接指向这一条。
- **风险 2（高）**：为让 TUI 复用而给 tui crate 加 `req-guard-gui` 依赖
  ——TUI 会被拖进 eframe，`--features tui` 的轻量构建失效。
  缓解：关键设计 1 的三选项表写明；验收用 `cargo tree -p req-guard-tui`
  断言依赖树**不含** `eframe` / `egui` / `req-guard-gui`。
- **风险 3（中）**：`can_review` 语义被误当成「现在该不该审」，
  界面只用它一个条件 → 已通过的段仍可重复批准，或越序段显示可审。
  缓解：G4 的 ⚠️ 与关键设计 6 明确要组合两个条件；T14 用 6 种段状态组合锁死。
- **风险 4（中）**：TUI 切换项目根后，旧的对话框/浮层残留，
  用旧仓的需求 id 去消费新仓的凭据 scope。
  缓解：关键设计 5——切换时清空选中项并关闭所有浮层；
  单测断言切换后 `prompt` / `overlay` 均为空。
- **风险 5（中）**：TUI 渲染每帧重新 `parse`（因为 `render` 只拿 `&App`，
  拿不到 `&mut` 去填 `Cache`）→ 大清单滚动掉帧。
  缓解：关键设计 2 已识别并说明「必须改 `render` 签名为 `&mut App`」；
  验收用「连续渲染 10 帧，`parse` 调用次数为 1」计数断言。
- **风险 6（低）**：`render` 签名从 `&App` 改 `&mut App` 波及 `tui/src/lib.rs`
  与既有 15 条 `TestBackend` 渲染冒烟测试。
  缓解：改动是机械的（测试里 `render(f, &app)` → `render(f, &mut app)`）；
  条数不得减少（验收写死）。
- **风险 7（低）**：跨 crate 的档位名集合断言（关键设计 3）脆弱。
  已如实标注为脆弱并给出改进路径（将来把 `Tone` 也下沉）。
- **回滚**：分三批可独立回滚——① `mdast` 下沉（T1/T2）、
  ② TUI 正文渲染（T4/T5/T6/T12）、
  ③ TUI 纯增量（T3/T7–T11/T13/T14）。
  ① 若要单独回滚，需把 `mdast` 搬回 `gui`（届时 GUI 侧测试可作护栏）。
  无数据迁移、无清单格式变更、无配置变更。

## 3. 测试计划


### 单元测试用例

| 编号 | 用例 | 断言点 |
| --- | --- | --- |
| U1 | `mdast`：REQ-013 的竖线三例（经公开 `parse` 入口） | `` `a|b` `` 与 `c\|d` → 2 格且内容为 `a\|b` / `c\|d`；含未转义裸竖线的表头 → 3 格且末格为 `2h`；未闭合反引号 → 3 格 |
| U2 | `mdast`：已转义竖线与围栏代码块 | `` `x\|y` `` → 1 格且内容为 `x\|y`；围栏内含裸竖线的行逐字未改写 |
| U3 | `mdast`：HTML 注释与链接 / 图片 | `<!-- GATE:STEP … -->` 不产出任何块；链接与图片只保留文字与目的地，不激活 |
| U4 | `mdast`：解析结果等价性 | 对同一段正文，`mdast::parse` 与下沉前 `gui::markdown::parse` 的 AST 结构逐块相等 |
| U5 | `mdast`：默认成员边界 | `cargo tree -p req-guard` 的输出不含 `mdast` 与 `pulldown-cmark` |
| U6 | `tui`：正文渲染为 Markdown | 标题加粗且按层级缩进；列表两层符号缩进差 > 0；任务列表显示 `☐` / `☑` |
| U7 | `tui`：渲染视图不含 GATE 标记 | 渲染产出的全部文本中不含 `GATE` 字样（锁风险 1 的信息面） |
| U8 | `tui`：原文视图含 GATE 标记 | 切到原文视图后可见 `GATE:STEP` 字样（证明两个视图确实不同，而非统一隐藏） |
| U9 | `tui`：链接不激活 | 渲染输出的行内不含任何 `ESC`（`\x1b`）字节，且链接文字带下划线样式 |
| U10 | `tui`：表格列宽 | 各列起始位置不重叠且行末对齐；超宽列以 `…` 截断，无横向滚动 |
| U11 | `tui`：围栏代码块不折行 | 渲染的代码行按原始字符输出，未按版心宽度折断 |
| U12 | `tui`：`render` 签名与缓存 | 连续渲染 10 帧，`mdast::parse` 调用次数为 1（锁风险 5） |
| U13 | `tui`：切换项目根——路径不存在 | 拒绝，`root` 保持原值，消息含 `不存在` |
| U14 | `tui`：切换项目根——路径是文件 | 拒绝，消息含 `不是目录` |
| U15 | `tui`：切换项目根——目录无 `.gates/requirements/` | 拒绝，消息含 `.gates/requirements`（不得表现为空列表） |
| U16 | `tui`：切换项目根——合法路径 | 成功；`reqs` 被重新载入；`prompt` 与 `overlay` 均为空；选中项重置 |
| U17 | `tui`：批准后跳段 | 三段清单下批准第 1 段后 `step` 变为 2 |
| U18 | `tui`：全部通过后跳段 | 全部 approved 时按 `a`，`step` 保持不变 |
| U19 | `tui`：可审性组合判定 | 6 种段状态组合下 `可审` 标记与实际可执行性逐项一致（锁风险 3） |
| U20 | `tui`：越序发起审批 | 拒绝并消息含阻塞段的段名（如 `需求分解`），而非 core 的原始报错 |
| U21 | `tui`：冻结徽标对照文案 | 帮助浮层的绘制文本包含 `未绑定` 与 `已改动` 的释义 |
| U22 | `tui`：左栏需求级状态标记 | `changes_requested` / `draft` / `in_review` 三种状态给出 3 种可区分标记 |
| U23 | `tui` / `gui` 色板档位 | 两侧的档位名集合相等（**不断言色值相等**，理由见关键设计 3） |
| U24 | `tui`：依赖树不含 GUI | `cargo tree -p req-guard-tui` 中不含 `eframe` / `egui` / `req-guard-gui`（锁风险 2） |
| U25 | `gui`：既有 markdown 测试 | 全部迁移到 `mdast` 后条数不减且全绿（`show()` 侧几何断言不变） |
| U26 | 既有 core / cli / tui / gui 全部单测 | 全绿；tui 的 15 条 `TestBackend` 渲染冒烟测试条数不减（签名为 `&mut` 后仍全绿） |
| U27 | 判决性实验：去掉 GATE 标记隐藏 | U7 必须 FAIL |
| U28 | 判决性实验：可审性只判 `can_review` 不判 `is_approved` | U19 必须 FAIL |
| U29 | 判决性实验：让 tui 依赖 `req-guard-gui` | U24 必须 FAIL |

### 端到端用例

| 编号 | 步骤 | 期望 |
| --- | --- | --- |
| E1 | 在本仓库起 TUI，选中 `REQ-013`，逐段翻看 | 正文按 Markdown 渲染：标题分级、表格对齐、任务列表有勾选框；屏上**看不到** `GATE` 标记行 |
| E2 | 同一份清单切到原文视图 | 看到原始 Markdown 含 `GATE` 标记行；再切回渲染视图又消失 |
| E3 | 打开含超链接的段落 | 链接显示为带下划线纯文本，鼠标点击**无任何反应**（终端未进入鼠标模式） |
| E4 | 审完「需求分解」段并批准 | 段光标**自动跳到**「技术方案」段，可直接继续 `a` |
| E5 | 三段全部批准后再按 `a` | 不跳段、不重复批准（提示无可审段） |
| E6 | 在第一段未批准时试图 `a` 第二段 | 拒绝并明确提示「需先完成 需求分解」 |
| E7 | 用切换根目录功能输入 `..`（上一级目录，无 `.gates/requirements/`） | 拒绝，提示含 `.gates/requirements`；界面仍停留在原项目 |
| E8 | 输入本仓库绝对路径 | 成功切回，需求列表重新载入，之前打开的浮层已关闭 |
| E9 | 在 80×24 小终端下打开帮助浮层 | 冻结徽标释义节可见；已有键位说明未被挤出视野 |
| E10 | 在本仓库起 GUI 与 TUI，看同一份清单的左栏 | 两端的 `changes_requested` 清单都带可区分标记（语义一致，色值可不同） |

### 边界 / 异常场景

- **B1** 正文为空（只有标题）：渲染视图不 panic，显示空态或空标题；
  原文视图显示空。沿用既有「渲染路径不 panic」类用例的写法。
- **B2** 正文含超长单行（无空行的 5000 字段落）：按版心折行，不横向滚动，
  翻页可看全。
- **B3** 正文含宽度超出版心的表格：列宽按比例压缩 + `…` 截断；
  **不得**引入横向滚动（关键设计 2）。
- **B4** 正文含嵌套围栏代码块（外层围栏内含 ```` ``` ````）：按原文处理，
  不试图嵌套渲染（与 GUI 的 N6 同口径）。
- **B5** 终端宽度 < 20 列：正文区仍可渲染（可读性差但**不崩溃**），
  浮层居中计算（`centered`，`ui.rs:468`）不得产生负宽区域。
- **B6** 切换项目根时目标目录里 `.gates/requirements/` 存在但为空：
  **允许切换**（是合法仓库，只是没有清单），显示空态并提示按 `n` 新建。
  ⚠️ 与 B15 区分：目录不存在 vs 目录存在但为空，后者在 `status::req_list`
  返回空集（`status.rs:194-196`），属正常。
- **B7** 切换项目根后立刻执行审批动作：凭据 scope 用的是**新仓的需求 id**
  （切换时已清空选中项，故用户必须先重新选中）——不会出现旧 id。
- **B8** `mdast` 解析一段格式极差的 Markdown：不 panic、不返回空但不报错，
  按「不丢内容」纪律至少渲染出可读文本（GUI 的纪律 3，TUI 同）。
- **B9** 清单的 `## 2. 技术方案` 标题被改坏：`section_of` 回退整篇
  （`requirement.rs:75-80`，这是**渲染路径的既定兜底**），
  故 TUI 渲染**照常显示**整篇内容而不是报错——与 GUI 现状一致。
  ⚠️ 这是渲染路径（fail-open 到整篇）与机械校验路径
  （`section_span` 必须报错）的**有意分歧**，不得在本需求里"顺手统一"。
- **B10** 终端不支持真彩色：ratatui 自动降级到 16 色，`Tone → Color` 映射
  的语义档位仍成立（档位不依赖色深），不崩溃。
- **B11** 切换项目根时正有 `Prompt` 弹窗打开：弹窗被关闭，不残留半截输入。

### 回归范围与影响面

- `cargo test -p req-guard-mdast`（新 crate）：全绿，且测试条数 ≥
  从 gui 迁移过来的条数（REQ-013 记为 markdown 23 条）。
- `cargo test -p req-guard-gui`：全绿，条数与改前相同
  （测试只是「搬家」，数量不应减少）；`show()` 侧几何断言**不改期望值**。
- `cargo test -p req-guard-tui`：全绿，条数 ≥ 改前 + 15（新增 U6–U22）。
- `cargo test -p req-guard-core` / `-p req-guard`：全绿，条数与改前**完全相同**
  （本需求不改 core，这是「N4/N5 不改 core」的硬证据）。
- `cargo fmt --check` 与 `cargo clippy --all-targets`（含新 crate）零输出。
- `cargo build`（default features）秒级完成，`cargo tree -p req-guard`
  中不含 `mdast` / `pulldown-cmark`（U5）。
- `cargo tree -p req-guard-tui` 不含 `eframe` / `egui` / `req-guard-gui`（U24）。
- CI：`ubuntu-latest` 的 `--workspace` lint / test 现在**可以**包含 `mdast`
  （无平台依赖）；`gui` 的既有平台排除逻辑**不受影响**。

### 验收门槛

- `cargo fmt --check` 与 `cargo clippy --all-targets`（五个 crate）零输出。
- 五个 crate 的 `cargo test` 失败数均为 0。
- `cargo test -p req-guard-core` 与 `-p req-guard` 测试条数与改前完全相同。
- `cargo test -p req-guard-tui` 测试条数 ≥ 改前 + 15；
  `cargo test -p req-guard-gui` 测试条数与改前相同。
- `cargo tree -p req-guard` 不含 `mdast` / `pulldown-cmark`；
  `cargo tree -p req-guard-tui` 不含 `eframe` / `egui` / `req-guard-gui`。
- **判决性实验**：去掉 GATE 隐藏（U27）、可审性漏判 `is_approved`（U28）、
  tui 依赖 gui（U29），对应单测必须 FAIL（自证的测试不算判决）。
- 人工检查：GUI 侧正文渲染观感与改动前**无可见差异**（`show()` 未改）。

<!-- GATE:AC -->
### AC-001
- Given: 一行表格的数据行为 `| T1 | 变异清单（`file|anchor|repl`） | 2h |`，表头为 3 列
- When: 对该表格调用新共享 crate 的公开正文解析入口
- Then: 解析出 3 个单元格，且第 3 格文本逐字等于 `2h`

### AC-002
- Given: 一段围栏代码块，其代码行内容为 `| a | `x|y` | `，且表格数据行含 `c\|d`
- When: 对该段与该行分别调用新共享 crate 的公开正文解析入口
- Then: 围栏块内文本逐字未被改写，且该表格行解析出 2 格且第 2 格文本为 `c|d`

### AC-003
- Given: 一段正文，其中含 HTML 注释 `<!-- GATE:STEP name=decomposition -->` 与一个指向 `https://a.tld/x` 的链接
- When: 调用新共享 crate 的公开正文解析入口
- Then: 解析结果中承载注释文本的块数为 0，且链接条目保留文字 `点我` 但不携带可激活标记

### AC-004
- Given: 实现完成后的 gui crate，其正文渲染调用点
- When: 执行该 crate 的全部单测
- Then: 失败数为 0，且测试总数与本次改动前的记录条数相同

### AC-005
- Given: 实现完成后的 workspace 默认构建
- When: 对默认构建产物执行 `cargo tree`
- Then: 输出中既不含新共享 crate 名也不含 `pulldown-cmark`

### AC-006
- Given: 实现完成后的 tui crate 的终端界面，正文区处于渲染视图
- When: 收集该视图全部绘制文本
- Then: 绘制文本中不出现字面量 `GATE`

### AC-007
- Given: 实现完成后的 tui crate 的终端界面，正文区可切换视图
- When: 切换到原文视图并收集全部绘制文本
- Then: 绘制文本中逐字出现 `GATE:STEP`，即原文视图与渲染视图呈现不同

### AC-008
- Given: 一段正文，其中含一个指向 `https://a.tld/x` 的链接
- When: 在 tui crate 的渲染视图下收集该链接所在行的全部字节
- Then: 该行字节中不含 `0x1b` 转义字节，且链接文字带有下划线样式

### AC-009
- Given: 一段正文，其中含两层嵌套列表
- When: 在 tui crate 的渲染视图下读取两层列表符号的起始列
- Then: 内层符号的起始列不小于外层符号起始列加 1

### AC-010
- Given: 一份清单的两段已批准且第三段待审，且终端界面处于该清单详情
- When: 批准第一段
- Then: 段光标下标等于 2，即自动跳到第三段

### AC-011
- Given: 一份清单的三段全部已批准
- When: 在终端界面按批准键
- Then: 段光标下标保持不变，且该清单的审批记录行数增量为 0

### AC-012
- Given: 一份清单的第一段待审而第二段待审，且强制顺序开启
- When: 读取该清单三段列表上的可审标记
- Then: 第一段标记为可审，第二段标记为不可审，且第二段的提示文本逐字包含 `需求分解`

### AC-013
- Given: 一份清单的第一段已批准而第二段待审，且强制顺序开启
- When: 读取该清单三段列表上的可审标记
- Then: 第一段标记为不可审且第二段标记为可审，即 1 个已通过段的可审标记为假

### AC-014
- Given: 终端界面已打开切换项目根的输入弹窗，且输入的路径不存在
- When: 提交该路径
- Then: 界面提示包含字面量 `不存在`，且当前项目根路径保持不变

### AC-015
- Given: 终端界面已打开切换项目根的输入弹窗，且输入的路径是一个普通文件
- When: 提交该路径
- Then: 界面提示包含字面量 `不是目录`，且当前项目根路径保持不变

### AC-016
- Given: 终端界面已打开切换项目根的输入弹窗，且输入的目录内没有需求清单目录
- When: 提交该路径
- Then: 界面提示包含字面量 `.gates/requirements`，且当前项目根路径保持不变

### AC-017
- Given: 终端界面已打开切换项目根的输入弹窗，输入了一个含需求清单目录的合法路径，且当前有一个弹窗处于打开状态
- When: 提交该路径
- Then: 项目根切换成功、需求列表重新载入、弹窗状态为空且选中项下标为 0

### AC-018
- Given: 实现完成后的 tui crate，其终端界面处于正文渲染视图且已渲染 10 帧
- When: 统计公开正文解析入口的调用次数
- Then: 该调用次数为 1，即解析结果被缓存而非每帧重算

### AC-019
- Given: 实现完成后的 tui crate 与 gui crate 的语义色板
- When: 分别枚举两者的语义档位名并取集合
- Then: 两个集合逐项相同，且 tui crate 的依赖树中不含 `eframe` 与 `req-guard-gui`

### AC-020
- Given: 实现完成后的 tui crate，其帮助浮层
- When: 收集帮助浮层的全部绘制文本
- Then: 绘制文本同时包含字面量 `未绑定` 与 `已改动` 的释义说明

### AC-021
- Given: 实现完成后的 core、cli、mdast 三个 crate
- When: 依次执行 `cargo test`
- Then: 三个 crate 的失败数均为 0

### AC-022
- Given: 实现完成后的 tui crate
- When: 执行 `cargo test`
- Then: 失败数为 0，且测试总数不少于改动前总数加 15

### AC-023
- Given: 实现完成后的 mdast、tui、gui 三个 crate
- When: 执行 `cargo fmt` 检查与 `cargo clippy --all-targets`
- Then: 前者无 diff 输出，后者 warning 数为 0

### AC-024
- Given: 实现完成后的两个界面的正文渲染视图
- When: 把渲染视图对 HTML 注释的处理改为原样输出
- Then: 该单测的失败数为 1，即该隐藏行为不是自证的

### AC-025
- Given: 实现完成后的 tui crate，其可审标记的计算逻辑
- When: 把计算逻辑改为只判断顺序约束而不判断该段自身是否已批准
- Then: 该单测的失败数为 1，即组合判定不是自证的

### AC-026
- Given: 实现完成后的 tui crate 的依赖声明
- When: 把其依赖改为直接引用 gui crate 后执行依赖树断言
- Then: 该断言的失败数为 1，即终端界面未被拖入图形界面依赖树

### AC-027
- Given: 一份需求清单的「## 2. 技术方案」二级标题被改坏，导致该段定位失败
- When: 在 tui crate 的正文视图渲染该清单
- Then: 渲染返回 `Ok`，且绘制文本包含字面量 `需求分解`，即按既定兜底显示整篇内容
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-05_14:23:30 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-05_14:23:37 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-05_14:23:45 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
<!-- /GATE:AUDIT -->
