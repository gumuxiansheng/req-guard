//! 清单正文的 **Markdown 渲染**：解析用 `pulldown-cmark`，渲染与安全纪律自己写。
//!
//! 为什么要渲染：审核人真正要读的是**内容**，不是 `- [ ]`、`**`、`` ` `` 这些标记。
//! 过去 GUI 把 `core::requirement::section_of` 的切片原样塞进只读文本框（等宽纯文本），
//! 段落、任务勾选框、表格全靠人脑翻译，审阅体验明显落后于编辑器里的同一份清单。
//!
//! ## 依赖边界：只借解析器，不借渲染器（REQ-013）
//!
//! 解析层换成 `pulldown-cmark`（CommonMark + 三条 GFM 扩展），因为自研解析器只覆盖
//! "够用"子集，边界写法要靠几百行状态码自己兜。**渲染与安全纪律仍在本文件里**：
//! - `egui_extras 0.36.2` **已无 markdown 模块**（实测其模块表里没有、依赖表里也没有
//!   `pulldown-cmark`），`egui_markdown` 在 crates.io 上只剩同名 0.1.0 的新包，都不引；
//! - `egui_commonmark 0.25` 的链接走 `ui.hyperlink_to` 且**无开关**，等于给 AI 留了一条
//!   "骗审核人点外链"的通道；它的表格还走 `egui::Grid`（末列吃剩余宽度，见 [`table`]）。
//!
//! ## 四条安全纪律（渲染器永不主动对外）
//!
//! 1. **永不激活链接、永不加载图片**：清单正文由 AI 生成，一个 `[点我](https://evil.tld)`
//!    若能被审核人一点就唤起浏览器，等于给 AI 留了一条骗人类点击的通道——门禁工具尤其
//!    不能开这个口子。链接 / 图片只渲染成**带下划线的纯文本**（[`rt_of`]）。
//! 2. **HTML 注释整块隐藏**：模板里的 `<!-- GATE:… -->` 是给门禁工具看的标记行，
//!    不该糊在正文里（[`escape_table_code_pipes`] 之后的解析阶段丢弃 `Event::Html`）。
//! 3. **不丢内容**：宁可显示成原文，也不能让人看不到清单里的某句话。
//! 4. **版心唯一**：段落 / 列表 / 标题 / 表格共用同一栏宽度，表格列宽也由它分配。
//!
//! 渲染范围：标题 / 段落 / 有序无序列表 / 任务列表 / 围栏代码块 / 表格 / 引用 /
//! 分隔线 + 行内的 `**粗**` `*斜*` `~~删~~` `` `码` ``。不支持的语法（脚注、定义列表）
//! 按**不渲染**处理——它们在清单里不会出现，出现时也不该冒充正文。
//!
//! ## 三条被真实缺陷教出来的纪律，改代码前先读对应注释
//!
//! - **竖线只在真的分列时才是分隔符**：代码段内的 `` `a|b` ``、转义的 `\|` 都算内容。
//!   GFM 要求表格内必须转义，而清单里大量不转义，故先过 [`escape_table_code_pipes`]；
//! - **宽度只有版心一个来源**，且版心有上限 [`MEASURE_W`]，见 [`show`] 与 [`distribute`]；
//! - **列表缩进按层级递增**：常量缩进套在每一层上，两层挤在一起就看不出层级，见 [`list_block`]。
//!
//! 分层：解析（[`parse`]，纯函数、不碰 egui、可单测）与渲染（[`show`]）分离，
//! 测试断言的是"解析出的结构"或"离屏画出的几何"，不需要跑窗口。

use crate::palette;
use eframe::egui;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::sync::Arc;

/// 块级元素。
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    List(List),
    Code { lang: String, text: String },
    Quote(Vec<Block>),
    Table(Table),
    Rule,
}

/// 列表（`ordered = false` 为无序列表）。
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    /// 有序列表的起始编号（`3. …` → 3）。
    pub start: u64,
    pub items: Vec<ListItem>,
}

/// 列表项。
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// `Some(true/false)` = GFM 任务列表项（`- [x]` / `- [ ]`）；`None` = 普通项。
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// 表格。
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub header: Vec<Vec<Inline>>,
    /// 每列对齐（由 `|:---:|---:|` 那行分隔行给出）。
    pub aligns: Vec<Align>,
    pub rows: Vec<Vec<Vec<Inline>>>,
}

/// 列对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// 行内元素。
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    /// 链接：只渲染文本，`dest` 仅作悬停提示，**不可点击**（见模块注释的安全取舍）。
    Link {
        text: Vec<Inline>,
        dest: String,
    },
    /// 图片：只渲染替代文本，**不发起任何加载**。
    Image {
        alt: String,
        dest: String,
    },
}

// ===================== 解析 =====================
// ===================== 解析 =====================

/// 解析开关：**只开清单里真实用到的三条 GFM 扩展**。
///
/// 表格（`| --- |` 分列）、任务列表（`- [x]`）、删除线（`~~废弃~~`）是 AI 写清单时最高频的三种
/// 写法；脚注 / 定义列表 / front-matter 一律不开（非目标 N6）——它们要么在清单里不会出现，
/// 要么（HTML 注释那种）本来就该隐藏而不是渲染成正文。
fn options() -> Options {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_TASKLISTS);
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o
}

/// 解析整段 Markdown 文本为块序列。
///
/// **先过一遍 [`escape_table_code_pipes`] 再交给解析器**，原因写在那个函数的注释里：
/// GFM 规定表格里的竖线必须转义（代码段内也一样），直接喂原文会被切成多列并**静默丢内容**。
pub fn parse(src: &str) -> Vec<Block> {
    Builder::default().run(&escape_table_code_pipes(src))
}

// ---------- 事件流 → AST ----------

/// 事件流 → [`Block`] 的状态机（显式栈，不递归）。
///
/// 两种"容器"必须分开推，否则表格单元格（装**行内**）与列表项（装**块**）会互相串位：
/// - [`Frame`]：装块的容器（文档 / 引用 / 列表 / 列表项 / 表格）；
/// - [`Leaf`]：装行内的栈栈（段落 / 标题 / 单元格 / 列表项正文 / 强调嵌套共用一条）。
#[derive(Default)]
struct Builder {
    frames: Vec<Frame>,
    leaves: Vec<Leaf>,
    /// 代码块累积（`Tag::CodeBlock` 期间非空）：`(语言, 原文)`。
    code: Option<(String, String)>,
}

/// 装块的容器。
enum Frame {
    Doc(Vec<Block>),
    Quote(Vec<Block>),
    List(List),
    /// 列表项：`checked` 由 `TaskListMarker` 事件补上。
    Item {
        checked: Option<bool>,
        blocks: Vec<Block>,
    },
    Table(TableFrame),
}

/// 一段行内内容，外加"它该被包成什么"。
struct Leaf {
    kind: LeafKind,
    inlines: Vec<Inline>,
}

enum LeafKind {
    /// 普通段落，以及"没有叶子时兜底新建"的叶子。
    Para,
    /// 标题（级别 1–6）。
    Heading(u8),
    /// 表格单元格。
    Cell,
    /// 列表项的直接正文：紧凑列表里 `- 甲` 是不带 `Paragraph` 包裹的。
    ItemBody,
    Emphasis,
    Strong,
    Strikethrough,
    Link(String),
    Image(String),
}

/// 表格累积状态。
struct TableFrame {
    aligns: Vec<Align>,
    head: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
    /// 当前行累积的各格。
    row: Vec<Vec<Inline>>,
    /// 当前是否在表头里（表头结束时搬进 `head`，数据行结束时搬进 `rows`）。
    in_head: bool,
}

impl TableFrame {
    fn into_table(mut self) -> Table {
        // 解析器会把缺格补成空串（实测），这里再兜一次：列数取表头与数据行的最大值，
        // 短行补空格 —— 宁可多出空列，也不要少一列把内容挤到错位。
        let cols = self
            .head
            .len()
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0));
        for row in self.rows.iter_mut() {
            while row.len() < cols {
                row.push(Vec::new());
            }
        }
        while self.head.len() < cols {
            self.head.push(Vec::new());
        }
        Table {
            header: self.head,
            aligns: self.aligns,
            rows: self.rows,
        }
    }
}

impl Builder {
    fn run(mut self, src: &str) -> Vec<Block> {
        self.frames.push(Frame::Doc(Vec::new()));
        for ev in Parser::new_ext(src, options()) {
            self.event(ev);
        }
        self.take_outer()
    }

    /// 取栈底容器的块序列作为结果。
    ///
    /// 事件流里**没有"文档结束"事件**（`TagEnd` 枚举里没有 `Document` 变体），所以收尾
    /// 只能靠取栈底 —— 正常情况下此时栈里就只剩 [`Frame::Doc`] 一个。
    fn take_outer(&mut self) -> Vec<Block> {
        match self.frames.first_mut() {
            Some(Frame::Doc(bs)) | Some(Frame::Quote(bs)) => std::mem::take(bs),
            // 栈底不是块容器：事件流被异常截断。返回空列表也不 panic ——
            // 渲染层本来就有"渲染路径不 panic"的底线用例兜着。
            _ => Vec::new(),
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(t) => self.start(t),
            Event::End(t) => self.end(t),
            Event::Text(t) => {
                // 代码块里的文本是**原文**，不经行内解析（行内代码是 `Event::Code`）。
                if let Some((_, buf)) = self.code.as_mut() {
                    buf.push_str(&t);
                    return;
                }
                self.push_text(sanitize(&t));
            }
            Event::Code(t) => self.push_inline(Inline::Code(sanitize(&t))),
            // 软换行渲染成空格：源码里的换行只是折行排版，渲染成硬换行会在段落里留下莫名空隙。
            Event::SoftBreak | Event::HardBreak => self.soft_break(),
            // HTML 注释（模板里的 `<!-- GATE:… -->` 标记行）与内联标签都**不进正文**：
            // 那些标记是给门禁工具看的，不是给审核人看的。
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item { checked, .. }) = self.frames.last_mut() {
                    *checked = Some(done);
                }
            }
            Event::Rule => self.push_block(Block::Rule),
            // 脚注 / 定义列表 / 元数据：非目标 N6，按"不渲染"处理。
            _ => {}
        }
    }

    fn start(&mut self, t: Tag<'_>) {
        match t {
            Tag::Paragraph => self.push_leaf(LeafKind::Para),
            Tag::Heading { level, .. } => self.push_leaf(LeafKind::Heading(heading_level(level))),
            // 这四种都是**块**：开始之前先把列表项的直接正文（`- 外` 里的"外"）落成段落，
            // 否则它会被挤到嵌套块后面（渲染时"外"就跑到了子列表下面，缩进也不对）。
            Tag::BlockQuote(_) => {
                self.flush_item_text();
                self.frames.push(Frame::Quote(Vec::new()));
            }
            Tag::CodeBlock(k) => {
                self.flush_item_text();
                self.code = Some((code_lang(k), String::new()));
            }
            Tag::List(start) => {
                self.flush_item_text();
                self.frames.push(Frame::List(List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                }))
            }
            Tag::Item => {
                self.push_leaf(LeafKind::ItemBody);
                self.frames.push(Frame::Item {
                    checked: None,
                    blocks: Vec::new(),
                });
            }
            Tag::Table(aligns) => {
                self.flush_item_text();
                self.frames.push(Frame::Table(TableFrame {
                    aligns: aligns.into_iter().map(align_of_cmark).collect(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    in_head: false,
                }));
            }
            Tag::TableHead => {
                if let Some(Frame::Table(t)) = self.frames.last_mut() {
                    t.in_head = true;
                    t.row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(Frame::Table(t)) = self.frames.last_mut() {
                    t.row.clear();
                }
            }
            Tag::TableCell => self.push_leaf(LeafKind::Cell),
            Tag::Emphasis => self.push_leaf(LeafKind::Emphasis),
            Tag::Strong => self.push_leaf(LeafKind::Strong),
            Tag::Strikethrough => self.push_leaf(LeafKind::Strikethrough),
            Tag::Link { dest_url, .. } => self.push_leaf(LeafKind::Link(sanitize(&dest_url))),
            Tag::Image { dest_url, .. } => self.push_leaf(LeafKind::Image(sanitize(&dest_url))),
            // 整块 HTML / 元数据：正文里不该出现，直接丢。
            Tag::HtmlBlock | Tag::MetadataBlock(_) => {}
            _ => {}
        }
    }

    fn end(&mut self, t: TagEnd) {
        match t {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.finish_leaf(),
            TagEnd::BlockQuote(_) => {
                // 引用里若直接是文字（没有 `Paragraph` 包裹），先补成段落再收块。
                self.finish_leaf();
                if let Some(Frame::Quote(bs)) = self.frames.pop() {
                    self.push_block(Block::Quote(bs));
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, text)) = self.code.take() {
                    self.push_block(Block::Code { lang, text });
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame::List(l)) = self.frames.pop() {
                    self.push_block(Block::List(l));
                }
            }
            TagEnd::Item => self.finish_item(),
            TagEnd::TableCell => self.finish_cell(),
            TagEnd::TableHead => {
                if let Some(Frame::Table(t)) = self.frames.last_mut() {
                    t.head = std::mem::take(&mut t.row);
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table(t)) = self.frames.last_mut() {
                    let row = std::mem::take(&mut t.row);
                    if !row.is_empty() {
                        t.rows.push(row);
                    }
                }
            }
            TagEnd::Table => {
                if let Some(Frame::Table(t)) = self.frames.pop() {
                    self.push_block(Block::Table(t.into_table()));
                }
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => self.finish_inline_leaf(),
            _ => {}
        }
    }

    /// 块挂到**当前容器**（栈顶 frame）。
    fn push_block(&mut self, b: Block) {
        match self.frames.last_mut() {
            Some(Frame::Doc(bs) | Frame::Quote(bs)) => bs.push(b),
            Some(Frame::Item { blocks, .. }) => blocks.push(b),
            // 表格与列表容器里不会出现裸块：解析器保证单元格只装行内、列表只装列表项。
            // 走到这里说明上游事件流变了 —— 宁可丢一块，也不要挂到错位置造成静默错乱。
            Some(Frame::Table(_) | Frame::List(_)) | None => {}
        }
    }

    fn push_leaf(&mut self, kind: LeafKind) {
        self.leaves.push(Leaf {
            kind,
            inlines: Vec::new(),
        });
    }

    /// 行内元素挂到**当前叶子**；没有叶子时兜底开一个段落叶子（宁可多一段，也不丢字）。
    fn push_inline(&mut self, i: Inline) {
        if self.leaves.is_empty() {
            self.push_leaf(LeafKind::Para);
        }
        if let Some(leaf) = self.leaves.last_mut() {
            leaf.inlines.push(i);
        }
    }

    fn push_text(&mut self, s: String) {
        if !s.is_empty() {
            self.push_inline(Inline::Text(s));
        }
    }

    /// 换行补一个空格，但**不叠出两个**：`- 甲\n  续行` 之类紧跟着的行已经带前导空白。
    fn soft_break(&mut self) {
        let trailing = self
            .leaves
            .last()
            .is_some_and(|l| matches!(l.inlines.last(), Some(Inline::Text(t)) if t.ends_with(' ')));
        if !trailing {
            self.push_text(" ".to_string());
        }
    }

    /// 列表项的直接正文在**嵌套块之前**先落成段落。
    ///
    /// 紧凑列表里 `- 外` 的"外"是不带 `Paragraph` 包裹的行内（[`LeafKind::ItemBody`]），
    /// 一旦项里还有嵌套块（`- 外\n  - 内`），不先冲刷的话嵌套列表会先进块序列、
    /// 段落被挤到它后面 —— 渲染时"外"就落到了子列表下面，缩进与行序一起错。
    fn flush_item_text(&mut self) {
        let pending = matches!(self.frames.last(), Some(Frame::Item { .. }))
            && self
                .leaves
                .last()
                .is_some_and(|l| matches!(l.kind, LeafKind::ItemBody) && !l.inlines.is_empty());
        if pending {
            self.finish_leaf();
        }
    }

    /// 结束块级叶子（段落 / 标题）：内容落成块。
    fn finish_leaf(&mut self) {
        let Some(leaf) = self.leaves.pop() else {
            return;
        };
        if leaf.inlines.is_empty() {
            return;
        }
        match leaf.kind {
            LeafKind::Heading(level) => self.push_block(Block::Heading {
                level,
                inlines: leaf.inlines,
            }),
            _ => self.push_block(Block::Paragraph(leaf.inlines)),
        }
    }

    /// 结束行内叶子（强调 / 链接 / 图片）：内容包成对应 `Inline` 塞回上一层。
    fn finish_inline_leaf(&mut self) {
        let Some(leaf) = self.leaves.pop() else {
            return;
        };
        let wrapped = match leaf.kind {
            LeafKind::Emphasis => Inline::Emphasis(leaf.inlines),
            LeafKind::Strong => Inline::Strong(leaf.inlines),
            LeafKind::Strikethrough => Inline::Strikethrough(leaf.inlines),
            LeafKind::Link(dest) => Inline::Link {
                text: leaf.inlines,
                dest,
            },
            LeafKind::Image(dest) => Inline::Image {
                alt: inline_text(&leaf.inlines),
                dest,
            },
            // 段落 / 标题 / 单元格 / 列表项正文不走这里（它们落成块，见 `finish_leaf`）。
            // 万一走到（例如上游改了事件顺序），拍平成文本而不是丢字。
            LeafKind::Para | LeafKind::Heading(_) | LeafKind::Cell | LeafKind::ItemBody => {
                Inline::Text(inline_text(&leaf.inlines))
            }
        };
        self.push_inline(wrapped);
    }

    /// 结束表格单元格：本格行内挂到当前行。
    fn finish_cell(&mut self) {
        let Some(leaf) = self.leaves.pop() else {
            return;
        };
        if let Some(Frame::Table(t)) = self.frames.last_mut() {
            t.row.push(leaf.inlines);
        }
    }

    /// 结束列表项：直接正文（紧凑列表没有 `Paragraph` 包裹）先补成段落，再挂回所属列表。
    fn finish_item(&mut self) {
        self.finish_leaf();
        let Some(Frame::Item { checked, blocks }) = self.frames.pop() else {
            return;
        };
        match self.frames.last_mut() {
            Some(Frame::List(l)) => l.items.push(ListItem { checked, blocks }),
            // 列表项没有所属列表：解析器保证不会发生，兜成单项列表而不是丢内容。
            _ => self.push_block(Block::List(List {
                ordered: false,
                start: 1,
                items: vec![ListItem { checked, blocks }],
            })),
        }
    }
}

fn heading_level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// 代码块语言：围栏取 ``` 后的标记，缩进代码块没有语言。
fn code_lang(k: CodeBlockKind) -> String {
    match k {
        CodeBlockKind::Fenced(l) => sanitize(&l),
        CodeBlockKind::Indented => String::new(),
    }
}

fn align_of_cmark(a: Alignment) -> Align {
    match a {
        // GFM 的"未指定对齐"与左对齐在排版上等价（解析器实测给的就是 `Alignment::None`）。
        Alignment::None | Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

/// 解析器给回的文本 → 可显示文本：字形降级 + 去掉变体选择符。
fn sanitize(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            // VS16（U+FE0F）只在 emoji 后面成对出现，去掉它、保留被替换后的符号。
            '\u{fe0f}' => None,
            other => Some(fallback_glyph(other).unwrap_or(other)),
        })
        .collect()
}

// ---------- 表格竖线预归一化 ----------

/// 把**表格行**里、代码段中、未转义的裸竖线补上反斜杠（GFM 表格转义）。
///
/// **为什么必须有这一步**：GFM 规定表格里的竖线必须转义，**代码段内也不例外**，
/// `pulldown-cmark` 严格照办。清单里"字段用 `file|anchor|replacement` 分隔"这类写法极常见，
/// 直接喂原文会被切成多列，而且**多出来的列会被静默丢弃**（实测：REQ-011 那条验收里的原例
/// 被切成 3 格，`repl` 之后的内容与 `2h` 一起消失）。"看不见"比"难看"严重得多。
/// 表格单元格内反斜杠转义会被解析器解开，所以补了转义之后显示结果与原文一致。
///
/// **只动表格行**：普通段落里的 `` `a|b` `` 若补转义，内容就真的变了——代码段内反斜杠
/// 不生效（实测：`` `a\|b` `` 会原样显示 `a\|b`），等于把对的写成错的。
///
/// 判"表格行"两条：① 本行的下一行是分隔行（`| --- |`）且**格数与本行相同**（格数按
/// [`cells_of`] 统计，与解析器口径一致，可挡掉"普通段落里恰好有 `|` 且下一行像分隔行"）；
/// ② 表格块内后续形如表格的数据行，直到空行或非表格行。围栏代码块内的行一律不动。
///
/// 已知边界：跨行代码段（软换行把一段代码断开）不会被识别，该行按 GFM 照常切列——清单里
/// 不这么写，且这种错是"能看见"的错（列多了一列），不会静默丢内容。
fn escape_table_code_pipes(src: &str) -> String {
    // 用 `split('\n')` 而不是 `lines()`：后者会把 CRLF 归一成 LF，等于顺手改了正文。
    let lines: Vec<&str> = src.split('\n').collect();
    let mut is_table = vec![false; lines.len()];
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        if fence_of(lines[i].trim_start()).is_some() {
            in_fence = !in_fence;
            i += 1;
            continue;
        }
        let header_of_table = !in_fence
            && lines.get(i + 1).is_some_and(|n| {
                let n = n.trim();
                is_delimiter_row(n) && cells_of(n).len() == cells_of(lines[i].trim()).len()
            });
        if header_of_table {
            is_table[i] = true;
            let mut j = i + 2;
            while j < lines.len() && is_table_row(lines[j].trim()) {
                is_table[j] = true;
                j += 1;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    if !is_table.iter().any(|b| *b) {
        // 绝大多数正文没有表格，别为它重建整篇。
        return src.to_string();
    }
    lines
        .iter()
        .enumerate()
        .map(|(k, l)| {
            if is_table[k] {
                escape_code_span_pipes(l)
            } else {
                (*l).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 单行内把**代码段中未转义的裸竖线**加上反斜杠，其余字符一字不动。
///
/// 等长反引号才闭合（GFM）：`` `a``b` `` 里的 `` `` `` 不该结束单个反引号开头的代码段；
/// 不成对时按普通字符处理（fail-closed 到"照常切列"）。
fn escape_code_span_pipes(line: &str) -> String {
    let c: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < c.len() {
        // 已有转义对整体进格（`\|` 不再重复加反斜杠）。
        if c[i] == '\\' && i + 1 < c.len() {
            out.push(c[i]);
            out.push(c[i + 1]);
            i += 2;
            continue;
        }
        if c[i] == '`' {
            let n = tick_run(&c, i);
            match tick_close(&c, i + n, n) {
                Some(end) => {
                    for k in i..=end {
                        if c[k] == '|' && (k == 0 || c[k - 1] != '\\') {
                            out.push('\\');
                        }
                        out.push(c[k]);
                    }
                    i = end + n;
                    continue;
                }
                None => {
                    out.push(c[i]);
                    i += 1;
                    continue;
                }
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

// ---------- 表格行判定（GFM 口径） ----------

fn fence_of(t: &str) -> Option<&'static str> {
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// `---` / `***` / `___`（允许中间有空格，长度 ≥ 3）。
fn is_rule(t: &str) -> bool {
    let s: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    s.len() >= 3 && matches!(s[0], '-' | '*' | '_') && s.iter().all(|&c| c == s[0])
}

fn heading_of(t: &str) -> Option<(u8, &str)> {
    let level = t.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    t[level..]
        .strip_prefix(' ')
        .map(|rest| (level as u8, rest.trim()))
}

/// 表格的对齐分隔行：`| --- | :---: |`（首尾竖线可省）。
fn is_delimiter_row(t: &str) -> bool {
    let cells = cells_of(t);
    cells.len() >= 2
        && cells.iter().all(|c| {
            let c = c.trim();
            !c.is_empty() && c.contains('-') && c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
}

/// 表格的数据行：含 `|`，且不是分隔线 / 标题（首尾竖线同样可省）。
fn is_table_row(t: &str) -> bool {
    t.contains('|') && !is_rule(t) && heading_of(t).is_none()
}

/// 拆分表格行：去掉首尾竖线后按 `|` 切，逐格 trim。
///
/// 现在它只服务于**表格行的判定**（[`escape_table_code_pipes`] 要拿格数与分隔行比对），
/// 不再直接产出单元格 —— 单元格由解析器给。
///
/// **`|` 不是见到就切**。两种竖线必须放过，否则 AI 写的表格会被切碎：
/// - 反斜杠转义的 `\|`（GFM 明确支持把竖线写进单元格里）；
/// - 反引号代码段里的竖线 —— 清单里"字段用 `file|anchor|replacement` 分隔"这类行
///   几乎都这么写，按列切开就变成多出好几列、内容整体错位。
fn cells_of(t: &str) -> Vec<String> {
    let t = t.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let cs: Vec<char> = t.chars().collect();
    let mut cells = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' if i + 1 < cs.len() => {
                // 转义对：两个字符一起进本格。
                buf.push('\\');
                buf.push(cs[i + 1]);
                i += 2;
            }
            '`' => {
                let n = tick_run(&cs, i);
                match tick_close(&cs, i + n, n) {
                    // 有等长闭合 → 整段代码原样进本格（段内竖线不切）。
                    Some(end) => {
                        buf.extend(&cs[i..end + n]);
                        i = end + n;
                    }
                    // 没有等长闭合 → 这几个反引号只是普通字符（GFM：不成对就不是代码段）。
                    None => {
                        buf.push('`');
                        i += 1;
                    }
                }
            }
            '|' => {
                cells.push(buf.trim().to_string());
                buf.clear();
                i += 1;
            }
            c => {
                buf.push(c);
                i += 1;
            }
        }
    }
    cells.push(buf.trim().to_string());
    cells
}

/// 从 `i` 起连续反引号的个数。
fn tick_run(c: &[char], i: usize) -> usize {
    1 + c[i + 1..].iter().take_while(|x| **x == '`').count()
}

/// 从 `start` 起找**恰好 n 个**反引号的闭合段，返回其起始下标。
///
/// 长度不同的反引号串不算闭合（GFM 代码段按等长成对闭合）：`` `a``b` `` 里的
/// `` `` `` 不该结束单个反引号开头的代码段。
fn tick_close(c: &[char], start: usize, n: usize) -> Option<usize> {
    let mut i = start;
    while i < c.len() {
        if c[i] == '`' {
            let run = tick_run(c, i);
            if run == n {
                return Some(i);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

// ---------- 字形降级 ----------

/// 内嵌字体缺字形时的**降级替换**：AI 写的清单很爱用 `✅` / `❌` / `⭐`，
/// 而 Noto Sans SC 没有这些 emoji 的字形（emoji 在 Noto Color Emoji 里，本项目没内嵌），
/// 不降级就会在界面上显示成 `?`（豆腐块），审核人看到的是" inexplicable 的问号"。
/// 换成字体里有的等义单字符（`✓ ✗ ⚠ ★` 都在子集范围内）——信息不丢，也不必为几个符号再内嵌一份字体。
fn fallback_glyph(c: char) -> Option<char> {
    match c {
        '✅' | '☑' | '✔' => Some('\u{2713}'),  // ✓
        '❌' | '✖' | '❎' => Some('\u{2717}'), // ✗
        '⭐' => Some('\u{2605}'),              // ★
        _ => None,
    }
}

/// 行内结构的纯文本化（渲染标题文本 / 测试断言 / 调试都用它）。
pub fn inline_text(v: &[Inline]) -> String {
    let mut s = String::new();
    for i in v {
        match i {
            Inline::Text(t) | Inline::Code(t) => s.push_str(t),
            Inline::Strong(c) | Inline::Emphasis(c) | Inline::Strikethrough(c) => {
                s.push_str(&inline_text(c))
            }
            Inline::Link { text, .. } => s.push_str(&inline_text(text)),
            Inline::Image { alt, .. } => s.push_str(alt),
        }
    }
    s
}
/// 渲染缓存：同一段、同一份原文只解析一次。
///
/// egui 是即时模式、每帧重画：不清缓存就得每帧重跑一遍解析（还要反复克隆原文）。
/// 键是 `(段下标, 原文)`——两者都相同才复用，正文一改立刻失效，不会渲染出上一份内容。
#[derive(Default)]
pub struct Cache(Option<(usize, String, Vec<Block>)>);

impl Cache {
    /// 取第 `step` 段的解析结果，必要时重新解析。
    pub fn get(&mut self, step: usize, text: &str) -> &[Block] {
        let fresh = self
            .0
            .as_ref()
            .is_some_and(|(s, src, _)| *s == step && src == text);
        if !fresh {
            self.0 = Some((step, text.to_string(), parse(text)));
        }
        &self.0.as_ref().expect("上一行刚写入").2
    }

    /// 作废（切换需求 / 重新读盘正文后调用）。
    pub fn clear(&mut self) {
        self.0 = None;
    }
}

// ===================== 渲染 =====================

/// 渲染块序列（垂直流式排布；纵向滚动交给外层 `ScrollArea`）。
///
/// **版心宽度是整篇正文唯一的宽度基准**，这里同时是它**唯一的收口处**：
/// 版心 = `min(可用宽, MEASURE_W)`，再交给一个"限宽 + 左对齐 + 整体居中"的子 ui 往下传。
/// 往下每一层只能是"上层宽度减去一个固定量"（引用块的边框留白、列表符号的缩进），
/// 不能有哪一层自己另问一个宽度——那样同段里就会出现"这段行数多、那段行数少"。
/// 两处最容易破这条的地方：列表（一项一行，见 [`list_block`]）与表格
/// （列宽由版心分配，见 [`table`]）。
///
/// **为什么收口在入口而不是各块自己算**：子 ui 的 rect 就是版心，于是段落折行、表格列宽分配、
/// 代码块滚动区读到的 `available_width()` 全都自动变成同一个版心——一处改动全局收敛，
/// 也不会有哪块"忘了"上限而把行拉宽。
pub fn show(ui: &mut egui::Ui, blocks: &[Block]) {
    // 正文允许框选复制：审核人常要摘一段回评论里，渲染不能把这条路堵死。
    ui.style_mut().interaction.selectable_labels = true;
    let avail = ui.available_rect_before_wrap();
    let measure = avail.width().min(MEASURE_W);
    // **摆正的是"版心这一块"，不是每个子控件**。所以不用 `vertical_centered`：
    // 它把每个子控件在**自己的可用宽度里**居中，于是"符号 + 短标签"这种窄行会被推到
    // 版心中间——符号的 x 随文字宽度变，列表缩进与"同列表各项正文起点一致"当场作废。
    //
    // ⚠ **必须用 `scope_builder`，不能换成 `new_child`**（都实测过，见单测
    // `在滚动区里不遮挡后续控件_且内容高度可滚动`）：
    // `new_child` 把子 ui 摆到 `max_rect` 上却**不推进父游标**，于是正文会压住后面的控件
    // （下一段的标题与按钮），且外层 `ScrollArea` 量到的内容高度只有视口高 → 整篇滚不动。
    // `scope_builder` = `new_child` + `advance_cursor_after_rect(child.min_rect())`：
    // 父游标按子 ui **实际内容高**推进，滚动区才量得出真实高度。
    //
    // 高度取 `avail.max.y` 而不是写死高度：滚动区里它是无限高，正文多长就长多高。
    let x0 = avail.min.x + (avail.width() - measure) / 2.0;
    let rect = egui::Rect::from_min_max(
        egui::pos2(x0, avail.min.y),
        egui::pos2(x0 + measure, avail.max.y),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| show_at(ui, blocks, 0),
    );
}

/// 按列表嵌套层级渲染块序列（`depth = 0` 是最外层）。
///
fn show_at(ui: &mut egui::Ui, blocks: &[Block], depth: usize) {
    for (i, b) in blocks.iter().enumerate() {
        // 每块一个 id 作用域：同段里出现多个表格 / 勾选框也不会互相串 id。
        // 键带上层级：嵌套列表与外层列表的下标会重号，只用下标会串到同一块去。
        ui.push_id((depth, i), |ui| {
            if i > 0 {
                ui.add_space(gap_before(b));
            }
            block(ui, b, depth);
            // 标题自带"后间距"，与"块间距"分开：标题后面留得多一点，标题就成了"小节"。
            if matches!(b, Block::Heading { .. }) {
                ui.add_space(HEADING_GAP_AFTER);
            }
        });
    }
}

/// 块间距：标题前给大一些，让标题与小节内容脱开。
fn gap_before(b: &Block) -> f32 {
    match b {
        Block::Heading { .. } => HEADING_GAP_BEFORE,
        _ => BLOCK_GAP,
    }
}

fn block(ui: &mut egui::Ui, b: &Block, depth: usize) {
    match b {
        Block::Heading { level, inlines } => {
            // 层级靠**字号 + 明度**两档区分：1–2 级是正文色（视觉重心在前面），
            // 3–4 级用弱化色（`Tone::Muted` 浅底 6.63 / 深底 6.04，均过 WCAG AA，见 palette）。
            // 走 `inline_job` 而不是拍平成纯文本，是为了让标题里的 `**粗**` 与 `` `码` `` 留住样式。
            let size = heading_size(*level);
            let deco = |rt: egui::RichText| {
                let rt = rt.size(size).strong();
                if *level >= 3 {
                    rt.color(palette::Tone::Muted.color(ui))
                } else {
                    rt
                }
            };
            let style = ui.style().clone();
            let w = ui.available_width();
            ui.label(inline_job(ui, inlines, w, &style, deco));
        }
        Block::Paragraph(inlines) => {
            if !inlines.is_empty() {
                inline_label(ui, inlines);
            }
        }
        Block::List(list) => list_block(ui, list, depth),
        Block::Code { lang, text } => code_block(ui, lang, text),
        Block::Quote(inner) => {
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .corner_radius(4)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                // 引用不改变列表层级：引用块只是缩进 + 边框，不是另一层列表。
                .show(ui, |ui| show_at(ui, inner, depth));
        }
        Block::Table(t) => table(ui, t),
        Block::Rule => {
            ui.separator();
        }
    }
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 22.0,
        2 => 19.0,
        3 => 17.0,
        _ => 15.0,
    }
}

/// 列表：**一项一行**。
///
/// 用 `ui.horizontal_top` 而不是 `ui.horizontal_wrapped`：项内文字由内层 `ui.vertical`
/// 自己折行，外层并不需要 wrap 布局；而 wrap 布局会把"这一行还剩多少宽度"掺进折行与
/// 对齐（egui 的 `Label` 在 wrap 布局里还会额外按首行缩进重排）—— 万一两项被排到同一行，
/// 第二项的折行宽度就成了"版心 − 上一项宽度"，同一列表里每行字数不一样。
/// 规则写死成一行一项，这个自由度就不要了（宽度基准见 [`show`]）。
///
/// **缩进按层级递增**（符号列落在 `LIST_INDENT_W * (depth + 1)`）：早先每一层都只加同一个
/// 常量，于是"外层项"与"内层项"的符号挤在同一列，看不出层级。
///
/// **有序编号按本列表最大编号位数右对齐**（画在定宽符号列里，见 [`LIST_SYMBOL_W`]）：
/// 否则 `9.` 与 `10.` 宽度差一个字符，同一列表里每项的正文起点都在动（跨过 9 的那一项
/// 整列文字会横向跳一下）。
///
/// **嵌套项的其余块另起一行、从行左边缘重新算缩进**（`ui.indent` 那段）：早先把嵌套列表排进
/// 父项的**正文列**里，于是每层的缩进都叠在父列偏移上（16 变成"16 + 符号宽 + 列间距"），
/// 层级步长不匀、第���层的正文还会比父层的符号还靠右。现在第 `depth` 层的符号列一律落在
/// `LIST_INDENT_W * (depth + 1)`：每深入一层正好右移一个 [`LIST_INDENT_W`]。
fn list_block(ui: &mut egui::Ui, list: &List, depth: usize) {
    // 编号位数（含小数点）取本列表最大编号的宽度。
    let num_w = list
        .items
        .len()
        .saturating_add(list.start.saturating_sub(1) as usize)
        .max(1)
        .to_string()
        .len()
        + 1;
    let row_h = ui.spacing().interact_size.y;
    for (i, item) in list.items.iter().enumerate() {
        // 首段（紧凑项）与符号同一行；其余块另起，见下面 `ui.indent` 那段。
        let (first, rest): (Option<&Vec<Inline>>, &[Block]) = match item.blocks.as_slice() {
            [Block::Paragraph(v)] => (Some(v), &[]),
            [Block::Paragraph(v), tail @ ..] => (Some(v), tail),
            other => (None, other),
        };
        ui.horizontal_top(|ui| {
            ui.add_space(LIST_INDENT_W * (depth + 1) as f32);
            match item.checked {
                // 任务项渲染成灰色只读勾选框：`[x]` 已完成 / `[ ]` 未完成。
                // 只读是刻意的：正文由 AI / 编辑器维护，界面只做审核决策，不代改清单。
                Some(done) => {
                    let mut v = done;
                    ui.add_enabled(false, egui::Checkbox::new(&mut v, ""))
                        .on_hover_text(if done {
                            "已勾选（清单由 AI / 编辑器维护，界面只读）"
                        } else {
                            "未勾选（清单由 AI / 编辑器维护，界面只读）"
                        });
                }
                None => {
                    let text = if list.ordered {
                        format!("{:>num_w$}.", list.start + i as u64)
                    } else {
                        "\u{2022}".to_string()
                    };
                    symbol(ui, &text, row_h);
                }
            }
            // 首段必须放进 `ui.vertical`：`horizontal_top` 是横向布局，
            // 那里 `available_width()` 是 inf，直接 label 就**不折行**了（会横向溢出）。
            ui.vertical(|ui| {
                if let Some(v) = first {
                    if !v.is_empty() {
                        inline_label(ui, v);
                    }
                }
            });
        });
        if rest.is_empty() {
            continue;
        }
        // 其余块（嵌套列表 / 表格 / 代码块 / 副标题）另起一行。
        // **嵌套列表自己已经按层级缩进**（`depth + 1` 级），这里再 `ui.indent` 就叠成两级；
        // 没有列表可缩进的块（表格 / 代码块 / 副标题）才统一进一级。
        if rest.iter().all(|b| matches!(b, Block::List(_))) {
            show_at(ui, rest, depth + 1);
        } else {
            // `ui.indent` 读的是**调用前**的 `spacing().indent`，所以必须先改后调、再还原。
            let saved = ui.spacing_mut().indent;
            ui.spacing_mut().indent = LIST_INDENT_W;
            ui.indent(("li", depth, i), |ui| show_at(ui, rest, depth + 1));
            ui.spacing_mut().indent = saved;
        }
    }
}

/// 在**定宽**符号列里画符号 / 编号（编号右对齐），让正文起点只由列宽决定。
///
/// 不用 `ui.label`：标签按文字自然宽度排，`9.` 与 `10.` 就会把后面的正文推到不同起点。
fn symbol(ui: &mut egui::Ui, text: &str, row_h: f32) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, ui.visuals().text_color());
    let (_id, rect) = ui.allocate_space(egui::vec2(LIST_SYMBOL_W, row_h));
    let x = rect.min.x + (LIST_SYMBOL_W - galley.size().x).max(0.0);
    let y = rect.min.y + (row_h - galley.size().y).max(0.0) / 2.0;
    ui.painter()
        .galley(egui::pos2(x, y), galley, ui.visuals().text_color());
}

fn code_block(ui: &mut egui::Ui, lang: &str, text: &str) {
    egui::Frame::new()
        .fill(ui.visuals().code_bg_color)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(4)
        .show(ui, |ui| {
            if !lang.is_empty() {
                ui.label(
                    egui::RichText::new(lang)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            }
            // 横向滚动 + 不折行：折行会把代码的缩进结构搅乱（层级对不上）。
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.label(egui::RichText::new(text).monospace());
            });
        });
}

/// 表格：**先量后排**——每格先用 egui 的排版器出一份"不折行"的 `Galley` 量自然宽，
/// 把自然宽按比例摊到**版心宽度**上得到列宽（见 [`distribute`]），再把放不下的格子
/// 按列宽折行排第二遍，最后把每个 `Galley` 贴到该列的固定 `x` 上。
///
/// 这块踩过三个坑，都写在这儿免得再犯：
/// 1. `egui::Grid` 的 `num_columns` 会让**最后一列吃掉剩余宽度**——右对齐的「风险 / 工期」
///    被甩到面板最右，表头与数值之间一大段空白，看着像表格坏掉；
///    加一个空占位列能挡住，但那样又**没法给列设软上限**（设了之后每格矩形都被撑到上限宽，
///    右对齐又飘到上限外，等于换个坑再踩一遍）。
/// 2. 自己按"中文 1 字宽、ASCII 0.55 字宽"估列宽：估偏小文字就溢到隔壁列，
///    偏大右对齐就飘走；粗体表头、全角标点更是没法算准。
/// 3. `ui.allocate_ui_with_layout(vec2(w, 0), ..)` **不是固定宽**：
///    它给子 ui 的 `max_rect` 是 w，但父游标是按子 ui 的 `min_rect`（= 内容实际宽度）推进的，
///    于是每一行的列起点都跟着自己的内容跑——表头与数据反而错位（这正是"不像表格"的成因）。
///
/// 宽度这条单独记一笔（GUI 上"表格忽宽忽窄、每行字数忽多忽少"就是它）：
/// **表宽只能由版心宽度决定，不能由内容决定。** 早先列宽上限写死 300px、整表又按内容收缩，
/// 于是两列短表只有 200px 宽、三列长表顶到面板边缘还带横向滚动条——同一篇正文里表格
/// 一会儿贴左、一会儿满宽，单元格每行能显示的字数也跟着变。现在列宽 = 自然宽按比例摊到
/// 版心宽度上：表与正文同宽、右边缘对齐，放不下时才压缩（压缩优先短列，长列折行）。
///
/// 代价：单元格文字是直接画的，**不能框选复制**——要整段复制切顶栏「原文」视图即可。
fn table(ui: &mut egui::Ui, t: &Table) {
    let cols = t
        .header
        .len()
        .max(t.rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return;
    }
    let style = ui.style().clone();

    // ---- 1) 逐格按"不折行"排一遍，量出各列自然宽 ----
    // `Painter::layout_job` 走 egui 的 galley 缓存，同样的 job 每帧只真排一次，代价可忽略。
    let head_nat: Vec<Arc<egui::Galley>> = (0..cols)
        .map(|i| cell_galley(ui, t.header.get(i), true, &style, f32::INFINITY))
        .collect();
    let rows_nat: Vec<Vec<Arc<egui::Galley>>> = t
        .rows
        .iter()
        .map(|r| {
            (0..cols)
                .map(|i| cell_galley(ui, r.get(i), false, &style, f32::INFINITY))
                .collect()
        })
        .collect();

    // ---- 2) 列宽 = 自然宽按比例摊到版心宽度上 ----
    let spacing = ui.spacing().item_spacing.x;
    let gaps = spacing * cols.saturating_sub(1) as f32;
    // 表头是粗体、必须按粗体量（细体量出来的宽度偏小，标题会被挤出列外）。
    let natural: Vec<f32> = (0..cols)
        .map(|i| {
            std::iter::once(&head_nat[i])
                .chain(rows_nat.iter().map(|r| &r[i]))
                .map(|g| g.size().x)
                .fold(0.0f32, f32::max)
                + CELL_PAD_X * 2.0
        })
        .collect();
    let widths = distribute(
        &natural,
        (ui.available_width() - gaps).max(MIN_COL_W),
        MIN_COL_W,
    );
    let total_w: f32 = widths.iter().sum::<f32>() + gaps;

    // ---- 3) 定稿：列宽放得下的格子沿用自然宽那份（不白排两遍），放不下的按列宽折行 ----
    let header_cells: Vec<Cell> = (0..cols)
        .map(|i| {
            fit_cell(&head_nat[i], widths[i], |w| {
                cell_galley(ui, t.header.get(i), true, &style, w)
            })
        })
        .collect();
    let data_rows: Vec<Vec<Cell>> = t
        .rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            (0..cols)
                .map(|i| {
                    fit_cell(&rows_nat[r][i], widths[i], |w| {
                        cell_galley(ui, row.get(i), false, &style, w)
                    })
                })
                .collect()
        })
        .collect();

    // ---- 4) 画：每行先按精确宽度占位，再把各格贴到本列的对齐位置 ----
    egui::ScrollArea::horizontal()
        .id_salt(ui.id().with("md_table_scroll"))
        // ⚠ **`auto_shrink` 的纵向必须是 `true`**（高度跟着内容走）。
        // 早先写 `[true, false]`：本 ScrollArea 没开纵向滚动，而 egui 里
        // `方向未开 + auto_shrink=false` 对应 `inner_size.y = max(可用高, 内容高)`
        // —— 短表格也会**占满整屏高度**，其后的段落被顶到视口外（实测：整屏空白，
        // 滚下去才看见下一段）。
        .auto_shrink([true, true])
        .show(ui, |ui| {
            ui.set_min_width(total_w);
            let rows =
                std::iter::once((true, &header_cells)).chain(data_rows.iter().map(|r| (false, r)));
            for (row_no, (header, cells)) in rows.enumerate() {
                let row_h = cells
                    .iter()
                    .map(|c| c.galley().size().y)
                    .fold(0.0f32, f32::max)
                    + CELL_PAD_Y * 2.0;
                let (_id, rect) = ui.allocate_space(egui::vec2(total_w, row_h));
                paint_row_bg(ui, rect, header, row_no);
                let mut x = rect.min.x;
                for (i, c) in cells.iter().enumerate() {
                    let w = widths[i];
                    let g = c.galley();
                    let gw = g.size().x;
                    let gx = match align_of(t, i) {
                        Align::Left => x + CELL_PAD_X,
                        Align::Center => x + (w - gw) / 2.0,
                        Align::Right => x + w - gw - CELL_PAD_X,
                    };
                    let gy = rect.min.y + (row_h - g.size().y) / 2.0;
                    ui.painter()
                        .galley(egui::pos2(gx, gy), g.clone(), ui.visuals().text_color());
                    x += w + spacing;
                }
            }
        });
}

/// 单元格最终用哪份排版：列宽够宽就用不折行那份，放不下才折行。
enum Cell {
    /// 不折行的排版（列宽放得下时直接用它，省一次重排）。
    Plain(Arc<egui::Galley>),
    /// 按列宽折行后的排版。
    Wrapped(Arc<egui::Galley>),
}

impl Cell {
    fn galley(&self) -> &Arc<egui::Galley> {
        match self {
            Cell::Plain(g) | Cell::Wrapped(g) => g,
        }
    }
}

fn fit_cell(
    natural: &Arc<egui::Galley>,
    col_w: f32,
    layout: impl FnOnce(f32) -> Arc<egui::Galley>,
) -> Cell {
    let w = col_w - CELL_PAD_X * 2.0;
    if natural.size().x <= w {
        Cell::Plain(natural.clone())
    } else {
        Cell::Wrapped(layout(w))
    }
}

/// 把各列自然宽按比例摊成 `target` 总宽（减去列间距后的可用宽度）。
///
/// - 自然宽合计 ≤ `target`（表比版心窄）：按比例放大到**刚好填满版心**。
///   表与正文同宽、右边缘对齐，视线不会因为"这张表只占半屏"而横向跳。
/// - 合计 > `target`（表比版心宽）：按比例压缩，压到 `min_w` 的列就钉在下限，
///   剩下的预算在还能压的列里再按比例分——**短列先保住完整内容，长列去折行**。
/// - 连下限都放不下（列多且面板窄）：每列都是 `min_w`，此时表比面板还宽，
///   交给横向滚动条，别把字挤成一列竖条。
fn distribute(natural: &[f32], target: f32, min_w: f32) -> Vec<f32> {
    if natural.is_empty() {
        return Vec::new();
    }
    let sum: f32 = natural.iter().sum();
    if sum <= target {
        let k = if sum > 0.0 { target / sum } else { 0.0 };
        return natural.iter().map(|w| w * k).collect();
    }
    // 压缩：每轮都从**原始自然宽**重新按比例缩放，被压到下限的列退出，预算重分。
    let mut out = natural.to_vec();
    let mut active: Vec<usize> = (0..natural.len()).collect();
    let mut budget = target;
    loop {
        let total: f32 = active.iter().map(|&i| natural[i]).sum();
        if total <= f32::EPSILON {
            break;
        }
        for &i in &active {
            out[i] = natural[i] * budget / total;
        }
        let clamped: Vec<usize> = active.iter().copied().filter(|&i| out[i] < min_w).collect();
        if clamped.is_empty() {
            break;
        }
        for &i in &clamped {
            out[i] = min_w;
        }
        active.retain(|i| !clamped.contains(i));
        budget -= min_w * clamped.len() as f32;
        if budget <= 0.0 {
            // 剩下的列连下限都分不到：全部按下限收尾（总宽超了版心 → 交给横向滚动）。
            for &i in &active {
                out[i] = min_w;
            }
            break;
        }
    }
    out
}

/// 单元格排版：`wrap_w` 为 `f32::INFINITY` 表示不折行（量自然宽时用）。
fn cell_galley(
    ui: &egui::Ui,
    content: Option<&Vec<Inline>>,
    header: bool,
    style: &egui::Style,
    wrap_w: f32,
) -> Arc<egui::Galley> {
    ui.painter().layout_job(inline_job(
        ui,
        content.map(Vec::as_slice).unwrap_or_default(),
        wrap_w,
        style,
        |rt| if header { rt.strong() } else { rt },
    ))
}

/// 底色分带：表头一条实底、数据行隔行浅底（不画横线，靠色带分，读起来更像表）。
fn paint_row_bg(ui: &egui::Ui, rect: egui::Rect, header: bool, row_no: usize) {
    let color = if header {
        ui.visuals().widgets.noninteractive.weak_bg_fill
    } else if row_no.is_multiple_of(2) {
        ui.visuals().faint_bg_color
    } else {
        return;
    };
    ui.painter().rect_filled(rect, 0.0, color);
}

/// 列宽下限：至少放得下一小段文字加两侧留白（空列也点得着，宽表不至于挤成竖条）。
const MIN_COL_W: f32 = 56.0;
/// 单元格留白：文字不贴着隔壁列。
const CELL_PAD_X: f32 = 8.0;
const CELL_PAD_Y: f32 = 3.0;
/// 列表符号前的缩进（有序无序同宽，两种列表看起来才是一套的）。
/// **每深入一层加一个**——固定缩进套在每层上会让嵌套列表看起来是"平的两段"（见 [`list_block`]）。
const LIST_INDENT_W: f32 = 16.0;
/// 列表符号列的**定宽**：符号 / 勾选框 / 编号都占这么宽，正文起点只由它决定。
/// 定宽是"编号右对齐"与"层级步长稳定"两条的前提（见 [`list_block`] 与 [`symbol`]）。
const LIST_SYMBOL_W: f32 = 20.0;
/// 版心宽度上限：正文折行不超过它，超出部分留白、内容水平居中。
///
/// 为什么要有上限：中央面板在宽屏下能到 1200+px，一行塞 50 多个汉字，而中文正文的
/// 舒适区是 30–40 字/行（≈ 640–720px）——超了"扫行时找不到下一行的开头"。
/// 680 ≈ 34 个汉字。这是**审美取值**，要改只改这一个常量。
const MEASURE_W: f32 = 680.0;
/// 块与块之间的间距（标题另算，见 [`gap_before`]）。
const BLOCK_GAP: f32 = 10.0;
/// 标题与它前面那块内容的间距：比块间距大，标题才像"小节标题"而不只是"大一号的行"。
const HEADING_GAP_BEFORE: f32 = 14.0;
/// 标题与它后面内容的间距：小一点，否则标题会与小节内容脱开、像换了个区域。
const HEADING_GAP_AFTER: f32 = 4.0;

fn align_of(t: &Table, i: usize) -> Align {
    t.aligns.get(i).copied().unwrap_or(Align::Left)
}

/// 行内 → 一个可折行的 `LayoutJob`（段落与列表项用这条路径）。
///
/// egui 0.36 的 `RichText` 只能带**一种**样式，不再支持旧版的 `push` 拼多段；
/// 段内混排（`**粗**`、`` `码` ``）必须靠 `append_to` 累加进同一个 job，
/// 否则一行里每个片段各占一个 label，折行位置与基线都会散掉。
///
/// `wrap_w` 是折行宽度：表格单元格给列宽，标签给 `available_width()`。
/// 注意 `ui.label` 会用自己的 `available_width()` 覆盖 job 里的折行宽度
/// （egui `Label::layout_in_ui`），所以标签这条路只能靠"块内宽度本身一致"来保证版心统一。
fn inline_job(
    ui: &egui::Ui,
    v: &[Inline],
    wrap_w: f32,
    style: &egui::Style,
    deco: impl Fn(egui::RichText) -> egui::RichText,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: wrap_w,
            ..Default::default()
        },
        ..Default::default()
    };
    for i in v {
        deco(rt_of(ui, i)).append_to(
            &mut job,
            style,
            egui::FontSelection::Default,
            egui::Align::Center,
        );
    }
    job
}

/// 段落 / 列表项：按当前可用宽度折行（可用宽度由块内布局保证一致，见 [`show`]）。
fn inline_label(ui: &mut egui::Ui, v: &[Inline]) {
    let style = ui.style().clone();
    let w = ui.available_width();
    ui.label(inline_job(ui, v, w, &style, |rt| rt));
}

/// 单个行内元素 → `RichText`；`Link` / `Image` 一律降级成**不可点**的纯文本（见模块注释）。
fn rt_of(ui: &egui::Ui, i: &Inline) -> egui::RichText {
    match i {
        Inline::Text(t) => egui::RichText::new(t.clone()),
        Inline::Code(t) => egui::RichText::new(t.clone()).code(),
        Inline::Strong(c) => joined(c).strong(),
        Inline::Emphasis(c) => joined(c).italics(),
        Inline::Strikethrough(c) => joined(c).strikethrough(),
        // 链接：加下划线让人一眼看出"这里原本是链接"，但**没有点击行为**。
        Inline::Link { text, .. } => joined(text).color(palette::link(ui)).underline(),
        Inline::Image { alt, .. } => egui::RichText::new(format!("\u{1F5BC} {}", alt)).italics(),
    }
}

fn joined(v: &[Inline]) -> egui::RichText {
    egui::RichText::new(inline_text(v))
}

// ===================== 单元测试 =====================

#[cfg(test)]
mod tests {
    use super::*;

    /// 断言"渲染后仍看得到这段文字"：审核工具宁可丑，不可丢内容。
    fn assert_visible(blocks: &[Block], needle: &str) {
        let text = blocks.iter().map(block_text).collect::<Vec<_>>().join("\n");
        assert!(
            text.contains(needle),
            "渲染文本里找不到 {:?}：\n{}",
            needle,
            text
        );
    }

    fn block_text(b: &Block) -> String {
        match b {
            Block::Heading { inlines, .. } | Block::Paragraph(inlines) => inline_text(inlines),
            Block::Code { text, .. } => text.clone(),
            Block::Rule => "---".into(),
            Block::Quote(bs) => bs_text(bs),
            Block::List(l) => l
                .items
                .iter()
                .map(|it| bs_text(&it.blocks))
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Table(t) => {
                let head = t
                    .header
                    .iter()
                    .map(|c| inline_text(c))
                    .collect::<Vec<_>>()
                    .join(" | ");
                let rows = t
                    .rows
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|c| inline_text(c))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{}\n{}", head, rows)
            }
        }
    }

    fn bs_text(blocks: &[Block]) -> String {
        blocks.iter().map(block_text).collect::<Vec<_>>().join("\n")
    }

    /// 解析一段**单块**正文并取出行内序列。
    ///
    /// 解析层换成 `pulldown-cmark` 之后没有"只解析行内"的入口了，而用例要断言的恰恰是
    /// "**公开入口**解析出的结构"，所以统一从 [`parse`] 进——直接调内部辅助函数断言的用例
    /// 会在重构时静默失效（测的是实现细节，不是行为）。
    fn inlines_of(src: &str) -> Vec<Inline> {
        let blocks = parse(src);
        match blocks.first() {
            Some(Block::Paragraph(v)) | Some(Block::Heading { inlines: v, .. }) => v.clone(),
            other => panic!("首块应含行内：{other:?}"),
        }
    }

    /// 离屏跑一帧的上下文，**必须**带内嵌字体。
    ///
    /// `Context::default()` 里没有可用的中文字形，长出来的 galley 尺寸全是 0，
    /// 任何几何断言都会拿到 NaN —— 也就是说没装字体的离屏测试只能验"有没有画东西"，
    /// 验不了"画在哪、画多宽"。窗口启动用的是同一份定义（[`crate::fonts`]）。
    fn ctx_with_fonts() -> eframe::egui::Context {
        let ctx = eframe::egui::Context::default();
        ctx.set_fonts(crate::fonts::definitions());
        ctx
    }

    /// 离屏跑帧的输入：**给一个正常大小的窗口**。
    ///
    /// `RawInput::default()` 的屏幕是两万点宽的"无限大"画布，任何宽度断言都会因为
    /// "怎么都不换行"而失去意义（表宽、折行宽度看起来都对，其实没被考验）。
    /// 800×600 与窗口默认的 1000×680 同量级。
    fn raw() -> eframe::egui::RawInput {
        eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::pos2(0.0, 0.0),
                eframe::egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        }
    }

    #[test]
    fn 任务列表解析出勾选态() {
        let b = parse("- [x] 已完成\n- [ ] 未完成\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表：{:?}", b[0])
        };
        assert_eq!(l.items.len(), 2);
        assert_eq!(l.items[0].checked, Some(true));
        assert_eq!(l.items[1].checked, Some(false));
        assert_visible(&b, "未完成");
    }

    #[test]
    fn 有序列表保留起始编号() {
        let b = parse("3. 三\n4. 四\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表")
        };
        assert!(l.ordered && l.start == 3);
        assert_visible(&b, "四");
    }

    #[test]
    fn 标题层级与行内样式() {
        let b = parse("### 小节 **粗** `码`\n");
        let Block::Heading { level, inlines } = &b[0] else {
            panic!("应为标题")
        };
        assert_eq!(*level, 3);
        assert!(matches!(inlines[1], Inline::Strong(_)));
        assert!(matches!(inlines[3], Inline::Code(_)));
    }

    #[test]
    fn 围栏代码块不解析内部标记() {
        let b = parse("```rust\nlet a = **1**;\n```\n");
        let Block::Code { lang, text } = &b[0] else {
            panic!("应为代码块")
        };
        assert_eq!(lang, "rust");
        assert_eq!(text, "let a = **1**;\n");
    }

    #[test]
    fn 表格解析出对齐() {
        let b = parse("| 项 | 值 |\n| :--- | ---: |\n| a | 1 |\n");
        let Block::Table(t) = &b[0] else {
            panic!("应为表格")
        };
        assert_eq!(t.aligns, vec![Align::Left, Align::Right]);
        assert_eq!(t.rows.len(), 1);
        assert_visible(&b, "a");
    }

    #[test]
    fn 字体缺字形的符号降级为等义字符() {
        // 内嵌字体没有 ✅/❌ 的字形，不降级就显示成 `?`。
        assert_eq!(
            inline_text(&inlines_of("✅ 通过　❌ 未做")),
            "✓ 通过　✗ 未做"
        );
        assert_eq!(inline_text(&inlines_of("⚠️ 注意")), "⚠ 注意");
        assert_eq!(inline_text(&inlines_of("⭐ 重要")), "★ 重要");
        // 没有被降级的字符原样保留。
        assert_eq!(inline_text(&inlines_of("正常 ✓ 字符")), "正常 ✓ 字符");
    }

    #[test]
    fn 表格首尾竖线可省() {
        let b = parse("项 | 值\n--- | ---\na | 1\n");
        let Block::Table(t) = &b[0] else {
            panic!("应为表格：{:?}", b[0])
        };
        assert_eq!(t.rows.len(), 1);
        assert_visible(&b, "a");
    }

    #[test]
    fn 表格单元格内的竖线不切列() {
        // 真实缺陷：清单里"字段用 `file|anchor|replacement` 分隔"这类行，
        // 按 `|` 硬切就变成多出好几列、内容整体错位（列数还会被数据行顶大）。
        //
        // 解析器（GFM）要求表格内的竖线必须转义，**代码段内也不例外**，而清单里大量不转义；
        // 直接喂原文会被切成 3 格并把 `repl` 之后的内容与 `2h` **静默丢掉**（实测）。
        // 是 [`escape_table_code_pipes`] 把它补回来的 —— 这条用例守的就是那一层。
        let b = parse(
            "| 编号 | 子任务 | 预估工时 |\n| --- | --- | --- |\n\
             | T1 | 变异清单文件格式（`file|anchor|replacement|test-filter|理由`） | 2h |\n",
        );
        let Block::Table(t) = &b[0] else {
            panic!("应为表格：{:?}", b[0])
        };
        assert_eq!(t.header.len(), 3);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].len(), 3);
        // `inline_text` 是结构文本化（代码段不加反引号），列内容一格都没丢。
        assert_eq!(inline_text(&t.rows[0][0]), "T1");
        assert_eq!(
            inline_text(&t.rows[0][1]),
            "变异清单文件格式（file|anchor|replacement|test-filter|理由）"
        );
        assert_eq!(inline_text(&t.rows[0][2]), "2h");
    }

    #[test]
    fn 表格里转义的竖线也不切列() {
        // GFM：`\|` 与代码段内的竖线同义，都是"内容里的竖线"。
        let b = parse("| a | b |\n| --- | --- |\n| `a|b` | c\\|d |\n");
        let Block::Table(t) = &b[0] else {
            panic!("应为表格：{:?}", b[0])
        };
        assert_eq!(t.header.len(), 2);
        assert_eq!(t.rows[0].len(), 2);
        assert_eq!(inline_text(&t.rows[0][0]), "a|b");
        assert_eq!(inline_text(&t.rows[0][1]), "c|d");
    }

    #[test]
    fn 未闭合的反引号仍按列分隔() {
        // 不成对的反引号不是代码段（GFM），否则整行会被并成一格、后面几列全丢。
        let b = parse("| a | b | c |\n| --- | --- | --- |\n| b`c | d | e |\n");
        let Block::Table(t) = &b[0] else {
            panic!("应为表格：{:?}", b[0])
        };
        assert_eq!(t.rows[0].len(), 3);
        assert_eq!(inline_text(&t.rows[0][0]), "b`c");
        assert_eq!(inline_text(&t.rows[0][2]), "e");
    }

    #[test]
    fn 已转义的竖线不被重复转义() {
        // 归一化只在**未转义**的裸竖线前加 `\`。这里若也加一次，单元格里会多一个反斜杠
        // （代码段内反斜杠不生效，`a\\|b` 会原样显示出来）。
        let b = parse("| a | b |\n| --- | --- |\n| `x\\|y` | z |\n");
        let Block::Table(t) = &b[0] else {
            panic!("应为表格：{:?}", b[0])
        };
        assert_eq!(t.rows[0].len(), 2);
        assert_eq!(inline_text(&t.rows[0][0]), "x|y");
    }

    #[test]
    fn 围栏代码块与普通段落里的竖线一字不改() {
        // 归一化**只动表格行**：代码段内反斜杠不生效，给普通段落的 `` `a|b` `` 补转义
        // 等于把对的写成错的（会显示成 `a\|b`）。
        let b = parse("段落里有 `a|b` 和 | 竖线\n\n```\n| a | `x|y` |\n```\n");
        assert_eq!(
            inline_text(&inlines_of("段落里有 `a|b` 和 | 竖线")),
            "段落里有 a|b 和 | 竖线"
        );
        let code: Vec<&str> = b
            .iter()
            .filter_map(|x| match x {
                Block::Code { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(code, vec!["| a | `x|y` |\n"]);
    }

    #[test]
    fn 列宽按自然宽比例摊到版心宽度() {
        // 比版心窄 → 放大到刚好填满（表与正文同宽、右边缘对齐）。
        assert_eq!(distribute(&[100.0, 200.0], 600.0, 56.0), vec![200.0, 400.0]);
        // 比版心宽 → 压缩，短列钉在下限保内容，剩下的宽度长列自己承担（长列去折行）。
        let w = distribute(&[1000.0, 100.0], 200.0, 56.0);
        assert_eq!(w, vec![144.0, 56.0], "{w:?}");
        // 等比压缩时不超版心、也不低于下限。
        let w = distribute(&[400.0, 400.0], 600.0, 56.0);
        assert!((w.iter().sum::<f32>() - 600.0).abs() < 0.5, "{w:?}");
        assert!(w.iter().all(|x| *x >= 56.0), "{w:?}");
        // 连下限都放不下 → 每列都是下限（表比面板还宽，交给横向滚动）。
        assert_eq!(
            distribute(&[300.0, 300.0, 300.0], 100.0, 56.0),
            vec![56.0, 56.0, 56.0]
        );
        assert!(distribute(&[], 100.0, 56.0).is_empty());
    }

    #[test]
    fn 版心宽度有上限且正文居中() {
        // 800 宽的屏：中央面板可用宽（约 784）远大于版心 680，
        // 于是正文必须被限到 680 并水平居中——早先正文直接吃满可用宽，
        // 一行能塞 50 多个汉字，"扫行时找不到下一行的开头"。
        let ctx = ctx_with_fonts();
        let blocks = parse(&format!("{}。\n", "这".repeat(200)));
        let measure = std::cell::Cell::new(0.0f32);
        let avail = std::cell::Cell::new(egui::Rect::NOTHING);
        let mut out = ctx.run_ui(raw(), |ui| {
            measure.set(ui.available_width().min(MEASURE_W));
            avail.set(ui.available_rect_before_wrap());
            show(ui, &blocks);
        });
        out.textures_delta.clear();
        let para = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("这") => Some(t.clone()),
                _ => None,
            })
            .expect("应画出段落文字");
        let wrap = para.galley.job.wrap.max_width;
        assert_eq!(measure.get(), MEASURE_W, "800 宽屏下版心应取上限");
        assert!(
            (wrap - measure.get()).abs() < 1.0,
            "折行宽度 {wrap} 应等于版心 {}",
            measure.get()
        );
        // 居中量的是**版心这一块**的左右留白，不是墨迹：
        // 段落实际排出来的每行宽度取决于字形（实测最宽的一行 676 而版心是 680），
        // 拿墨迹中线断言"左右留白相等"会永远差那几像素。版心左边缘 = 段落排版原点。
        let pad_left = para.pos.x - avail.get().min.x;
        let pad_right = avail.get().max.x - (para.pos.x + measure.get());
        assert!(
            (pad_left - pad_right).abs() < 1.0,
            "版心左右留白应相等：左 {pad_left} vs 右 {pad_right}（版心宽 {}）",
            measure.get()
        );
    }

    #[test]
    fn 版心在窄面板下退化为可用宽() {
        // 面板比版心还窄时不能硬撑 680（会溢出），`min` 必须生效。
        let ctx = ctx_with_fonts();
        let blocks = parse(&format!("{}。\n", "短".repeat(4)));
        let mut raw = raw();
        raw.screen_rect = Some(eframe::egui::Rect::from_min_size(
            eframe::egui::pos2(0.0, 0.0),
            eframe::egui::vec2(400.0, 600.0),
        ));
        let measure = std::cell::Cell::new(0.0f32);
        let mut out = ctx.run_ui(raw, |ui| {
            measure.set(ui.available_width());
            show(ui, &blocks);
        });
        out.textures_delta.clear();
        let wrap = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("短") => {
                    Some(t.galley.job.wrap.max_width)
                }
                _ => None,
            })
            .expect("应画出段落文字");
        assert!(measure.get() < MEASURE_W, "400 宽屏的可用宽应小于版心");
        assert!(
            (wrap - measure.get()).abs() < 1.0,
            "折行宽度 {wrap} 应等于可用宽 {}",
            measure.get()
        );
    }

    #[test]
    fn 表格宽度等于版心而不是等于内容() {
        // 两列短表曾经只有百来 px 宽（列宽上限 300px + 整表按内容收缩），
        // 与同页段落不齐；表头底色的宽度就是整表宽度，拿它跟版心比最直接。
        // 基准是 `min(可用宽, MEASURE_W)`：版心加了上限之后，"版心"不再是可用宽本身。
        let ctx = ctx_with_fonts();
        let blocks = parse("一段正文。\n\n| 项 | 值 |\n| --- | --- |\n| a | 1 |\n");
        let measure = std::cell::Cell::new(0.0f32);
        let mut out = ctx.run_ui(raw(), |ui| {
            measure.set(ui.available_width().min(MEASURE_W));
            show(ui, &blocks);
        });
        out.textures_delta.clear();
        let table_w = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Rect(r) => Some(r.rect.width()),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        assert!(
            (table_w - measure.get()).abs() < 1.0,
            "表宽 {table_w} 应等于版心宽 {}（表与正文同宽）",
            measure.get()
        );
        // 段落与表格是**同一个**版心：正文的折行宽度也就是版心宽度。
        let para_wrap = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("一段正文") => {
                    Some(t.galley.job.wrap.max_width)
                }
                _ => None,
            })
            .expect("应画出段落文字");
        assert!(
            (para_wrap - measure.get()).abs() < 1.0,
            "段落折行宽度 {para_wrap} 应等于版心宽 {}",
            measure.get()
        );
        // 单元格文字不得越出版心右边缘（列宽分配错了就会越界）。
        let (left, right) = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some(t.visual_bounding_rect()),
                _ => None,
            })
            .fold((f32::MAX, 0.0f32), |(lo, hi), r| {
                (lo.min(r.min.x), hi.max(r.max.x))
            });
        assert!(
            right <= left + measure.get() + 1.0,
            "文字右边缘 {right} 不应越过版心右边缘 {}",
            left + measure.get()
        );
    }

    #[test]
    fn 列表缩进按层级递增() {
        // 早先每一层都只加同一个常量缩进，内外层的正文起点只差 16px，
        // 两层挤在一起看不出层级。守住"每深入一层 +LIST_INDENT_W"这条。
        // 比的是两项**正文**的起点：符号相同（都是 `•`），差值就等于缩进差。
        let ctx = ctx_with_fonts();
        let blocks = parse("- 外\n  - 内\n");
        let mut out = ctx.run_ui(raw(), |ui| show(ui, &blocks));
        out.textures_delta.clear();
        let x_of = |name: &str| {
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    // 取**排版原点**而不是视觉矩形：后者含字形边距，
                    // 不同字的边距不同（"外"与"内"就差 1px），会污染缩进断言。
                    egui::epaint::Shape::Text(t) if t.galley.text().trim() == name => Some(t.pos.x),
                    _ => None,
                })
                .fold(None::<f32>, |acc: Option<f32>, x| {
                    Some(acc.map_or(x, |a| a.min(x)))
                })
                .unwrap_or_else(|| panic!("没画出 {name}"))
        };
        let delta = x_of("内") - x_of("外");
        assert!(
            (delta - LIST_INDENT_W).abs() < 0.5,
            "内外层正文起点应差 {LIST_INDENT_W}，实得 {delta}"
        );
    }

    #[test]
    fn 有序编号按最大位数右对齐() {
        // `9.` 与 `10.` 宽度差一个字符：不右对齐的话，同一列表里跨过 9 的那一项
        // 整列文字会横向跳一下。
        let ctx = ctx_with_fonts();
        let blocks = parse("9. 九\n10. 十\n");
        let mut out = ctx.run_ui(raw(), |ui| show(ui, &blocks));
        out.textures_delta.clear();
        let right_of = |num: &str| {
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    // 排版右边缘（绘制原点 + 排版矩形右端），不含字形边距。
                    egui::epaint::Shape::Text(t) if t.galley.text().trim() == num => {
                        Some(t.pos.x + t.galley.rect.max.x)
                    }
                    _ => None,
                })
                .fold(None::<f32>, |acc: Option<f32>, x| {
                    Some(acc.map_or(x, |a| a.min(x)))
                })
                .unwrap_or_else(|| panic!("没画出编号 {num}"))
        };
        assert!(
            (right_of("9.") - right_of("10.")).abs() < 1.0,
            "编号右边缘应一致：{} vs {}",
            right_of("9."),
            right_of("10.")
        );
    }

    #[test]
    fn 列表一项占一行且折行宽度一致() {
        // 守住的不变量（不是复现某个旧缺陷）：两项必须在两行上，且折行宽度完全一致。
        // 若列表改回 `horizontal_wrapped`，两项一旦排到同一行，第二项的折行宽度就是
        // "版心 − 上一项宽度"，这条断言会先炸。
        let ctx = ctx_with_fonts();
        let blocks = parse("- 甲\n- 乙\n");
        let mut out = ctx.run_ui(raw(), |ui| show(ui, &blocks));
        out.textures_delta.clear();
        let rows: Vec<(String, f32, f32)> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    t.visual_bounding_rect().min.y,
                    t.galley.job.wrap.max_width,
                )),
                _ => None,
            })
            .collect();
        let item = |name: &str| {
            rows.iter()
                .find(|(t, _, _)| t.trim() == name)
                .unwrap_or_else(|| panic!("没画出行内文本 {name}：{rows:?}"))
        };
        let (y_a, wrap_a) = {
            let (_, y, w) = item("甲");
            (y, w)
        };
        let (y_b, wrap_b) = {
            let (_, y, w) = item("乙");
            (y, w)
        };
        assert!(
            (y_a - y_b).abs() >= 8.0,
            "两项应在两行上（y {y_a} vs {y_b}）：{rows:?}"
        );
        assert!(
            (wrap_a - wrap_b).abs() < 0.5,
            "两项的折行宽度应一致：{wrap_a} vs {wrap_b}"
        );
    }

    #[test]
    fn 门禁标记注释整块跳过() {
        let b = parse("## 1. 需求分解\n\n<!-- GATE:STEP name=decomposition -->\n\n- [ ] 背景\n");
        assert!(!bs_text(&b).contains("GATE"), "GATE 标记行不该出现在正文里");
        assert!(matches!(b[0], Block::Heading { .. }));
        assert!(matches!(b[1], Block::List(_)));
    }

    #[test]
    fn 链接与图片只降级为文本不丢内容() {
        let v = inlines_of("见 [文档](https://a.tld/x) 与 ![图](a.png)");
        let Inline::Link { dest, .. } = &v[1] else {
            panic!("应为链接：{:?}", v[1])
        };
        assert_eq!(dest, "https://a.tld/x");
        assert!(v.iter().any(|i| matches!(i, Inline::Image { .. })));
        let t = inline_text(&v);
        assert!(t.contains("文档") && t.contains("图"));
    }

    #[test]
    fn 未闭合语法按原文保留() {
        let v = inlines_of("未闭合 **粗 与 `码");
        assert_eq!(inline_text(&v), "未闭合 **粗 与 `码");
    }

    #[test]
    fn 标识符里的下划线不被当成斜体() {
        assert_eq!(
            inline_text(&inlines_of("see req_guard_hook.sh")),
            "see req_guard_hook.sh"
        );
    }

    #[test]
    fn 引用块内可嵌套列表() {
        let b = parse("> 提示：\n> - 一\n> - 二\n");
        let Block::Quote(inner) = &b[0] else {
            panic!("应为引用")
        };
        assert!(inner.iter().any(|x| matches!(x, Block::List(_))));
        assert_visible(&b, "二");
    }

    #[test]
    fn 嵌套列表并入父项() {
        let b = parse("- 外\n  - 内\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表")
        };
        assert_eq!(l.items.len(), 1);
        assert!(l.items[0]
            .blocks
            .iter()
            .any(|x| matches!(x, Block::List(_))));
        assert_visible(&b, "内");
    }

    #[test]
    fn 顶格续行并入所属列表项() {
        let b = parse("- 第一项\n  续行说明\n- 第二项\n");
        let Block::List(l) = &b[0] else {
            panic!("应为列表")
        };
        assert_eq!(l.items.len(), 2);
        assert_visible(&b, "续行说明");
    }

    #[test]
    fn 缓存按段与原文取值_改文即换内容() {
        let mut c = Cache::default();
        assert!(matches!(c.get(0, "## a")[0], Block::Heading { .. }));
        assert!(matches!(c.get(0, "- [ ] 一")[0], Block::List(_)));
        // 同键反复取：内容必须一致（缓存命中与重新解析等价，用户看不出差别）。
        let again = c.get(0, "- [ ] 一").to_vec();
        assert_eq!(&again, c.get(0, "- [ ] 一"));
        // 正文改了 → 立刻换成新内容（否则审核人看的是上一版清单）。
        assert!(matches!(&c.get(0, "- [ ] 二")[0], Block::List(l)
            if l.items[0].blocks[0] == Block::Paragraph(inlines_of("二"))));
        // 段下标变了 → 取的是新一段的内容（不是上一段的缓存）。
        assert!(matches!(c.get(1, "## b")[0], Block::Heading { .. }));
        c.clear();
        assert!(matches!(c.get(1, "## b")[0], Block::Heading { .. }));
    }

    #[test]
    fn 标题与注释的块序稳定() {
        // HTML 注释在事件流里是 `HtmlBlock` + `Html`，整块丢弃后
        // "标题后面紧跟列表"这个次序必须不变（清单的段结构就靠它）。
        let b = parse("## 1. 需求分解\n\n<!-- GATE:STEP name=decomposition -->\n\n- [ ] 背景\n");
        assert!(matches!(b[0], Block::Heading { level: 2, .. }));
        assert!(matches!(b[1], Block::List(_)));
        assert_eq!(b.len(), 2, "不该多出空块：{b:?}");
    }

    #[test]
    fn 链接地址与图片地址都不进正文() {
        // 安全纪律的可观测面：渲染出来的文字里**不许**出现真实地址，
        // 否则等于把 `https://evil.tld` 摆在审核人眼前等他照抄。
        let ctx = ctx_with_fonts();
        let blocks = parse("见 [文档](https://a.tld/x) 与 ![图](a.png)\n");
        let mut out = ctx.run_ui(raw(), |ui| show(ui, &blocks));
        out.textures_delta.clear();
        let drawn: String = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(drawn.contains("文档") && drawn.contains("图"), "{drawn:?}");
        assert!(!drawn.contains("a.tld"), "链接地址不应被渲染：{drawn:?}");
        assert!(!drawn.contains("a.png"), "图片地址不应被渲染：{drawn:?}");
    }

    #[test]
    fn 在滚动区里不遮挡后续控件_且内容高度可滚动() {
        // 复现**真实容器**（`app.rs` 的结构）：纵向 ScrollArea → CollapsingHeader 正文 → show()。
        //
        // 曾经的故障：`show()` 用 `new_child` 把版心子 ui 直接摆到父 ui 的可用区里，
        // **父游标不推进**（`new_child` 不像 `scope_builder` 那样 advance_cursor），
        // 于是正文压在了下一段标题/按钮上，且 ScrollArea 量到的内容高度只有视口高 → 滚不动。
        // 裸 ui 里的离屏测试（版心宽度那几条）**测不出这个故障**：它们不经过滚动区。
        let ctx = ctx_with_fonts();
        let body = (0..40)
            .map(|i| format!("第 {i} 段：{}", "内容".repeat(30)))
            .collect::<Vec<_>>()
            .join("\n\n");
        let blocks = parse(&body);
        let end_y = std::cell::Cell::new(0.0f32);
        let content_h = std::cell::Cell::new(0.0f32);
        let mut out = ctx.run_ui(raw(), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("md_scroll")
                .show(ui, |ui| {
                    egui::CollapsingHeader::new("1. 需求分解")
                        .open(Some(true))
                        .show(ui, |ui| show(ui, &blocks));
                    end_y.set(ui.label("END-OF-BODY").rect.max.y);
                    content_h.set(ui.min_rect().height());
                });
        });
        out.textures_delta.clear();
        let last_text_y = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if !t.galley.text().contains("END-OF-BODY") => {
                    Some(t.visual_bounding_rect().max.y)
                }
                _ => None,
            })
            .fold(0.0f32, f32::max);
        // 正文必须排在**它自己的段标题下方**（截图里的故障形态：正文压住段标题）。
        let header_y = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("需求分解") => {
                    Some(t.visual_bounding_rect().max.y)
                }
                _ => None,
            })
            .fold(0.0f32, f32::max);
        let first_text_y = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if t.galley.text().contains("第 0 段") => {
                    Some(t.visual_bounding_rect().min.y)
                }
                _ => None,
            })
            .fold(f32::MAX, f32::min);
        assert!(
            first_text_y > header_y,
            "正文首行 y={first_text_y} 应在段标题 y={header_y} 之下：正文压住了段标题"
        );
        assert!(
            end_y.get() > last_text_y,
            "后续控件应排在正文下方（END y={} <= 正文末行 y={last_text_y}）：正文压住了后续控件",
            end_y.get()
        );
        assert!(
            content_h.get() > 600.0,
            "ScrollArea 量到的内容高度 {} 应超过视口 600，否则滚不动",
            content_h.get()
        );
    }

    #[test]
    fn 短表格不留大片空白() {
        // 复现：短表格后面跟一段文字时，中间出现一整屏空白。
        // 成因：`table()` 里的横向 ScrollArea 用了 `auto_shrink([true, false])` ——
        // 纵向 `auto_shrink = false` 且该方向没开滚动，egui 会按"填满可用高度"排版，
        // 于是表格占掉整屏高度，其后的段落被顶到屏幕外（滚下去才看见）。
        let ctx = ctx_with_fonts();
        let blocks = parse("| 项 | 值 |\n| --- | --- |\n| 工时 | 2d |\n\n紧跟表格的一段文字。\n");
        let mut out = ctx.run_ui(raw(), |ui| show(ui, &blocks));
        out.textures_delta.clear();
        let y_of = |needle: &str| {
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text().contains(needle) => {
                        Some(t.visual_bounding_rect())
                    }
                    _ => None,
                })
                .fold(None::<egui::Rect>, |acc: Option<egui::Rect>, r| {
                    Some(acc.map_or(r, |a| a.union(r)))
                })
                .unwrap_or_else(|| {
                    panic!("没画出 {needle}：它被顶到视口外了（表格下方留了一整屏空白）")
                })
        };
        let table = y_of("工时");
        let para = y_of("紧跟表格");
        let gap = para.min.y - table.max.y;
        assert!(
            gap < 30.0,
            "短表格与下一段之间不应有大片空白，实得 {gap}px（表格底部 {} → 段落顶部 {}）",
            table.max.y,
            para.min.y
        );
    }

    #[test]
    fn 渲染路径不panic且真的画出了东西() {
        // egui 可以脱离窗口跑一帧：用来守住"渲染代码在真实 egui 调用下不炸"这条底线。
        let src = "## 1. 需求分解\n\n> 提示 **加粗** 与 `代码`\n\n\
                   - [x] 已完成\n- [ ] 未完成\n  - 嵌套项\n\n1. 第一\n2. 第二\n\n\
                   | 项 | 值 |\n| :--- | ---: |\n| 工时 | 2d |\n\
                   | 超长单元格 | 这一格很长很长很长很长很长很长很长很长很长很长很长很长，用来逼出折行路径 |\n\n\
                   ```bash\nreq-guard approve REQ-001\n```\n\n---\n\n\
                   见 [文档](https://a.tld) 与 ~~废弃~~ 条目\n";
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui, &parse(src)));
        // 离屏跑帧没有渲染器去消费纹理增量，必须显式 clear，否则 epaint 在 Drop 时 panic。
        out.textures_delta.clear();
        // 有形状输出 = 确实排出了可见内容（一条 shape 都没画说明根本没跑起来）。
        assert!(
            !out.shapes.is_empty(),
            "渲染后应有绘制输出，说明正文真的画出来了"
        );
    }

    #[test]
    fn 模板首段能解析出全部要素() {
        let src = "## 1. 需求分解\n\n> **AI 需求门禁清单**：三段全通过才允许编码。\n\n\
                   - [ ] 背景与问题：为什么要做\n- [x] 已确认范围\n\n\
                   | 项 | 值 |\n| --- | --- |\n| 工时 | 2d |\n\n\
                   ```bash\nreq-guard approve REQ-001\n```\n\n---\n";
        let blocks = parse(src);
        assert!(matches!(blocks[0], Block::Heading { level: 2, .. }));
        assert!(matches!(blocks[1], Block::Quote(_)));
        assert!(matches!(blocks[2], Block::List(_)));
        assert!(matches!(blocks[3], Block::Table(_)));
        assert!(matches!(blocks[4], Block::Code { .. }));
        assert!(matches!(blocks[5], Block::Rule));
        assert_visible(&blocks, "req-guard approve REQ-001");
    }
}
