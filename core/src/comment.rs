//! 审核评论机制。
//!
//! ## 原则
//! **审核人不修改清单正文，只添加评论；AI 读取评论后自行修改正文。**
//! **AI 不能 resolve（关闭）评论**，只能 reply——否则 AI 可"自证已改"，审核形同虚设。
//!
//! ## 存储
//! 评论存放于**独立文件** `<需求文件 stem>.comments.md`（与清单正文分离）。
//! 原因：清单正文对 AI 可写，若评论内嵌，AI 改正文时可顺手删改审核意见——
//! 等于"嫌疑人修改证据"。独立后配合 L1 hook 的路径拦截，实现证据不可篡改。
//!
//! ## 判定
//! 仅 `state=open && blocking=true` 的评论影响门禁（拦截）；非阻塞评论只提示。

use crate::error::{GateError, Result};
use crate::requirement::{self, token, Requirement};
use std::fs;
use std::path::{Path, PathBuf};

/// 评论文件后缀（定义在 [`crate::requirement`]，那里是"文件命名约定"的唯一出处）。
pub use crate::requirement::COMMENTS_SUFFIX;

const BEGIN: &str = "<!-- GATE:COMMENTS";
const END: &str = "<!-- /GATE:COMMENTS -->";
const C_BEGIN: &str = "<!-- GATE:COMMENT";
const C_END: &str = "<!-- /GATE:COMMENT -->";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentState {
    Open,
    Resolved,
}

impl CommentState {
    pub fn as_str(self) -> &'static str {
        match self {
            CommentState::Open => "open",
            CommentState::Resolved => "resolved",
        }
    }
    pub fn parse(s: &str) -> CommentState {
        if s == "resolved" {
            CommentState::Resolved
        } else {
            CommentState::Open
        }
    }
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub id: String,
    pub step: Option<String>,
    pub author: String,
    pub ts: String,
    pub state: CommentState,
    pub blocking: bool,
    pub line: Option<usize>,
    pub quote: Option<String>,
    pub ctx_before: Option<String>,
    pub ctx_after: Option<String>,
    pub stale: bool,
    /// 被回复的父评论 ID（AI 只能以回复形式参与，见 [`add`]）。
    pub reply: Option<String>,
    pub body: String,
}

impl Comment {
    /// 是否属于"未解决的阻塞性评论"（会拦截编码）。
    pub fn is_blocking_open(&self) -> bool {
        self.blocking && self.state == CommentState::Open
    }
}

/// 由需求 ID 推导评论文件路径：`REQ-001-foo.md` → `REQ-001-foo.comments.md`。
pub fn comments_path(root: &Path, req_id: &str) -> Result<PathBuf> {
    let r = requirement::find(root, req_id)?;
    let stem = r
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| req_id.to_string());
    Ok(r.path
        .with_file_name(format!("{}{}", stem, COMMENTS_SUFFIX)))
}

/// 新增评论的入参。
///
/// 参数较多，用具名结构体承载：既避免一长串位置参数写错顺序，
/// 也让后续 UI（TUI/GUI）可以直接构造复用。
pub struct NewComment<'a> {
    /// 关联的审核步骤（`decomposition`/`solution`/`testplan`）；`None` 为不锚定步骤的总评。
    pub step: Option<&'a str>,
    /// 作者。`ai` 视为 AI 身份（不得 resolve，只能 reply）。
    pub author: &'a str,
    /// 评论正文。
    pub text: &'a str,
    /// 行号锚定用的原文片段。
    pub quote: Option<&'a str>,
    /// 是否阻塞（未 resolve 即拦截编码）。
    pub blocking: bool,
    /// 被回复的父评论 ID。
    pub reply: Option<&'a str>,
}

/// 添加评论（不改任何步骤状态）。`quote` 用于行号锚定，`reply` 指向被回复的父评论。
///
/// **职责分离**：`author=ai` 时必须带 `reply`——AI 只能回复审核人的意见，
/// 不能自己新开一条评论（否则 AI 可自问自答，伪造"意见已处理"的假象）。
pub fn add(root: &Path, req_id: &str, d: NewComment<'_>) -> Result<Comment> {
    let (step, author, text, quote, blocking, reply) =
        (d.step, d.author, d.text, d.quote, d.blocking, d.reply);
    if text.trim().is_empty() {
        return Err(GateError::Validation("评论内容不能为空（--text）".into()));
    }
    if is_ai(author) && reply.is_none() {
        return Err(GateError::Validation(
            "AI 只能回复评论（--reply <评论ID> --author ai --text \"...\"），不能新开评论；\
             新开评论权归审核人"
                .into(),
        ));
    }
    if let Some(s) = step {
        crate::requirement::validate_step(s)?;
    }
    let path = comments_path(root, req_id)?;
    let content = if path.exists() {
        fs::read_to_string(&path).map_err(|e| GateError::Io {
            path: Some(path.clone()),
            source: e,
        })?
    } else {
        String::new()
    };

    if let Some(rid) = reply {
        if !parse(&content).iter().any(|c| c.id == rid) {
            return Err(GateError::Validation(format!(
                "未找到被回复的评论 {}（需求 {} 的评论文件中不存在该 ID）",
                rid, req_id
            )));
        }
    }

    let id = next_id(&content);
    let ts = safe_field(&crate::gate::now_str());

    // 行号锚定：按 quote 在正文中定位，并抓取前后各一行上下文（用于后续消歧）。
    let req = requirement::find(root, req_id)?;
    let body = fs::read_to_string(&req.path).map_err(|e| GateError::Io {
        path: Some(req.path.clone()),
        source: e,
    })?;
    let line = quote.and_then(|q| locate_line(&body, q));
    let (cb, ca) = match line {
        Some(n) => context(&body, n),
        None => (None, None),
    };

    let mut block = String::new();
    block.push_str(&format!(
        "{} id={} step={} author={} ts={} state={} blocking={} line={} reply={} stale=false -->\n",
        C_BEGIN,
        id,
        step.unwrap_or("-"),
        safe_field(author),
        ts,
        CommentState::Open.as_str(),
        blocking,
        line.map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string()),
        reply.unwrap_or("-")
    ));
    if let Some(q) = quote {
        block.push_str(&format!("> quote: {}\n", one_line(q)));
    }
    if let Some(c) = &cb {
        block.push_str(&format!("> ctx_before: {}\n", one_line(c)));
    }
    if let Some(c) = &ca {
        block.push_str(&format!("> ctx_after: {}\n", one_line(c)));
    }
    block.push('\n');
    block.push_str(text.trim_end());
    block.push('\n');
    block.push_str(C_END);
    block.push('\n');

    let mut out = if content.is_empty() {
        format!(
            "# {} 审核评论（AI 禁止直接写入，请用 req-guard comment）\n\n{} req={} -->\n",
            req_id, BEGIN, req_id
        )
    } else {
        content
    };
    if let Some(pos) = out.find(END) {
        let mut s = out[..pos].to_string();
        s.push_str(&block);
        s.push_str(&out[pos..]);
        out = s;
    } else {
        out.push_str(&block);
        out.push_str(END);
        out.push('\n');
    }
    fs::write(&path, out).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;

    let reply_id = reply.map(|r| r.to_string());
    crate::gate::audit(
        root,
        &format!(
            "COMMENT-ADD {} id={} author={} blocking={} reply={}",
            req_id,
            id,
            safe_field(author),
            blocking,
            reply_id.as_deref().unwrap_or("-")
        ),
    );

    Ok(Comment {
        id,
        step: step.map(|s| s.to_string()),
        author: author.to_string(),
        ts,
        state: CommentState::Open,
        blocking,
        line,
        quote: quote.map(one_line),
        ctx_before: cb,
        ctx_after: ca,
        stale: false,
        reply: reply_id,
        body: text.trim_end().to_string(),
    })
}

/// 关闭（resolve）评论。**AI 被禁止调用**——只能 reply。
pub fn resolve(root: &Path, req_id: &str, cid: &str, author: &str) -> Result<()> {
    if is_ai(author) {
        return Err(GateError::Validation(
            "AI 不能关闭（resolve）评论，只能 reply；关闭权归审核人".into(),
        ));
    }
    let path = comments_path(root, req_id)?;
    if !path.exists() {
        return Err(GateError::Validation(format!(
            "需求 {} 尚无评论文件",
            req_id
        )));
    }
    let content = fs::read_to_string(&path).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    let mut out = String::new();
    let mut found = false;
    for line in content.lines() {
        if line.trim_start().starts_with(C_BEGIN) && token(line, "id") == cid {
            found = true;
            out.push_str(&set_field(line, "state", "resolved"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        return Err(GateError::Validation(format!("未找到评论 {}", cid)));
    }
    fs::write(&path, out).map_err(|e| GateError::Io {
        path: Some(path),
        source: e,
    })?;
    crate::gate::audit(
        root,
        &format!(
            "COMMENT-RESOLVE {} id={} reviewer={}",
            req_id,
            cid,
            safe_field(author)
        ),
    );
    Ok(())
}

/// 身份是否为 AI（大小写不敏感）。用于"AI 不得 resolve、只能 reply"的职责分离判定。
pub fn is_ai(author: &str) -> bool {
    author.trim().eq_ignore_ascii_case("ai")
}

/// 列出某需求的全部评论。
pub fn list(root: &Path, req_id: &str) -> Result<Vec<Comment>> {
    let path = comments_path(root, req_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path).map_err(|e| GateError::Io {
        path: Some(path),
        source: e,
    })?;
    Ok(parse(&content))
}

/// 统计未解决评论与其中阻塞性数量。
pub fn summary(root: &Path, req_id: &str) -> Result<(usize, usize)> {
    let cs = list(root, req_id)?;
    let open = cs.iter().filter(|c| c.state == CommentState::Open).count();
    let blocking = cs.iter().filter(|c| c.is_blocking_open()).count();
    Ok((open, blocking))
}

/// 行号漂移重算：按 `quote` 在正文中重新定位，命中则回写 `line`；未命中标记 `stale=true`。
/// 返回变为 stale 的条数。
pub fn refresh_anchors(root: &Path, req_id: &str) -> Result<usize> {
    let path = comments_path(root, req_id)?;
    if !path.exists() {
        return Ok(0);
    }
    let content = fs::read_to_string(&path).map_err(|e| GateError::Io {
        path: Some(path.clone()),
        source: e,
    })?;
    let req: Requirement = requirement::find(root, req_id)?;
    let body = fs::read_to_string(&req.path).map_err(|e| GateError::Io {
        path: Some(req.path.clone()),
        source: e,
    })?;

    let mut stale_count = 0usize;
    let mut out = String::new();
    for line in content.lines() {
        if line.trim_start().starts_with(C_BEGIN) {
            let cid = token(line, "id");
            if let Some(q) = quote_of(&content, &cid) {
                match locate_line(&body, &q) {
                    Some(n) => out.push_str(&set_field(line, "line", &n.to_string())),
                    None => {
                        stale_count += 1;
                        let l = set_field(line, "line", "-");
                        out.push_str(&set_field(&l, "stale", "true"));
                    }
                }
            } else {
                out.push_str(line);
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    fs::write(&path, out).map_err(|e| GateError::Io {
        path: Some(path),
        source: e,
    })?;
    Ok(stale_count)
}

// ===================== 解析辅助 =====================

fn parse(content: &str) -> Vec<Comment> {
    let mut out = Vec::new();
    let mut cur: Option<Comment> = None;
    let mut body: Vec<String> = Vec::new();
    let mut quote: Option<String> = None;
    let mut cb: Option<String> = None;
    let mut ca: Option<String> = None;

    for line in content.lines() {
        if line.trim_start().starts_with(C_BEGIN) {
            let step = token(line, "step");
            let line_no = token(line, "line").parse::<usize>().ok();
            cur = Some(Comment {
                id: token(line, "id"),
                step: if step.is_empty() || step == "-" {
                    None
                } else {
                    Some(step)
                },
                author: token(line, "author"),
                ts: token(line, "ts"),
                state: CommentState::parse(&token(line, "state")),
                blocking: token(line, "blocking") == "true",
                line: line_no,
                quote: None,
                ctx_before: None,
                ctx_after: None,
                stale: token(line, "stale") == "true",
                reply: none_if_dash(token(line, "reply")),
                body: String::new(),
            });
            body.clear();
            quote = None;
            cb = None;
            ca = None;
            continue;
        }
        if line.contains(C_END) {
            if let Some(mut c) = cur.take() {
                c.quote = quote.take();
                c.ctx_before = cb.take();
                c.ctx_after = ca.take();
                c.body = body.join("\n").trim().to_string();
                out.push(c);
            }
            continue;
        }
        if cur.is_some() {
            if let Some(v) = line.trim_start().strip_prefix("> quote:") {
                quote = Some(v.trim().to_string());
            } else if let Some(v) = line.trim_start().strip_prefix("> ctx_before:") {
                cb = Some(v.trim().to_string());
            } else if let Some(v) = line.trim_start().strip_prefix("> ctx_after:") {
                ca = Some(v.trim().to_string());
            } else {
                body.push(line.to_string());
            }
        }
    }
    out
}

/// `-` 或空串视为"无值"（GATE 行的空值约定）。
fn none_if_dash(v: String) -> Option<String> {
    if v.is_empty() || v == "-" {
        None
    } else {
        Some(v)
    }
}

fn next_id(content: &str) -> String {
    let mut max = 0u32;
    for line in content.lines() {
        if line.trim_start().starts_with(C_BEGIN) {
            let id = token(line, "id");
            if let Some(rest) = id.strip_prefix('C') {
                if let Ok(n) = rest.parse::<u32>() {
                    if n > max {
                        max = n;
                    }
                }
            }
        }
    }
    format!("C{:03}", max + 1)
}

fn quote_of(content: &str, cid: &str) -> Option<String> {
    let mut in_c = false;
    for line in content.lines() {
        if line.trim_start().starts_with(C_BEGIN) {
            in_c = token(line, "id") == cid;
            continue;
        }
        if line.contains(C_END) {
            in_c = false;
            continue;
        }
        if in_c {
            if let Some(v) = line.trim_start().strip_prefix("> quote:") {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// 在正文中按 `quote` 定位行号（1-based，首次命中）。
fn locate_line(body: &str, quote: &str) -> Option<usize> {
    let q = quote.trim();
    if q.is_empty() {
        return None;
    }
    for (i, l) in body.lines().enumerate() {
        if l.contains(q) {
            return Some(i + 1);
        }
    }
    None
}

/// 取指定行（1-based）的前后各一行作为上下文。
fn context(body: &str, line: usize) -> (Option<String>, Option<String>) {
    let lines: Vec<&str> = body.lines().collect();
    let before = if line >= 2 {
        lines.get(line - 2).map(|s| s.to_string())
    } else {
        None
    };
    let after = lines.get(line).map(|s| s.to_string());
    (before, after)
}

/// 重写标记行中某个 key 的值；key 不存在时插入到 `-->` 之前。
fn set_field(line: &str, key: &str, val: &str) -> String {
    let prefix = format!("{}=", key);
    let mut toks: Vec<String> = Vec::new();
    let mut hit = false;
    for t in line.split_whitespace() {
        if t.starts_with(&prefix) {
            toks.push(format!("{}={}", key, val));
            hit = true;
        } else {
            toks.push(t.to_string());
        }
    }
    if !hit && toks.last().map(|s| s.as_str()) == Some("-->") {
        toks.insert(toks.len() - 1, format!("{}={}", key, val));
    }
    toks.join(" ")
}

/// GATE 行禁止空白，统一替换为 `_`。
fn safe_field(s: &str) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join("_");
    if t.is_empty() {
        "-".to_string()
    } else {
        t
    }
}

/// 压成单行（避免换行破坏 `> quote:` 行解析）。
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ===================== 单元测试 =====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cleanup, temp_dir};

    /// 构造一条"无引用、非阻塞、非回复"的评论入参，其余字段按需用结构体更新语法覆盖。
    fn draft<'a>(step: Option<&'a str>, author: &'a str, text: &'a str) -> NewComment<'a> {
        NewComment {
            step,
            author,
            text,
            quote: None,
            blocking: false,
            reply: None,
        }
    }

    #[test]
    fn set_field_覆盖已有与插入缺失() {
        let l = "<!-- GATE:COMMENT id=C001 state=open -->";
        assert_eq!(
            set_field(l, "state", "resolved"),
            "<!-- GATE:COMMENT id=C001 state=resolved -->"
        );
        assert_eq!(
            set_field(l, "stale", "true"),
            "<!-- GATE:COMMENT id=C001 state=open stale=true -->"
        );
    }

    #[test]
    fn locate_line_首行命中与未命中() {
        let body = "第一行\n第二行 回滚方案\n第三行\n";
        assert_eq!(locate_line(body, "回滚方案"), Some(2));
        assert_eq!(locate_line(body, "不存在"), None);
        assert_eq!(locate_line(body, "   "), None);
    }

    #[test]
    fn context_取前后各一行() {
        let body = "第一行\n第二行\n第三行\n";
        let (b, a) = context(body, 2);
        assert_eq!(b.as_deref(), Some("第一行"));
        assert_eq!(a.as_deref(), Some("第三行"));
        assert_eq!(context(body, 1).0, None, "首行没有上文");
    }

    #[test]
    fn next_id_自增() {
        assert_eq!(next_id(""), "C001");
        assert_eq!(next_id("<!-- GATE:COMMENT id=C003 author=寇工 -->"), "C004");
    }

    #[test]
    fn is_ai_大小写不敏感() {
        assert!(is_ai("ai"));
        assert!(is_ai(" AI "));
        assert!(!is_ai("寇工"));
    }

    #[test]
    fn none_if_dash_空值归一() {
        assert_eq!(none_if_dash("-".into()), None);
        assert_eq!(none_if_dash(String::new()), None);
        assert_eq!(none_if_dash("C001".into()), Some("C001".to_string()));
    }

    #[test]
    fn parse_读取reply锚点与状态() {
        let content = "\
<!-- GATE:COMMENTS req=REQ-001 -->
<!-- GATE:COMMENT id=C001 step=solution author=寇工 ts=2026-09-11_0100 state=open blocking=true line=26 reply=- stale=false -->
> quote: 回滚方案
> ctx_before: 上文
> ctx_after: 下文

正文内容
<!-- /GATE:COMMENT -->
<!-- GATE:COMMENT id=C002 step=- author=ai ts=2026-09-11_0101 state=resolved blocking=false line=- reply=C001 stale=true -->

已补充
<!-- /GATE:COMMENT -->
<!-- /GATE:COMMENTS -->
";
        let cs = parse(content);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].line, Some(26));
        assert!(cs[0].is_blocking_open());
        assert_eq!(cs[0].quote.as_deref(), Some("回滚方案"));
        assert_eq!(cs[0].body, "正文内容");
        assert_eq!(cs[0].reply, None, "reply=- 应解析为 None");
        assert_eq!(cs[1].reply.as_deref(), Some("C001"));
        assert_eq!(cs[1].step, None, "step=- 应解析为 None");
        assert!(cs[1].stale);
        assert!(!cs[1].is_blocking_open(), "已 resolve 不再阻塞");
    }

    #[test]
    fn add_校验步骤键与回复目标() {
        let root = temp_dir("comment-validate");
        crate::requirement::create(&root, None, "校验").unwrap();
        assert!(
            add(&root, "REQ-001", draft(Some("unknown"), "寇工", "文本")).is_err(),
            "未知步骤键应被拒"
        );
        assert!(
            add(
                &root,
                "REQ-001",
                NewComment {
                    reply: Some("C999"),
                    ..draft(None, "寇工", "文本")
                }
            )
            .is_err(),
            "回复不存在的评论应被拒"
        );
        assert!(
            add(&root, "REQ-001", draft(None, "寇工", "   ")).is_err(),
            "空评论应被拒"
        );
        cleanup(&root);
    }

    #[test]
    fn 评论链路_ai只能回复且事件入审计() {
        let root = temp_dir("comment-flow");
        crate::requirement::create(&root, None, "评论链路").unwrap();

        let c1 = add(
            &root,
            "REQ-001",
            NewComment {
                quote: Some("回滚方案"),
                blocking: true,
                ..draft(Some("solution"), "寇工", "数据库迁移的回退步骤需要明确")
            },
        )
        .unwrap();
        assert_eq!(c1.id, "C001");
        assert!(c1.is_blocking_open());
        assert_eq!(summary(&root, "REQ-001").unwrap(), (1, 1));

        // AI 不能新开评论
        let e = add(&root, "REQ-001", draft(None, "ai", "我改了")).unwrap_err();
        assert!(
            e.to_string().contains("只能回复"),
            "AI 新开评论应被拒：{}",
            e
        );

        // AI 回复
        let c2 = add(
            &root,
            "REQ-001",
            NewComment {
                reply: Some("C001"),
                ..draft(None, "ai", "已补充回退步骤")
            },
        )
        .unwrap();
        assert_eq!(c2.reply.as_deref(), Some("C001"));

        // reply 必须落盘并能被重新解析出来
        let parsed = list(&root, "REQ-001").unwrap();
        let back = parsed.iter().find(|c| c.id == "C002").unwrap();
        assert_eq!(back.reply.as_deref(), Some("C001"), "reply 必须持久化");

        // AI 不能 resolve
        assert!(
            resolve(&root, "REQ-001", "C001", "ai").is_err(),
            "AI 不得关闭评论"
        );
        resolve(&root, "REQ-001", "C001", "寇工").unwrap();
        assert_eq!(
            summary(&root, "REQ-001").unwrap(),
            (1, 0),
            "关闭后不再有阻塞评论"
        );

        let audit = fs::read_to_string(root.join(".gates/audit/gate-audit.log")).unwrap();
        assert!(audit.contains("COMMENT-ADD"), "评论新增应入审计：{}", audit);
        assert!(
            audit.contains("COMMENT-RESOLVE"),
            "评论关闭应入审计：{}",
            audit
        );
        cleanup(&root);
    }

    #[test]
    fn refresh_anchors_命中保持与失效标记() {
        let root = temp_dir("comment-anchors");
        crate::requirement::create(&root, None, "锚点").unwrap();
        add(
            &root,
            "REQ-001",
            NewComment {
                quote: Some("风险点与回滚方案"),
                blocking: true,
                ..draft(Some("solution"), "寇工", "补充")
            },
        )
        .unwrap();
        assert_eq!(
            refresh_anchors(&root, "REQ-001").unwrap(),
            0,
            "正文未变仍应命中"
        );

        add(
            &root,
            "REQ-001",
            NewComment {
                quote: Some("这段引用在正文中并不存在"),
                ..draft(None, "寇工", "补充2")
            },
        )
        .unwrap();
        assert_eq!(
            refresh_anchors(&root, "REQ-001").unwrap(),
            1,
            "未命中应标记 stale"
        );
        cleanup(&root);
    }
}
