---
doc_type: proposal
tier: standard
owner: -
review_policy: codebound
verified_at: 2026-10-05
source_refs: [gui/src, gui, Cargo.lock, docs/设计]
---

# REQ-013 GUI 正文排版观感与 Markdown 解析层收敛

> **AI 需求门禁清单**：三段步骤全部 `approved` 后，AI 才被允许编写代码。
> 本文件是硬拦截依据——`.gates/hooks/req-guard-check.{sh,ps1}` 只解析下列 `GATE` 标记行；
> 正文可自由编辑，但**请勿手工修改 GATE 行**（请用 `req-guard approve`）。

<!-- GATE:HEAD id=REQ-013 status=approved created=2026-10-05_12:24:29 -->
<!-- GATE:STEP name=decomposition label=需求分解 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:41:58 sum=222a737de6077b8a48eef7bb39ff7c0b66cec9a7b415b0020f8e1be8cccf6d42 -->
<!-- GATE:STEP name=solution label=技术方案 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:47:24 sum=9f6c482c27c958a91cbfbf38b270536f9c140b8d13a7d273526a32a6e8269bdf -->
<!-- GATE:STEP name=testplan label=测试计划 status=approved reviewer=Mike_Zhu email=zhuyuan2706@gmail.com sig=490ced91b512 updated=2026-10-05_14:45:40 sum=a5f178e0280d3d40bbf59cc0682b3a9e441bf25ef94f25ad6c7a9e17a85dadda -->

## 1. 需求分解

### 背景与问题

GUI 正文由 `gui/src/markdown.rs` 渲染（解析用 `pulldown-cmark`，渲染与安全纪律自研）。
本清单起因于审核人的两条反馈：**排版观感简陋**、**解析在边界写法上不稳**。
第一至五条已在本轮修完，第六、七条是**实现过程中被自身测试与审核人实测暴露出来的相邻缺陷**，
一并纳入本次范围（两者的修复都已落地并各有判决性用例）。

1. **行宽没有上限，正文被拉到整屏宽**。`render_center`（`gui/src/app.rs`）把整个中央栏交给
   `markdown::show`，而 `show` 直接用 `ui.available_width()` 当版心。窗口默认 1000×680、
   拉宽到 1400 后中央栏就有 1200+px，一行能塞 50 多个汉字——中文正文舒适区是 30–40 字/行，
   超过就"扫行时找不到下一行开头"。表格、段落、列表因此全都按同一个过宽的版心折行。

2. **列表缩进不随层级变化**。`LIST_INDENT_W` 是一个常量 16px，`list_block` 每层都只加这一个
   常量，于是外层项与内层项的正文起点只差 16px，两层挤在一起看不出层级；有序列表编号
   `format!("{}.", …)` 宽度随位数变化，"9." 与 "10." 的正文起点不同。

3. **块间距与标题层级都是单一常量**。块间距一律 6px；标题 1–4 级只改字号、不区分色阶，
   于是一屏里"标题 / 段落 / 列表 / 表格"的呼吸感完全一样；标题里的 `**粗**` 与 `` `码` ``
   还被 `inline_text` 拍平成纯文本，行内样式在标题里直接丢失。

4. **解析层是手写状态机，只覆盖"够用"子集**。REQ-011 已修掉表格竖线与列宽两个缺陷，
   但解析器本身仍不实现 GFM：setext 标题、autolink、HTML 实体、列表项懒续行、表格缺格补齐
   等都没有；`<!-- GATE:… -->` 的隐藏、单元格内代码段的等长反引号闭合、任务列表勾选态，
   全靠 500 余行自研代码维持。

5. **换库这件事本身有实测坑，不能一把梭**。实测 `pulldown-cmark 0.13.4`
   （`Options` 开 `ENABLE_TABLES | ENABLE_TASKLISTS | ENABLE_STRIKETHROUGH`）直接解析
   REQ-011 验收里的原例

   ```text
   | 编号 | 子任务 | 预估工时 |
   | --- | --- | --- |
   | T1 | 变异清单文件格式（`file|anchor|replacement|test-filter|理由`） | 2h |
   ```

   得到 3 格 `["T1", "变异清单文件格式（`file", "anchor"]` —— 单元格里的裸竖线**照样切列**，
   且 `repl` 与 `2h` 被**静默丢弃**。原因是 GFM 明确规定表格内的竖线**必须转义**（代码段内也一样），
   `pulldown-cmark` 严格照办，而本项目的验收基线是"代码段内竖线算内容"。
   即：**换库必须带一层预归一化**，否则会用"更规范的解析器"换来"内容丢失"——
   对门禁工具来说，"看不见"比"难看"严重得多。

6. **短表格下方会出现一整屏空白**（审核人实测截图）。成因：`table()` 给横向 `ScrollArea`
   写了 `.auto_shrink([true, false])`。该 ScrollArea 没开纵向滚动，而 egui 里
   "方向未开 + `auto_shrink=false`"对应 `inner_size.y = max(可用高, 内容高)` ——
   **两行的短表格也占满整屏高度**，其后的段落被顶到视口外（离屏实测：那段文字**根本没被画出来**，
   滚下去才看见）。这条缺陷 REQ-011 就带着，只是此前正文窄、表格常在折叠段里，没被看到；
   版心收口后表格与后续段落挨得更近，问题被顶到眼前。

7. **切换审核段时视口停在"新段的中段"**（审核人实测："展示的内容不是从头开始"）。
   两帧离屏模拟量到的数字：第 1 段展开且滚动偏移 1800 时内容高 5159；点第 2 段标题后
   第 1 段收起，内容高降到 3993，而 **ScrollArea 的偏移被原样保留在 1800** ——
   第 2 段的标题被推到屏幕 y=2144（视口仅 600 高），视口正好停在第 2 段正文的中段。
   成因在 `render_center`：`app.step` 一变，上一段正文收起（内容变矮），但滚动位置没人管。
   这不是本轮改动引入的（是"点哪一段就看哪一段"的老问题），但本轮把正文排版修好之后，
   它成了审核人 daily 流程里最硌手的一处，一并修。

### 目标

- **G1 版心宽度有上限**：正文折行宽度 = `min(ui.available_width(), MEASURE_W)`，
  `MEASURE_W = 680px`（≈ 34 个汉字，符合中文舒适区）；窄面板时仍取可用宽，不出现横向滚动。
- **G2 版心水平居中**：版心左右留白差 < 1px（宽屏下不再"贴左 + 右侧一大片空白"）。
- **G3 列表层级可读**：每深入一层缩进 +16px；有序编号按本列表最大位数右对齐，
  同一列表内各项正文起点一致；块间距按块类型分级（不再一律 6px）。
- **G4 标题层级可辨**：1–2 级用正文色 + 粗体，3–4 级用 `Tone::Muted.color(ui)` + 粗体
  （该色在浅/深底对比度 6.63 / 6.04，均过 WCAG AA），字号梯度保留；标题内 `**粗**` / `` `码` ``
  保留行内样式。
- **G5 解析层换成成熟实现**：`pulldown-cmark 0.13.4`（`default-features = false`），
  **保留 `Block` / `Inline` AST 与四条安全纪律**（链接不激活、图片不加载、HTML 注释隐藏、
  不丢内容）；REQ-011 的表格竖线三条验收必须**继续成立**（由预归一化保证）。
- **G6 依赖增量可度量**：`pulldown-cmark` 以 `default-features = false` 引入，
  新增传递依赖恰为 3 个（`bitflags` / `memchr` / `unicase`），release 二进制增量 ≤ 400KB
  （实测 12,367,716 → 12,647,116 = +279,400 字节）。
- **G7 表格不留大片空白**：短表格的高度等于其行高之和，其后的段落必须落在同一屏内可见
  （离屏断言：表格底与下一段顶的间距 < 30px，且下一段必须被画出）。
- **G8 切段后视口落在该段开头**：点击某段标题后，该段标题与其正文开头必须落在视口内；
  外层 ScrollArea 补 `id_salt`，使滚动偏移的记忆不依赖"前面创建了几个控件"。

### 非目标

- **N1** 不引语法高亮：`syntect`（实测其 `default` feature 走 onig（C 库），要用须
  `default-features = false + default-fancy`），代码块维持纯色文本。
- **N2** 不引整包渲染器：`egui_extras 0.36.2` 已无 markdown 模块（实测其模块表里只有
  `datepicker / syntax_highlighting / image / layout / loaders / sizing / strip / table`，
  依赖表里也没有 `pulldown-cmark`）——`docs/设计/UI架构细化方案.md` §4.4 那句"可引
  `egui_extras` / `egui_markdown`"已过时，本次一并更正；`egui_markdown` 在 crates.io 上
  只剩 0.1.0 / 175 下载的同名新包（非 emilk 的 0.7.x），不引；`egui_commonmark 0.25.0`
  因链接走 `ui.hyperlink_to`（backend `src/misc.rs:239`）且无开关而不引。
- **N3** 不改清单格式，不要求存量清单迁移，不改模板。
- **N4** 不做"可点链接 / 图片加载"——安全取舍（模块注释里的四条纪律）不动。
- **N5** 不动 `core` / `cli` / `tui`；`cargo build`（default features）仍零 UI 依赖。
- **N6** 不追 GFM 全量：脚注、定义列表、front-matter 块不解析成正文（按原文/跳过处理）。
- **N7** 不改 TUI 与左栏需求列表的滚动行为；G8 只管中央面板"点段标题 → 视口到该段开头"。

### 子任务拆解

| 编号 | 子任务 | 预估工时 |
| --- | --- | --- |
| T1 | `show()` 入口收版心：`MEASURE_W` 常量 + 限宽居中的子 ui（`scope_builder`，**不是** `new_child`） | 0.5h |
| T2 | 块间距分级：抽出间距常量表，标题前后 / 块间分别取值 | 0.5h |
| T3 | 列表层级缩进：符号列定宽 + 嵌套项从行左边缘算缩进 + 有序编号右对齐 | 1.5h |
| T4 | 标题色阶（复用 `palette::Tone`）+ 标题内保留行内样式 | 1h |
| T5 | `gui/Cargo.toml` 引入 `pulldown-cmark 0.13.4`（`default-features = false`） | 0.5h |
| T6 | 预归一化器：表格行内代码段的**未转义**裸竖线加反斜杠（复用 `tick_run`/`tick_close`） | 1h |
| T7 | 事件 → AST 映射替换 `parse()` 旧实现，删旧解析器（约 500 行 → 约 200 行） | 2h |
| T8 | 单测：几何断言改基准 + 竖线三条走公开入口 + 归一化边界 4 例 + GATE 注释 / 链接降级 | 2h |
| T9 | `docs/设计/UI架构细化方案.md` §4.4 更正 + 排版纪律表补 3 条 | 1h |
| T10 | 判决性实验：关掉预归一化 / 把版心改回 `available_width()`，对应单测必须 FAIL | 0.5h |
| T11 | `table()` 的横向 ScrollArea 改 `auto_shrink([true, true])`（高度跟内容走） | 0.5h |
| T12 | `render_center`：切段时 `scroll_to_me` 到该段标题 + ScrollArea 补 `id_salt` | 0.5h |
| T13 | 两条新缺陷的判决性用例：短表格间距离屏断言 + 两帧滚动模拟（偏移保留的形态） | 1.5h |

合计 13.5h。T1–T4（排版，零新增依赖）可独立交付、独立回滚；T5–T7（解析层）是第二步；
T11–T13 是实现期暴露的相邻缺陷，与 T1–T4 同批交付（T11 在渲染器内，T12 在 `app.rs`）。

### 影响范围

- **模块**：`gui/src/markdown.rs`（渲染器主体）、`gui/src/app.rs`（**新增**：中央面板的
  滚动位置与 ScrollArea id）、`gui/Cargo.toml` + `Cargo.lock`（新增 1 条依赖）、
  `docs/设计/UI架构细化方案.md`（§4.4 更正 + 排版纪律表）。
- **接口**：`parse(src) -> Vec<Block>`、`show(ui, &[Block])`、`Cache::get/clear`、`inline_text`
  签名与语义**全部不变**；`Block` / `Inline` 类型不变（渲染层与调用方 `app.rs` 的正文入口
  零改动）。层级缩进靠内部新增 `show_at(ui, blocks, depth)` 私有函数实现，不外泄。
  `render_center` 只在"点击段标题"分支多调一次 `Response::scroll_to_me`，不改任何状态语义。
- **配置 / 清单格式 / 数据表 / 外部 API**：无。
- **向后兼容**：解析出的 AST **语义等价**（竖线三条、任务勾选态、有序起始编号、引用内嵌套、
  链接/图片降级、`<!-- GATE:… -->` 隐藏），并新增 GFM 覆盖；唯一**有意的断言变更**是
  REQ-011 的两条离屏几何断言改用新基准 `min(可用宽, MEASURE_W)`（断言意图不变：表与正文同宽、
  文字不越出版心）。G7 / G8 只改排布与视口，不改任何内容。

### 验收标准

见第 3 段 `GATE:AC` 块（编号连续、Given/When/Then 齐全且可度量；其中两条为本次新增的
短表格空白与切段视口验收，由审核人经 `amend` 补入）。

## 2. 技术方案

### 总体思路

**排版与解析分开两步走，共用同一份 AST 与同四条安全纪律**：排版只动常量与入口版心
（零新增依赖，先行）；解析层换成 `pulldown-cmark` 但**外面套一层 25 行预归一化**，
把"代码段内裸竖线算内容"这条项目基线补回 GFM 规范之上。

### 关键设计

1. **版心收口在 `show()` 入口**（T1）：`let m = ui.available_width().min(MEASURE_W);`
   然后 `ui.vertical_centered(|ui| { ui.set_max_width(m); …原渲染循环… })`。
   为什么必须放在入口而不是每个块里：REQ-011 的第一条纪律是"宽度只有版心一个来源"，
   任何一层自己再问宽度都会让同段里"这段行数多、那段行数少"。`set_max_width` 会改写子 ui 的
   `max_rect`，因此 `show` 内部所有 `ui.available_width()`（段落折行、表格列宽分配、
   代码块滚动区）自动都变成 `m` —— 一处改动全局收敛。
   ⚠ 连带影响：REQ-011 里"表宽 == 版心""段落折行宽 == 版心"两条离屏断言的对照基准要改成
   `min(available, MEASURE_W)`（800×600 屏下即 680），否则这两条会先炸。

2. **块间距分级**（T2）：把 `add_space(6.0)` 换成常量表——块间 `BLOCK_GAP = 10`、
   标题前 `HEADING_GAP_BEFORE = 14` / 后 `HEADING_GAP_AFTER = 4`、列表项内不加额外间距
   （交给 `ui.spacing().item_spacing.y`）。理由：现在"标题"与"段落"之间的空白和"两个段落"之间一样，
   层级在视觉上被抹平。

3. **列表层级缩进**（T3）：`show` 保持签名，内部转 `show_at(ui, blocks, depth)`，
   `list_block` 收到 `depth` 后 `ui.add_space(LIST_INDENT_W * (depth + 1))`；
   有序列表编号按本列表**最大编号位数**右对齐（`{:>width$}.`），使同一列表内各项正文起点一致。
   `horizontal_top` 一项一行的既有纪律不动（它是为了不让 wrap 布局把折行宽度掺进来）。

4. **标题色阶 + 行内样式**（T4）：1–2 级沿用正文色 + 粗体；3–4 级用 `palette::Tone::Muted.color(ui)`
   + 粗体（该色浅底 6.63 / 深底 6.04，对比度达标的实测值见 `palette.rs` 单测）。
   标题文本改走 `inline_job`（复用段落那条路径），对每个 `RichText` 追加 `.size(size)`，
   于是 `` ### 小节 **粗** `码` `` 里粗体与代码样式都还在。

5. **解析层换库，AST 不换**（T5/T7）：`pulldown-cmark = { version = "0.13.4", default-features = false }`。
   实测依赖增量：`default-features = false` 下只多 3 个 crate（`bitflags` / `memchr` / `unicase`）；
   release 冷编译实测 7.6s；空程序 423424 字节 → 带解析器的最小二进制 723512 字节，
   增量 300088 字节（相对 GUI 的 18–64MB 可忽略）。
   `Options` 只开 `ENABLE_TABLES | ENABLE_TASKLISTS | ENABLE_STRIKETHROUGH`（实测这三项
   覆盖 REQ-011 的三条竖线验收 + 勾选态 + 删除线；脚注/定义列表按 N6 不开）。
   事件 → `Block`/`Inline` 的映射要点（均为实测得到）：
   - `Tag::Table([Left, Right, Center])` 直达 `Align`，`Alignment::None` 记为 `Left`；
   - `Event::TaskListMarker(bool)` → `ListItem::checked`；`Tag::List(Some(3))` → `ordered + start`；
   - 表格数据行缺格**实测自动补空串**（`["1","",""]`），比旧解析器更宽容；
   - `Event::Html` / `InlineHtml` / `Tag::HtmlBlock` **一律丢弃** → `<!-- GATE:… -->`
     实测正是以 `HtmlBlock` + `Html("<!-- GATE:STEP name=x -->\n")` 到达，丢弃即隐藏；
   - `Tag::Link` / `Tag::Image` 仍只取文字与 `dest` 存进 `Inline`，渲染层不激活
     （`rt_of` 里链接仍是"带下划线的纯文本"，见 `gui/src/markdown.rs:1128`）——**安全纪律零改动**。

6. **预归一化器：换库后"不丢内容"的落点**（T6，本方案的关键设计）：
   在把原文交给 `pulldown-cmark` **之前**做一次行级归一化，只做一件事：
   **表格行里、代码段中、未被转义的裸竖线**加一个反斜杠。
   - 判定复用现成的 `tick_run`（连续反引号个数）与 `tick_close`（**等长**闭合），
     未闭合时按普通字符处理（fail-closed 到"照常切列"，与 REQ-011 的 U3 同向）；
   - 已是 `\|` 的不重复加（判据：前一字符是 `\` 则跳过）——否则单元格里会多出一个反斜杠；
   - **围栏代码块内的行不动**（`in_fence` 状态跟踪），非表格行（含普通段落里的 `` `a|b` ``）不动，
     否则会改写用户没打算改的正文；
   - 归一化只改**竖线前加一个 `\`**，不改任何文字，AI 写清单的习惯不用迁移。
   实测（同一段代码里对四组输入跑通）：REQ-011 原例 → 3 格、末格 `2h`；
   `` `a|b` `` 与 `c\|d` → 2 格、内容 `a|b` / `c|d`；未闭合反引号 → 3 格；
   已转义的 `` `x\|y` `` → 1 格、内容 `x|y`；围栏代码块内含 `` | `x|y` | `` 的行**未被改写**。

7. **为什么不整体换 `egui_commonmark`**：见非目标 N2（链接可点且无开关是**安全**问题，
   不是观感问题；`egui::Grid` 的末列吃剩余宽度是 REQ-011 已经踩过并写进注释的坑）。
   本方案只借解析器，排版与安全纪律仍在自己手里——这与 §4.4 "正文渲染器宁少勿全"的取舍一致。

### 涉及的文件与模块清单

<!-- GATE:TOUCH -->
gui/src/markdown.rs
gui/Cargo.toml
Cargo.lock
docs/设计/UI架构细化方案.md
gui/src/app.rs
<!-- /GATE:TOUCH -->

### 兼容性、性能与安全影响

- **兼容性**：`parse` / `show` / `Cache` / `Block` / `Inline` 的公开形态不变，`gui/src/app.rs`
  调用点零改动。解析语义等价并新增 GFM 覆盖；已知会变的写法只有一类：旧实现把"列表项内顶格
  续行"并入上一项（自研启发式），换成 pulldown-cmark 后按 CommonMark 判定归属，
  极个别清单的视觉归属可能变化（内容不会少）。断言层面的唯一变更是前述两条离屏几何断言换基准。
- **性能**：解析从"逐字符 `Vec<char>` 状态机"换成 `memchr` 加速的线性扫描，量级上更快；
  每帧成本不增（`Cache` 仍在，键为 `(段下标, 原文)`）。渲染层每帧多出的排版量不变。
- **依赖与产物**：`gui` 新增 1 条直接依赖、3 条传递依赖；release 二进制实测增量 300088 字节
  （≤ G6 的 400KB 上限）；`cargo build`（default features）依赖图不变。
- **安全**：四条纪律（链接不激活 / 图片不加载 / HTML 注释隐藏 / 不丢内容）全部保留；
  新增依赖仅 3 个成熟小 crate。需要特别记住的是**新纪律**：换库后"不丢内容"这条**从解析器
  转移到了预归一化器**——直接喂原文会静默丢单元格（背景与问题第 5 条已实测），
  因此预归一化器与它的边界单测是本次的硬交付，不是可选优化。

### 风险点与回滚方案

- **风险 1（高）**：预归一化器误伤围栏代码块或非表格行（把用户正文里的竖线改成 `\|`）。
  缓解：`in_fence` 状态 + "仅当行内含 `|`"双条件；两条单测锁住（围栏内含裸竖线的代码块原样、
  普通段落里 `` `a|b` `` 原样）。
- **风险 2（中）**：已转义竖线被重复转义（`\|` → `\\|`，单元格里多一个反斜杠）。
  缓解：加转义前判"前一字符是 `\`"；单测锁住 `` `x\|y` `` → 内容恰为 `x|y`。
- **风险 3（中）**：旧解析器的启发式归属（顶格续行并入上一项等）在新解析器下观感变化，
  存量清单里可能有段落"换了位置"。缓解：既有解析单测逐条改写为新期望并人审 diff；
  若审核人认为观感不可接受，可只回滚 T5–T7（第一步排版改动不含在回滚范围内）。
- **风险 4（中）**：版心上限 680px 是**审美取值**，不同字体 / DPI 下"34 字一行"未必合适。
  缓解：`MEASURE_W` 提为单一常量，改一行即可；单测只断言"`min(可用宽, 680)`"这个定义本身，
  不锁死"必须 680px"以外的美学结论。
- **风险 5（低）**：`pulldown-cmark 0.13` 的事件枚举在后续小版本可能增项（需在 `match` 里补分支）。
  缓解：依赖声明写 `version = "0.13.4"`（`^0.13`，不含 0.14）；CI 的 `cargo clippy --all-targets`
  零 warning 门槛会把"漏处理新事件导致的未覆盖代码"暴露出来。
- **回滚**：`git revert` 即可，无数据迁移、无清单格式变更；两步可分别回滚
  （T1–T4 一批、T5–T7 一批）。若只想退解析层，删 `gui/Cargo.toml` 的依赖行、
  恢复旧 `parse` 段并保留排版改动——这依赖"解析层与渲染层分离"的既有分层，所以本次不改分层。

## 3. 测试计划

### 单元测试用例

| 编号 | 用例 | 断言点 |
| --- | --- | --- |
| U1 | 800×600 离屏渲染一段长正文 | 段落折行宽度 == `min(available, MEASURE_W)`（=680），容差 1px |
| U2 | 同上，读正文块左右留白 | 左右留白差 < 1px（水平居中） |
| U3 | 段落 + 两列短表（REQ-011 两条几何断言改基准） | 表头底色宽 == 版心；单元格文字右边缘 ≤ 版心 +1px |
| U4 | 两层嵌套列表 | 内外层符号的 x 相差 == `LIST_INDENT_W`（16），容差 0.5px |
| U5 | 同层两项短列表（保留 REQ-011 的不变式） | 两项 y 相差 ≥ 8px，两者折行宽度差 < 0.5px |
| U6 | 有序列表 9 / 10 两项 | 两项正文起点 x 相同（编号按最大位数右对齐） |
| U7 | REQ-011 原例（走**公开** `parse`，不再直接调 `cells_of`） | 3 格，末格文本 == `2h` |
| U8 | `` `a|b` `` 与 `c\|d` 同表 | 2 格，格内文本恰为 `a|b` / `c|d` |
| U9 | 单元格内未闭合反引号 `b`c` | 仍按列分隔成 3 格（不成对不是代码段） |
| U10 | 归一化边界三例 | 已转义 `` `x\|y` `` → 1 格且内容 `x|y`；围栏代码块内裸竖线行**原样**；普通段落里 `` `a|b` `` 原样 |
| U11 | 标题内行内样式 | `### 小节 **粗** ` + 反引号码 解析出 `Strong` 与 `Code` 子节点（拍平前的结构） |
| U12 | `<!-- GATE:… -->` 与链接 / 图片 | 渲染文本不含 `GATE`；`Inline::Link.dest` 不出现在任何绘制文本里 |
| U13 | 既有 markdown + app 单测 | 全绿（断言改写后条数不减少） |
| U14 | 判决性实验两例 | ① 版心改回直接用 `available_width()` → U1 失败；② 归一化器短路（直喂原文）→ U7 失败且末格不再是 `2h` |

### 端到端用例

| 编号 | 步骤 | 期望 |
| --- | --- | --- |
| E1 | 打开 REQ-011 那份清单（1000×680 与 1400×900 各一次），展开"技术方案"段 | 两种窗口宽度下正文的行宽与左右留白观感一致；宽屏下正文成块居中，不被拉成整屏 |
| E2 | 看含三层嵌套 + 任务列表的段落 | 层级一眼可辨（缩进逐层递增），勾选框仍只读 |
| E3 | 看含"代码段内含竖线"的三列表格 | 3 列、子任务整句在一格内、`2h` 在末列（与 REQ-011 的验收一致） |
| E4 | 切「原文」视图再切回 | 两种视图内容一致（渲染层没有吞字），原文视图仍可整段复制 |

### 边界 / 异常场景

- **B1** 正文宽于版心（窗口很宽）：正文居中，不出现横向滚动条。
- **B2** 面板被拉得比版心还窄：版心 == 可用宽（`min` 生效），不溢出、不挤压表格。
- **B3** 表格列多到连列宽下限都分不完：仍出横向滚动条（沿用 REQ-011 的既有行为，不回归）。
- **B4** 围栏代码块里出现 `` | `a|b` | `` 这样的行：内容一字不改（归一化不碰围栏内）。
- **B5** 段落（非表格行）里有 `` `a|b` ``：内容一字不改。
- **B6** 表格单元格同时出现 `\|` 与裸 `|`（同一行）：两种都归位，无重复转义、无丢格。
- **B7** 空清单 / 只有标题：渲染不 panic（沿用 `渲染路径不panic且真的画出了东西` 那条）。

### 回归范围与影响面

- `cargo test -p req-guard-gui` 全量（含 app 的 15 条 GUI 状态测试）。
- `cargo fmt -p req-guard-gui -- --check` 与 `cargo clippy -p req-guard-gui --all-targets` 零输出。
- `cargo test -p req-guard-core` / `req-guard`（本需求不碰这两个 crate，结论应一字不变）。
- `cargo build`（default features）仍秒级、依赖图无 `pulldown-cmark`：`cargo tree -p req-guard`
  中不出现该 crate。
- 其余 crate（`core` / `cli` / `tui`）不依赖本模块，结论应一字不变。

### 验收门槛

- `cargo fmt -p req-guard-gui -- --check` 零 diff。
- `cargo clippy -p req-guard-gui --all-targets` 零 warning。
- `cargo test -p req-guard-gui` 全绿，且测试总数 ≥ 现有 38 条（markdown 23 + app 15）。
- `cargo tree -p req-guard-gui` 中 `pulldown-cmark` 的传递依赖恰为 `bitflags` / `memchr` / `unicase` 3 个。
- release 二进制体积增量 ≤ 400KB（对比改前后的 `target/release/req-guard` 字节数）。
- **判决性实验**：分别把版心改回"直接用 `available_width()`"、把预归一化器短路成"直喂原文"，
  对应单测必须 FAIL（自证的测试不算判决）。

<!-- GATE:AC -->
### AC-001
- Given: 一个装载了内嵌字体的离屏 egui 上下文，`screen_rect` 为 800×600，正文块是一段超过 40 个汉字的段落
- When: 调用正文渲染入口渲染该段落，并读取该段文字排版时的折行宽度
- Then: 该折行宽度与 `min(可用宽, 680)` 之差的绝对值小于 1

### AC-002
- Given: 与 AC-001 相同的离屏上下文，屏幕宽 800 远大于版心 680
- When: 离屏渲染一段正文并读取该段落文字排版矩形的最小 x 与宽度
- Then: 左留白与右留白（`可用宽 - 最小 x - 宽度`）之差的绝对值小于 1，即正文水平居中

### AC-003
- Given: 与 AC-001 相同的离屏上下文，正文块是一段文字紧接一张两列短表
- When: 离屏渲染并读取表头底色矩形的宽度
- Then: 该宽度与 `min(可用宽, 680)` 之差的绝对值小于 1，且表宽等于正文折行宽度

### AC-004
- Given: 与 AC-001 相同的离屏上下文，正文块是一层列表项内再嵌一层列表项
- When: 离屏渲染并读取两层列表符号文字的最小 x 坐标
- Then: 内外层符号的 x 坐标差落在 15.5 与 16.5 之间，即每深入一层缩进 16 像素

### AC-005
- Given: 与 AC-001 相同的离屏上下文，正文块是一个有序列表，首项编号为 9、次项为 10
- When: 离屏渲染并读取两枚编号文字的矩形最大 x（即编号右边缘）
- Then: 两枚编号的最大 x 之差小于 1，即同列表内各项正文起点一致

### AC-006
- Given: 一行表格单元格内容为 `` `a|b` `` 与 `c\|d`，表头为 2 列
- When: 对该行调用公开的正文解析入口
- Then: 解析出 2 个单元格，第 1 格文本为字面量 `a|b`，第 2 格文本为字面量 `c|d`

### AC-007
- Given: 一行表格的数据行为 `| T1 | 变异清单文件格式（`file|anchor|replacement|test-filter|理由`） | 2h |`，表头为 3 列
- When: 对该表格调用公开的正文解析入口
- Then: 该数据行解析出 3 个单元格，且第 3 格文本等于字面量 `2h`

### AC-008
- Given: 一行表格的某个单元格内容为 `` b`c ``（反引号不成对）
- When: 对该行调用公开的正文解析入口
- Then: 该行解析出的单元格数为 3，即不成对的反引号未被当作代码段起始

### AC-009
- Given: 一行表格的某个单元格内容为 `` `x\|y` ``（竖线已被反斜杠转义）
- When: 对该行调用公开的正文解析入口
- Then: 该行解析出的单元格数为 1，且该格文本等于字面量 `x|y`，即未被重复转义

### AC-010
- Given: 一段围栏代码块，其代码行内容为 `` | a | `x|y` | ``
- When: 对该段调用公开的正文解析入口
- Then: 解析出的代码块文本逐字等于 `` | a | `x|y` | ``，即归一化未改写围栏内的竖线

### AC-011
- Given: 一段正文，其中含 HTML 注释 `<!-- GATE:STEP name=decomposition -->`、一个指向 `https://a.tld/x` 的链接与一个图片 `![图](a.png)`
- When: 离屏渲染该正文并收集所有绘制文本
- Then: 绘制文本不含 `GATE`，且不含 `a.tld`，即注释被隐藏且链接地址未被渲染出来

### AC-012
- Given: 一段正文，含一个二级标题 `## 1. 需求分解` 与一行普通文本
- When: 对该正文调用公开的正文解析入口
- Then: 解析结果的第 1 个块是层级为 2 的标题，且第 2 个块不是列表

### AC-013
- Given: 与 AC-001 相同的离屏上下文，正文块是一段文字紧接一张两列短表
- When: 离屏渲染并取所有绘制文字的视觉矩形最大 x
- Then: 该最大 x 不大于版心宽度加 1，即单元格文字不越出版心右边缘

### AC-014
- Given: 实现完成后的 gui crate，其依赖声明中 pulldown-cmark 关闭了默认 feature
- When: 执行 `cargo tree -p req-guard-gui` 并统计 pulldown-cmark 名下的传递依赖
- Then: 其传递依赖恰为 bitflags、memchr、unicase 3 个

### AC-015
- Given: 实现完成后的 gui crate
- When: 执行 `cargo test -p req-guard-gui`
- Then: 失败数为 0，且测试总数不少于 38

### AC-016
- Given: 实现完成后的 gui crate
- When: 执行 `cargo fmt -p req-guard-gui -- --check` 与 `cargo clippy -p req-guard-gui --all-targets`
- Then: 前者无 diff 输出，后者 warning 数为 0

### AC-017
- Given: 一份未关闭预归一化器（即直接把表格原文交给 Markdown 解析库）的实现，与本清单新增的竖线用例
- When: 运行该竖线用例
- Then: 该用例失败，且末格文本不等于字面量 `2h`，证明该用例不是自证

### AC-018
- Given: 一份把版心直接取为可用宽而不设 680 上限的实现，与本清单新增的版心用例
- When: 运行该版心用例
- Then: 该用例失败，且读到的折行宽度大于 680

### AC-019
- Given: 一个装载了内嵌字体的离屏 egui 上下文，屏幕 800×600，正文块是一张两行的短表格紧接一段文字
- When: 离屏渲染该正文，并读取表格末行文字的矩形底端与该段文字的矩形顶端
- Then: 两者之差小于 30，且该段文字出现在绘制输出里，即短表格下方不留一整屏空白

### AC-020
- Given: 离屏 egui 上下文里有一个纵向滚动区与三段折叠头，第一段展开且滚动偏移为 1800
- When: 下一帧切到第二段展开、对该段标题调用一次 Response 的 scroll_to_me，再渲染一帧
- Then: 第二段标题的矩形顶端小于视口高度 600，即切段后视口落在该段开头而不是其中段
<!-- /GATE:AC -->

## 审核记录

<!-- GATE:AUDIT -->
- 2026-10-05_12:34:25 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-05_12:34:34 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
- 2026-10-05_12:34:50 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
- 2026-10-05_14:27:41 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | amended | 新增两处Bugfix
- 2026-10-05_14:41:58 | Mike_Zhu <zhuyuan2706@gmail.com> | decomposition | approved | -
- 2026-10-05_14:45:16 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | amended | 补两条验收
- 2026-10-05_14:45:40 | Mike_Zhu <zhuyuan2706@gmail.com> | testplan | approved | -
- 2026-10-05_14:47:24 | Mike_Zhu <zhuyuan2706@gmail.com> | solution | approved | -
<!-- /GATE:AUDIT -->
