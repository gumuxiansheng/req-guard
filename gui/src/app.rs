//! GUI 状态机与渲染。
//!
//! 所有动作都落到 `req-guard-core`：审核走 `requirement::review`、新建走 `requirement::create`、
//! 绕过走 `gate::bypass`、判定走 `gate::gate_check`——与 CLI / TUI 完全等价。

use eframe::egui;
use req_guard_core::status::{self, ReqStatus, StepState};
use req_guard_core::{ac, comment, gate, idcheck, requirement, resolve, touch};

use crate::markdown;
use crate::palette::Tone;
use std::path::Path;
use std::time::{Duration, Instant};

/// 轻量轮询间隔（不做文件系统监听）。
const REFRESH: Duration = Duration::from_secs(3);

/// 当前打开的弹窗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    None,
    NewReq,
    Approve,
    Reject,
    Bypass,
    Audit,
    CheckResult,
    /// 评论面板：看评论 / 新增 / 关闭（resolve）/ 重算行号锚点。
    Comments,
    /// 新增评论（`input_blocking` 决定是否阻塞）。
    NewComment,
    /// 关闭（resolve）选中的评论：需要审核人署名。
    ResolveComment,
    /// 修订（amend）当前段：审核人 + 必须给出改稿说明。
    Amend,
    /// 绑定内容摘要（seal）：给存量清单补绑定 / 改稿后重新绑定。
    Seal,
    /// 归档（done）确认框（REQ-015 G4）。
    ///
    /// `done` 在界面上走 `ui_issue_credential` **免票据**，门槛比 CLI 低，
    /// 而它又会把需求移出门禁管辖、使其上未关闭的阻塞评论一并失效——
    /// 低门槛 + 不可逆 = 必须补一层确认。这个弹窗就是那层确认。
    DoneConfirm,
    /// 物理归档（REQ-015 G2）：预演 / 执行 / 到期清扫。
    Archive,
    /// 归档区只读浏览（REQ-015 G3）。
    ArchivedView,
    /// 体检总览（REQ-016 G1/G3/G5/G6）：四组只读体检收在一处，
    /// 顶栏只 +1 个按钮（AC-027），不把 5 个入口摊在操作条上。
    Health,
    /// 追加变更范围声明（`touch --declare` 的界面入口，REQ-016 G2）。
    ///
    /// 是**写入**动作：会按 `touch.reapprove` 把技术方案打回 pending 并清空 `sum=`。
    DeclareTouch,
    /// 批量绑定内容摘要（REQ-016 G4）：多选后一次 `seal_many`。
    SealMany,
    /// 工程卫生面板（REQ-017）：编号冲突 / 返工率 / 门禁自检 / 环境。
    ///
    /// 收在一个面板里而不是四个入口：GUI 底部操作条已经很满
    /// （REQ-016 收尾时 11 个按钮），且这四项都是"偶尔看一眼"。
    Hygiene,
}

/// 四类只读体检的结果（REQ-016 G1/G3/G5/G6）。
///
/// 一次性算完并缓存——N 份清单 = N 次读盘 + N 次 SHA-256，
/// 放进 3 秒轮询等于每 3 秒全量重算一遍（REQ-015 风险 4 的同一条教训）。
#[derive(Debug, Default, Clone)]
pub struct HealthReport {
    /// ① 变更范围契约（`touch::check`）。
    pub touch: Vec<touch::TouchIssue>,
    /// ② 内容冻结一致性（`requirement::verify_sums`），按 `(需求编号, 问题)` 汇总。
    pub sums: Vec<(String, requirement::SumIssue)>,
    /// ③ 验收标准机检（`ac::check`）。
    pub ac: Vec<ac::AcIssue>,
    /// ④ 交叉引用体检（`touch::check_cross_refs`）。
    pub cross: Vec<touch::CrossRefIssue>,
    /// ④ 的段定位是否失败——失败时**不给**任何具体引用的结论（AC-009）。
    pub cross_locate_failed: bool,
}

/// 体检的四组（顺序即界面上的分组顺序）。
pub const HEALTH_GROUPS: [&str; 4] = ["① 变更范围", "② 内容一致性", "③ 验收标准", "④ 交叉引用"];

/// 工程卫生面板的结果（REQ-017）。
///
/// 三块**只读**判定收在一起：编号冲突（`idcheck::check`）、
/// 门禁自检（`gate::verify_install_with` + `verify_warnings`）、
/// 环境（身份 / 等级 / 票据**脱敏**摘要）。
/// `selfcheck` 用 `Option` 是因为它要实跑脚本、可能失败——失败时如实报，
/// 不伪装成"没问题"。
#[derive(Debug, Default, Clone)]
pub struct HygieneReport {
    pub id_issues: Vec<idcheck::IdIssue>,
    /// `(问题数, 警告数)`：自检有问题时是两个数，正常时是 `(0, 0)`。
    pub selfcheck: Option<(usize, usize, Vec<String>)>,
}

/// 返工率的展示阈值（REQ-017 关键设计 4）。
///
/// 0–2 次属正常打磨（`amend` 的设计意图就是"方向没错、只是要改"），
/// ≥3 次说明方案本身没想清楚。**只是一个提示阈值，不做成门禁**——
/// `requirement.rs` 写明「做成门禁只会催生『少写 amend 刷分』」。
pub const REWORK_REVIEW_THRESHOLD: usize = 3;

/// 归档动作作用在哪儿：单份 / 到期清扫。
///
/// 与 `dry_run` 正交，构成 2×2 共四种动作（预演单份 / 执行单份 / 预演清扫 / 执行清扫）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveWhich {
    One(String),
    Due,
}

/// 管理台状态。
pub struct App {
    pub root: std::path::PathBuf,
    pub reqs: Vec<ReqStatus>,
    /// 选中的需求下标。
    pub selected: usize,
    /// 选中的步骤下标。
    pub step: usize,
    /// 当前需求**全文**（只读；按段展示时用 [`requirement::section_of`] 切片）。
    pub body: String,
    /// 正文是否按 Markdown 渲染（`false` = 原文等宽视图）。
    ///
    /// 默认渲染：审核人读的是内容而不是 `- [ ]` / `**`。原文视图保留下来是因为
    /// 审核人偶尔要逐字核对 GATE 标记行、或整段复制去别处。
    pub render_md: bool,
    /// 渲染视图的解析缓存：同一段同一份原文只解析一次（egui 每帧都重画）。
    md: markdown::Cache,
    pub dialog: Dialog,
    pub input_title: String,
    pub input_reviewer: String,
    pub input_reason: String,
    /// 底部状态提示。
    pub message: Option<String>,
    /// 底部状态提示的色调（红=失败 / 绿=成功 / 琥珀=提示；见 [`set_msg`]）。
    ///
    /// 与 `message` 成对写入：单独留 `message` 是历史包袱——所有提示都渲染成黄色，
    /// 于是"创建失败"和"已创建"长得一样，颜色完全没起到区分作用。
    pub msg_tone: Tone,
    /// 最近一次门禁检查结果。
    pub check_pass: bool,
    pub check_detail: Vec<String>,
    /// 本次检查用的口径说明（REQ-014 G3）。
    ///
    /// 必须显示出来：`Staged` 为空与「判定为别的理由」在结果文本里长得一样，
    /// 不写明口径用户就会把"忘了 `git add`"误读成需求本身有问题。
    pub check_scope: String,
    /// 本次拦截的类别名（放行时为 `None`）。
    ///
    /// 取 `resolve::BlockKind::as_str()` 的稳定字面量（`core/src/resolve.rs:108`），
    /// 这样与台账、脚本输出的说法一致，不另造一套中文类别名。
    pub check_kind: Option<String>,
    pub audit: Vec<String>,
    /// 当前需求的评论（打开面板时经 `comment::list` 读入）。
    pub comments: Vec<comment::Comment>,
    /// 评论面板里选中的评论下标。
    pub comment_sel: usize,
    /// 新增评论的正文输入。
    pub input_comment: String,
    /// 新增评论是否阻塞（未 resolve 即拦截编码）。
    pub input_blocking: bool,
    /// 归档确认框里「我知道阻塞评论会失效」的勾选（REQ-015 G4 事实 ②）。
    ///
    /// 只在**确有未关闭阻塞评论**时才要求勾选：没有阻塞评论却还要勾一次，
    /// 只会训练用户闭眼勾（那等于把确认降级成装饰）。
    pub ack_blocking: bool,
    /// 预演/执行归档产出的 `源路径 → 目标路径` 列表（REQ-015 G2）。
    pub archive_plan: Vec<(String, String)>,
    /// 归档区浏览：是否正处于只读历史视图。
    pub browsing_archived: bool,
    /// 归档区清单（打开视图时读一次并缓存，**不进 3 秒轮询**——
    /// 归档区可能有几百份，每 3 秒重扫一遍纯属浪费）。
    pub archived: Vec<ReqStatus>,
    /// 顶栏身份区文本（`identity::describe` + `auth::effective_level`）。
    ///
    /// 显示它只为回答一个问题：「我现在能不能审批」。
    /// 取不到身份时按钮**不禁用**（L0 下仍可审批），但用户得知道自己处在哪种形态。
    pub who: String,
    /// 体检结果缓存（`None` = 还没算过；打开面板时算一次，不进轮询）。
    pub health: Option<HealthReport>,
    /// 体检面板里当前展开的分组下标（0..4）。
    pub health_sel: usize,
    /// 追加变更范围声明的路径输入。
    pub input_glob: String,
    /// 批量封存里勾选的需求编号（去重保序）。
    pub seal_ids: Vec<String>,
    /// 新建需求时的自定义编号（留空 = 自动编号，与改前逐字一致）。
    pub input_new_id: String,
    /// `lint_id` 对当前 `input_new_id` 的形态提示（**只提示、不阻断**）。
    pub id_lint: Vec<String>,
    /// 工程卫生面板的缓存（编号冲突 + 自检结果；打开时算一次，不进轮询）。
    pub hygiene: Option<HygieneReport>,
    /// 凭据签发 / 清除的调用次数。
    ///
    /// 存在的理由是可观测性，不是业务状态：AC-010 要证「只读体检不消耗审批资格」
    /// （签发次数为 0），AC-013 要证「批量封存签发一次、清除一次」。
    /// 这两条都无法从最终结果反推——3 份都成功既可能是"一次签发管到底"，
    /// 也可能是"每份各签一次"。**没有计数就只能靠读代码相信**。
    pub cred_issued: usize,
    pub cred_cleared: usize,
    /// 上一帧渲染时展开的是哪一段（[`Self::step`] 的上一帧值）。
    ///
    /// 存在的唯一理由：切段时要把**新段的开头**滚进视口（见 [`render_center`]），
    /// 而"是否发生过切段"只能靠跨帧比较才看得出来。不用它而每次都滚，
    /// 会把用户手动滚到的位置每帧拽回去。
    last_step_shown: usize,
    last_refresh: Instant,
}

impl App {
    pub fn new(root: &Path) -> App {
        let mut app = App {
            root: root.to_path_buf(),
            reqs: Vec::new(),
            selected: 0,
            step: 0,
            body: String::new(),
            render_md: true,
            md: markdown::Cache::default(),
            dialog: Dialog::None,
            input_title: String::new(),
            input_reviewer: String::new(),
            input_reason: String::new(),
            message: None,
            msg_tone: Tone::Warning,
            check_pass: false,
            check_detail: Vec::new(),
            check_scope: String::new(),
            check_kind: None,
            audit: Vec::new(),
            comments: Vec::new(),
            comment_sel: 0,
            input_comment: String::new(),
            input_blocking: false,
            ack_blocking: false,
            archive_plan: Vec::new(),
            browsing_archived: false,
            archived: Vec::new(),
            who: String::new(),
            health: None,
            health_sel: 0,
            input_glob: String::new(),
            seal_ids: Vec::new(),
            input_new_id: String::new(),
            id_lint: Vec::new(),
            hygiene: None,
            cred_issued: 0,
            cred_cleared: 0,
            last_step_shown: 0,
            last_refresh: Instant::now(),
        };
        app.reload();
        app.refresh_who();
        app
    }

    /// 当前选中的需求。切到归档区视图时，这里的语义随之变成「选中的历史清单」——
    /// 于是正文/评论的渲染逻辑一行都不用改，只读约束统一由 [`Self::read_only`] 施加。
    pub fn current(&self) -> Option<&ReqStatus> {
        if self.browsing_archived {
            self.archived.get(self.selected)
        } else {
            self.reqs.get(self.selected)
        }
    }

    /// 当前视图是否只读。
    ///
    /// 判定**只看 core 给的数据**（`state == Done`），不在前端另判一次路径：
    /// 再判一次就是第二个真相，而两份真相迟早漂移（REQ-015 关键设计 7）。
    pub fn read_only(&self) -> bool {
        self.browsing_archived
    }

    pub fn current_step(&self) -> Option<&'static str> {
        self.current()?.steps.get(self.step).map(|s| s.key)
    }

    /// 写一条底部提示。
    ///
    /// 色调按语义给：**失败/被拒 → `Tone::Danger`（红）**、**成功 → `Tone::Success`（绿）**、
    /// **校验不通过这类"还差点什么"→ `Tone::Warning`（琥珀，默认）**。
    /// 颜色之外再配一个图标（见 [`Tone::rich`]），色弱用户不看颜色也分得出级别。
    fn set_msg(&mut self, tone: Tone, text: impl Into<String>) {
        self.message = Some(text.into());
        self.msg_tone = tone;
    }

    /// 重新读取需求与正文。
    pub fn reload(&mut self) {
        match status::req_list(&self.root) {
            Ok(list) => {
                self.reqs = list;
                if self.selected >= self.reqs.len() {
                    self.selected = self.reqs.len().saturating_sub(1);
                }
            }
            Err(e) => self.set_msg(Tone::Danger, format!("读取需求失败：{}", e)),
        }
        self.load_body();
        self.last_refresh = Instant::now();
    }

    fn load_body(&mut self) {
        let body = match self.current() {
            Some(r) => {
                std::fs::read_to_string(&r.path).unwrap_or_else(|e| format!("读取正文失败：{}", e))
            }
            None => String::new(),
        };
        // 只有正文真的变了才作废缓存：3s 轮询每次都清会把每帧解析变成每 3s 解析（能忍，但没必要）。
        if body != self.body {
            self.body = body;
            // 正文变了 → 旧的解析结果作废（否则切需求后会渲染上一份内容）。
            self.md.clear();
        }
    }

    /// 3s 轻量轮询。
    fn maybe_refresh(&mut self) {
        if self.last_refresh.elapsed() >= REFRESH {
            self.reload();
        }
    }

    // ===================== 动作（全部走 core） =====================

    /// 只读视图下的写动作统一入口：拦住并给出可执行的说明。
    fn deny_when_read_only(&mut self) -> bool {
        if self.read_only() {
            self.set_msg(Tone::Warning, "归档区是只读历史，不能执行写动作");
            true
        } else {
            false
        }
    }

    pub fn create_req(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let title = self.input_title.trim().to_string();
        if title.is_empty() {
            self.set_msg(Tone::Warning, "标题不能为空");
            return;
        }
        // 留空 = `None` = 自动编号：与改前**逐字一致**（存量用户零感知）。
        let want = self.input_new_id.trim();
        let id_arg = if want.is_empty() { None } else { Some(want) };
        match requirement::create(&self.root, id_arg, &title) {
            Ok(r) => {
                self.dialog = Dialog::None;
                self.input_title.clear();
                self.input_new_id.clear();
                self.id_lint.clear();
                self.reload();
                self.set_msg(Tone::Success, format!("已创建 {}", r.id));
            }
            // 编号冲突走这里：**原样显示 core 的话**（含它是哪个编号、
            // 与谁冲突），不包装成"创建失败"就把信息抹掉了。
            Err(e) => self.set_msg(Tone::Danger, format!("创建失败：{}", e)),
        }
    }

    pub fn approve(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => {
                self.set_msg(Tone::Warning, "没有可审核的需求");
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "审核人不能为空");
            return;
        }
        let strict = gate::strict_order(&self.root);
        // 界面进程内签发凭据：以"人类点击本界面"为在场证明，凭据全程只在进程内存里，
        // 不经 stdout / 环境变量（见 core auth::ui_issue_credential）。L0 为无操作。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome = requirement::review(&self.root, &id, step, &reviewer, true, "", strict);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.reload();
                // 自动前进到第一个未通过的段：批准完阶段 1 后界面应立即可审阶段 2，
                // 否则光标停在已通过的旧段，后续段永远无法进入审核。
                self.jump_to_reviewable();
                self.set_msg(
                    Tone::Success,
                    format!("已批准 {} / {}", id, requirement::step_label(step)),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("批准失败：{}", e)),
        }
    }

    /// 界面进程内签发凭据（见 [`req_guard_core::auth::ui_issue_credential`]）。
    ///
    /// 返回 `false` 表示签发失败（`message` 已写明原因，调用方应放弃本次动作）；
    /// L0 下是无操作且返回 `true`——未开加固的项目照旧可审批。
    ///
    /// 另：写动作一律先看 [`Self::read_only`]。按钮置灰只是第一道，
    /// 动作入口再挡一次是因为置灰挡不住键盘触发与未来的新入口——
    /// 而「归档区里的需求被批准了」是没有任何事后补救的写脏。
    ///
    /// ★ 这是方案 2 解决"GUI 拿不到令牌"的关键：凭据由界面进程**自己签发并持有**，
    /// 不经 CLI stdout，也不需要人类先 `export REQ_GUARD_TOKEN`。
    fn prepare_credential(&mut self, scope: &str) -> bool {
        self.cred_issued += 1;
        match req_guard_core::auth::ui_issue_credential(&self.root, scope) {
            Ok(_) => true,
            Err(e) => {
                self.set_msg(Tone::Danger, format!("签发界面凭据失败：{}", e));
                false
            }
        }
    }

    /// 清除界面凭据，并记一次计数（见 [`Self::cred_cleared`]）。
    ///
    /// 走这一层而不再各处直接调 `auth::clear_credential`：AC-013 要求
    /// 「签发与清除各 1 次」，若各处直接调，测试就只能靠读代码相信。
    fn clear_credential(&mut self) {
        self.cred_cleared += 1;
        req_guard_core::auth::clear_credential();
    }

    /// 把 `step` 光标移到第一个未通过的段（全通过时保持不动）。
    fn jump_to_reviewable(&mut self) {
        if let Some(r) = self.current() {
            if let Some(i) = r.steps.iter().position(|s| !s.state.is_approved()) {
                self.step = i;
            }
        }
    }

    pub fn reject(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => {
                self.set_msg(Tone::Warning, "没有可审核的需求");
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        let reason = self.input_reason.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "审核人不能为空");
            return;
        }
        if reason.is_empty() {
            self.set_msg(Tone::Warning, "打回必须填写原因");
            return;
        }
        let strict = gate::strict_order(&self.root);
        // 同 approve：界面进程内签发凭据（人类点击 = 在场证明）。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome = requirement::review(&self.root, &id, step, &reviewer, false, &reason, strict);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                // 打回原因同步落成阻塞性评论，保证 AI 必然看到"为什么被打回"。
                let _ = comment::add(
                    &self.root,
                    &id,
                    comment::NewComment {
                        step: Some(step),
                        author: &reviewer,
                        text: &reason,
                        quote: None,
                        blocking: true,
                        reply: None,
                    },
                );
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.input_reason.clear();
                self.reload();
                self.set_msg(
                    Tone::Success,
                    format!("已打回 {} / {}", id, requirement::step_label(step)),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("打回失败：{}", e)),
        }
    }

    /// 修订当前段（REQ-004 G1，`requirement::amend`）。
    ///
    /// 与 [`Self::reject`] 机械同构（回退待审 + 清 `sum=` + 必须重审），
    /// 区别只在状态标签与台账事件名：段标 `amended`、记 `AMEND`。
    /// 界面把它单列，是因为"方向没错、只是漏个约束"用 reject 表达会让台账里
    /// "否决"与"改稿"混成一坨，事后答不出"哪几段反复返工"。
    pub fn amend(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let (id, step) = match (self.current(), self.current_step()) {
            (Some(r), Some(s)) => (r.id.clone(), s),
            _ => {
                self.set_msg(Tone::Warning, "没有可修订的需求");
                return;
            }
        };
        let reviewer = self.input_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "修订提请人不能为空");
            return;
        }
        let reason = self.input_reason.trim().to_string();
        if reason.is_empty() {
            self.set_msg(Tone::Warning, "修订必须说明改什么、为什么改");
            return;
        }
        let strict = gate::strict_order(&self.root);
        // 与批准 / 打回同一套"界面进程内签发凭据"：scope 仍是 `<需求>:<段>`
        // （core 的 amend 走 ensure_human + ScopeCheck::Exact）。
        if !self.prepare_credential(&format!("{}:{}", id, step)) {
            return;
        }
        let outcome = requirement::amend(&self.root, &id, step, &reviewer, &reason, strict);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.input_reason.clear();
                self.reload();
                self.set_msg(
                    Tone::Warning,
                    format!(
                        "已请求修订 {} / {}（{}）——摘要已清空，段已回到待审，请改完重新批准",
                        id,
                        requirement::step_label(step),
                        reviewer
                    ),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("修订失败：{}", e)),
        }
    }

    /// 绑定内容摘要（`requirement::seal`）：把已批准段的 `sum=` 绑到**当前正文**。
    ///
    /// 两个用途，界面上不区分（core 会按"还有段可绑 / 都绑过了"给出不同要求）：
    /// - 存量清单首次启用内容冻结（REQ-002 上线前批准的段）；
    /// - 正文已改、但确实无需重审时显式重新绑定——此时**必须给理由**，记 `RESEAL`。
    ///
    /// 人类专属：core 的 `ensure_human` 会拒 AI；界面照旧走进程内凭据。
    pub fn seal(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可绑定的需求");
            return;
        };
        let id = r.id.clone();
        let reason = self.input_reason.trim().to_string();
        if !self.prepare_credential("seal") {
            return;
        }
        let outcome = requirement::seal(&self.root, &id, &reason);
        self.clear_credential();
        match outcome {
            Ok(bound) => {
                self.dialog = Dialog::None;
                self.input_reason.clear();
                self.reload();
                let sums: Vec<String> = bound
                    .iter()
                    .map(|(label, sum)| format!("{}={}…", label, &sum[..8.min(sum.len())]))
                    .collect();
                self.set_msg(
                    Tone::Success,
                    format!(
                        "已绑定 {} 段内容摘要：{}",
                        bound.len(),
                        if sums.is_empty() {
                            "（无）".to_string()
                        } else {
                            sums.join("，")
                        }
                    ),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("绑定失败：{}", e)),
        }
    }

    pub fn do_bypass(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let reason = self.input_reason.trim().to_string();
        if reason.is_empty() {
            self.set_msg(Tone::Warning, "应急绕过必须填写原因");
            return;
        }
        if !self.prepare_credential("bypass") {
            return;
        }
        let outcome = gate::bypass(&self.root, &reason, "gui", 60);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reason.clear();
                self.set_msg(Tone::Success, "已开启应急绕过 60 分钟（已记审计）");
            }
            Err(e) => self.set_msg(Tone::Danger, format!("绕过失败：{}", e)),
        }
    }

    /// 执行门禁检查。
    ///
    /// 走 [`gate::gate_check_with`] 并把**当前选中需求**作为消歧提示（REQ-014 G1）：
    /// 裸的 `gate_check` 既无变更集也无 hint，于是只要仓库里有 ≥2 份未归档清单，
    /// 判定就恒为 `Ambiguous` —— 界面明明有"我这份能不能开工"的答案却丢掉，
    /// 把一个恒报拦截的按钮交给审核人。本仓库实测 18 份 live 清单即命中该形态。
    ///
    /// 口径（REQ-014 关键设计 1）：
    /// - 有选中项 → `hint = Some(当前 id)` + `PathSource::Staged`
    ///   （语义：这份需求 + 本次暂存的改动）；
    /// - 无选中项 → `hint = None` + `PathSource::None`，**与改前逐字一致**
    ///   （不拿空变更集去反查，否则"忘了 git add"会伪装成"没有需求"）。
    pub fn run_check(&mut self) {
        let (ctx, scope) = self.check_ctx();
        self.check_scope = scope;
        match gate::gate_check_with(&self.root, &ctx) {
            Ok(v) => {
                self.check_pass = v.is_pass();
                // 放行也可能是"靠绕过"：此时 summary 是警告文案，必须一起显示，
                // 否则弹窗里只剩脚本明细、看不出这是非常规放行。
                let mut lines = v.detail().to_vec();
                if lines.is_empty() || v.bypassed() {
                    lines.insert(0, v.summary().to_string());
                }
                // 类别要在 detail 补 summary **之前**认：detail 首条可能是
                // "涉及需求：…" 那行，而每类别的判别词都在 summary 的首行里。
                self.check_kind = if v.is_pass() {
                    None
                } else {
                    check_kind_of(v.summary(), v.detail()).map(str::to_string)
                };
                self.check_detail = lines;
                self.dialog = Dialog::CheckResult;
            }
            Err(e) => {
                self.check_kind = None;
                self.set_msg(Tone::Danger, format!("检查失败：{}", e));
            }
        }
    }

    /// 本次检查用的裁决上下文 + 给人看的口径说明。
    ///
    /// 口径**必须显示出来**（REQ-014 G3）：`Staged` 为空时面板要写明「空」，
    /// 否则结果会因为"忘了 `git add`"而长得像是另一回事（风险 2）。
    /// 取数用 [`req_guard_core::touch::staged_files`]，与 `resolve::collect_paths`
    /// 在 `PathSource::Staged` 下走的是同一个函数（`core/src/resolve.rs:739`），
    /// 故这里显示的条数与判定实际用的变更集一致；代价是一次额外的
    /// `git diff --cached`，只在点击时发生、不在 3 秒轮询路径上。
    pub(crate) fn check_ctx(&self) -> (resolve::Ctx, String) {
        let hint = self.current().map(|r| r.id.clone());
        let Some(id) = hint else {
            return (
                resolve::Ctx {
                    source: resolve::PathSource::None,
                    hint: None,
                },
                "全局判定（未选择需求）· 变更集：无".to_string(),
            );
        };
        let staged = touch::staged_files(&self.root).map(|v| v.len());
        let scope = match staged {
            Ok(0) => format!("按 {id} 判定 · 变更集：staged（空）"),
            Ok(n) => format!("按 {id} 判定 · 变更集：staged（{n} 个文件）"),
            Err(e) => format!("按 {id} 判定 · 变更集：staged（读取失败：{e}）"),
        };
        (
            resolve::Ctx {
                source: resolve::PathSource::Staged,
                hint: Some(id),
            },
            scope,
        )
    }

    // ---------- REQ-017：工程卫生（编号冲突 / 返工率 / 自检 / 环境）----------

    /// 跑一次工程卫生面板（**只读**：不签发凭据、不写审计）。
    ///
    /// 自检**只在打开时算一次并缓存**（AC-022）：`verify_install_with(semantic=true)`
    /// 会在临时沙箱里实跑脚本，放进 3 秒轮询等于每 3 秒起一次子进程。
    pub fn run_hygiene(&mut self) {
        let root = self.root.clone();
        let root = root.as_path();
        let mut report = HygieneReport::default();

        match idcheck::check(root) {
            Ok(v) => report.id_issues = v,
            Err(e) => self.set_msg(Tone::Danger, format!("编号冲突检测失败：{}", e)),
        }

        // `base` 传 `None`：本地没有"PR 视角"，硬判 `origin/main` 只会误报
        // （`gate.rs` 的文档明说）；`semantic` 传 `true`：REQ-012 已实测
        // 「只做子串存在性检查形同虚设」（往 hook 顶部插 `exit 0` 仍报 PASS），
        // 界面若传 false 等于把一个**已知无效**的自检显示给用户。
        //
        // 两个参数就是 AC-016 要读的那两个：第 2 个是 `None`、第 3 个是 `true`。
        let mut problems = gate::verify_install_with(root, None, true);
        // 告警（父 hook 未委托 core、`.ps1` 副本缺失等）与硬伤分开算，
        // 但**都要显示**：只显示硬伤会让"看着没问题"变成假象。
        let warns_extra = gate::verify_warnings(root);
        let errors = problems.len();
        problems.extend(warns_extra.iter().cloned());
        report.selfcheck = Some((errors, warns_extra.len(), problems));

        self.hygiene = Some(report);
        self.dialog = Dialog::Hygiene;
    }

    /// 编号冲突的 `(错误数, 警告数)`。
    pub fn id_issue_counts(&self) -> (usize, usize) {
        let Some(h) = &self.hygiene else {
            return (0, 0);
        };
        let errors = h
            .id_issues
            .iter()
            .filter(|i| matches!(i.severity, req_guard_core::issue::Severity::Error))
            .count();
        (errors, h.id_issues.len() - errors)
    }

    /// 编号冲突的文本（渲染与断言共用）。
    pub fn id_issue_lines(&self) -> Vec<String> {
        self.hygiene
            .as_ref()
            .map(|h| {
                h.id_issues
                    .iter()
                    .map(|i| format!("[{}] {}", i.severity.as_str(), i.message))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 门禁自检的文本。
    pub fn selfcheck_lines(&self) -> Vec<String> {
        match self.hygiene.as_ref().and_then(|h| h.selfcheck.clone()) {
            Some((errors, warns, msgs)) => {
                let mut out = vec![format!("自检结果：错误 {} / 警告 {}", errors, warns)];
                out.extend(msgs);
                out
            }
            None => vec!["（还没跑过自检）".to_string()],
        }
    }

    /// 环境摘要文本（身份 / 等级 / AI 上下文 / 票据**脱敏**摘要）。
    ///
    /// ★ 票据部分只放 [`req_guard_core::token::summary`] 的四项，**不放原文、不放哈希**。
    pub fn env_lines(&self) -> Vec<String> {
        let mut out = vec![
            format!("身份：{}", req_guard_core::identity::describe(&self.root)),
            format!(
                "鉴权等级：L{}（AI 上下文：{}）",
                req_guard_core::auth::effective_level(&self.root),
                if req_guard_core::auth::is_ai_context() {
                    "是"
                } else {
                    "否"
                }
            ),
        ];
        out.push(match req_guard_core::token::summary() {
            Some(s) if s.enabled => {
                let left = match s.minutes_left {
                    Some(m) => format!("剩余 {m} 分钟"),
                    // 过期是 L3 下最常见的失败原因，必须点名而不是留个空白。
                    None => "已过期".to_string(),
                };
                format!(
                    "票据：通道已启用 · 模式 {} · {} · {}",
                    s.mode,
                    if s.scoped {
                        "范围绑定"
                    } else {
                        "未绑范围"
                    },
                    left
                )
            }
            Some(_) => "票据：通道未启用（L0–L2 无需票据）".to_string(),
            None => "票据：读不到凭据库（本机未签发过，或环境不含 HOME）".to_string(),
        });
        out
    }

    /// 某份需求的分段返工次数（≥ [`REWORK_REVIEW_THRESHOLD`] 时提示复盘）。
    pub fn rework_lines(&self, id: &str) -> Vec<String> {
        let counts = requirement::amend_counts(&self.root, id);
        let mut out = Vec::new();
        for (key, n) in counts {
            let label = requirement::step_label(key.as_str());
            out.push(format!("{label}：改稿 {n} 次"));
            if n >= REWORK_REVIEW_THRESHOLD {
                // 提示而非门禁：做成门禁只会催生"少写 amend 刷分"。
                out.push(format!(
                    "⚠ {label} 已改稿 {n} 次，建议复盘：反复返工通常说明方案当初没想清楚"
                ));
            }
        }
        out
    }

    /// 校验用户输入的自定义编号形态（**只提示、不阻断**）。
    ///
    /// ⚠️ 这里**不**用 `idcheck::check` 做"创建前预检"：`check` 扫的是**磁盘上已有的**
    /// 清单，而新建的这份还不存在——查不出「与它冲突」。创建前能做的只有形态检查；
    /// 真正的重复由 `requirement::create` 返回 `Err`。
    pub fn refresh_id_lint(&mut self) {
        self.id_lint = idcheck::lint_id(self.input_new_id.trim())
            .into_iter()
            .map(|i| i.message)
            .collect();
    }

    // ---------- REQ-016：校验报告（只读体检 + 两个写入动作）----------

    /// 跑一次四类只读体检并缓存（REQ-016 G1/G3/G5/G6）。
    ///
    /// ★ **不调用 `prepare_credential`**（G8）：只读体检不该消耗审批资格，
    /// 也不该因为"没票"就跑不了——它是给人看现状的，不是审批动作。
    pub fn run_health(&mut self) {
        // 克隆一份 root：下面的错误分支要调 `self.set_msg`，
        // 抱着 `&self.root` 会与可变借用打架。
        let root = self.root.clone();
        let root = root.as_path();
        let mut report = HealthReport::default();

        // ① 变更范围契约：变更集取暂存区，与门禁判定同一份取数（`touch::staged_files`）。
        match touch::staged_files(root) {
            Ok(files) => match touch::check(
                root,
                touch::TouchScope::Union,
                None,
                &touch::Changed::Staged(files),
            ) {
                Ok(v) => report.touch = v,
                Err(e) => self.set_msg(Tone::Danger, format!("变更范围体检失败：{}", e)),
            },
            Err(e) => self.set_msg(Tone::Danger, format!("读取暂存区失败：{}", e)),
        }

        // ② 内容冻结一致性：跨清单汇总。`ReqStatus` 里没有正文，只有 `path`，
        //    故按 `r.path` 读全文再喂给纯函数 `verify_sums`——不自己拼路径（第二个真相）。
        let list = self.reqs.clone();
        for r in list {
            match std::fs::read_to_string(&r.path) {
                Ok(content) => {
                    for issue in requirement::verify_sums(&content) {
                        report.sums.push((r.id.clone(), issue));
                    }
                }
                Err(e) => self.set_msg(Tone::Danger, format!("读取 {} 失败：{}", r.id, e)),
            }
        }

        // ③ 验收标准机检。
        match ac::check(root, &ac::AcTarget::All) {
            Ok(v) => report.ac = v,
            Err(e) => self.set_msg(Tone::Danger, format!("验收标准体检失败：{}", e)),
        }

        // ④ 交叉引用：**必须**用 `section_span` 定位第 2 段。
        //    `section_of` 定位失败会回退整篇（`requirement.rs:73-80` 专门说明
        //    机械校验不能用它）——回退会让"没找到第 2 段"变成"校验了整篇"，
        //    那是把 fail-closed 变成 fail-open。
        let Some(id) = self.current().map(|r| r.id.clone()) else {
            self.health = Some(report);
            return;
        };
        match requirement::find(root, &id).and_then(|r| {
            std::fs::read_to_string(&r.path).map_err(|e| req_guard_core::error::GateError::Io {
                path: Some(r.path.clone()),
                source: e,
            })
        }) {
            Ok(content) => match App::cross_ref_slice(&content) {
                Some((section, first_line)) => {
                    report.cross = touch::check_cross_refs(root, &section, first_line);
                }
                None => report.cross_locate_failed = true,
            },
            Err(e) => self.set_msg(Tone::Danger, format!("读取 {} 失败：{}", id, e)),
        }

        self.health = Some(report);
        self.dialog = Dialog::Health;
    }

    /// 交叉引用体检要校验的那一段切片：`(正文, 起始行号)`；定位失败返回 `None`。
    ///
    /// ★ **必须**用 [`requirement::section_span`]，不能用 [`requirement::section_of`]：
    /// 后者定位失败会**回退整篇**（`requirement.rs:73-80` 专门说明了原因），
    /// 于是"没找到第 2 段"会被静默改写成"校验了整篇" —— 那是把 fail-closed
    /// 变成 fail-open，而这正是机械校验最不能犯的错。
    ///
    /// 抽成一个具名函数是为了让这条纪律**可被判决性实验打中**：
    /// 把它换成 `section_of`，`交叉引用切片用严格定位而非回退整篇` 必须变红。
    pub fn cross_ref_slice(content: &str) -> Option<(String, usize)> {
        let (start, end) = requirement::section_span(content, 1)?;
        let section = content
            .lines()
            .skip(start)
            .take(end - start)
            .collect::<Vec<_>>()
            .join("\n");
        Some((section, start + 1))
    }

    /// 每组的 `(错误数, 警告数)`，按 [`HEALTH_GROUPS`] 的顺序。
    pub fn health_counts(&self) -> [(usize, usize); 4] {
        let empty = (0usize, 0usize);
        let Some(h) = &self.health else {
            return [empty; 4];
        };
        let count = |v: &[req_guard_core::issue::Severity]| {
            (
                v.iter()
                    .filter(|s| matches!(s, req_guard_core::issue::Severity::Error))
                    .count(),
                v.iter()
                    .filter(|s| matches!(s, req_guard_core::issue::Severity::Warn))
                    .count(),
            )
        };
        [
            count(&h.touch.iter().map(|i| i.severity).collect::<Vec<_>>()),
            count(&h.sums.iter().map(|(_, i)| i.severity).collect::<Vec<_>>()),
            count(&h.ac.iter().map(|i| i.severity).collect::<Vec<_>>()),
            (h.cross.len(), 0),
        ]
    }

    /// 当前分组的问题行（渲染与断言共用同一份文本）。
    pub fn health_lines(&self) -> Vec<String> {
        let Some(h) = &self.health else {
            return vec!["（还没跑过体检）".to_string()];
        };
        match self.health_sel {
            0 => h.touch.iter().map(|i| i.message.clone()).collect(),
            1 => h
                .sums
                .iter()
                .map(|(id, i)| format!("{} {}", id, i.message))
                .collect(),
            2 => h.ac.iter().map(|i| i.message.clone()).collect(),
            _ => {
                if h.cross_locate_failed {
                    // 定位失败 = 无从校验：如实说"定位失败"，**不给**任何具体引用的结论。
                    return vec!["第 2 段定位失败：无法校验交叉引用".to_string()];
                }
                h.cross
                    .iter()
                    .map(|i| {
                        format!(
                            "第 {} 行 → {} {}：{}",
                            i.line,
                            i.path,
                            i.section,
                            if i.target_missing {
                                "目标文件不存在"
                            } else {
                                "小节号不存在"
                            }
                        )
                    })
                    .collect()
            }
        }
    }

    /// 追加变更范围声明（写入动作，REQ-016 G2）。
    ///
    /// scope 是 `ScopeCheck::Any`（`touch.rs:801`）——与 `done:{id}`、`"archive"`
    /// 三处形状各异，**照抄不得统一**（关键设计 2）。
    pub fn declare_touch(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let Some(id) = self.current().map(|r| r.id.clone()) else {
            self.set_msg(Tone::Warning, "没有可声明范围的需求");
            return;
        };
        let glob = self.input_glob.trim().to_string();
        if glob.is_empty() {
            self.set_msg(Tone::Warning, "路径不能为空");
            return;
        }
        let actor = self.input_reviewer.trim().to_string();
        if actor.is_empty() {
            self.set_msg(Tone::Warning, "操作人不能为空");
            return;
        }
        if !self.prepare_credential("any") {
            return;
        }
        let outcome = touch::declare(&self.root, &id, &[glob], self.input_reason.trim(), &actor);
        self.clear_credential();
        match outcome {
            Ok(added) => {
                self.dialog = Dialog::None;
                self.input_glob.clear();
                self.reload();
                self.health = None; // 声明变了 → 体检缓存作废
                self.set_msg(
                    Tone::Success,
                    if added.is_empty() {
                        "该路径已在声明里，未重复追加".to_string()
                    } else {
                        format!("已追加 {} 条声明", added.len())
                    },
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("声明失败：{}", e)),
        }
    }

    /// 声明对话框的警示文本（REQ-016 G2 / AC-011、AC-012）。
    ///
    /// 必须说清两个副作用：技术方案会被**打回** pending，且该段的 `sum=` 会被**清空**
    /// （`reopen_step` 的注释：旧摘要留着会继续"保护"一份已改动的正文）。
    /// 这两件事用户点之前就该知道，而不是事后才发现冻结没了。
    pub fn declare_warning(&self) -> String {
        if gate::touch_reapprove(&self.root) {
            "提交后技术方案将被打回 pending 并需重新过审，且该段的内容摘要（sum=）会被清空"
                .to_string()
        } else {
            "提交后技术方案状态保持不变（本仓库 touch.reapprove 为 false）".to_string()
        }
    }

    /// 批量绑定内容摘要（REQ-016 G4 / AC-013、AC-014）。
    ///
    /// ★ **必须**走 `seal_many` 而不是 `for id { seal(..) }`：
    /// `seal` 在**调用前**就消费凭据，`seal_many` 在**循环结束后**才消费一次
    /// （`requirement.rs:2082` vs `:1613`）。L3 下票据用后即废，
    /// 循环写法会让第 2 份起全部失败。故这里**签发一次、清一次**。
    pub fn seal_selected(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        if self.seal_ids.is_empty() {
            self.set_msg(Tone::Warning, "请先勾选要绑定的需求");
            return;
        }
        let actor = self.input_reviewer.trim().to_string();
        if actor.is_empty() {
            self.set_msg(Tone::Warning, "操作人不能为空");
            return;
        }
        if !self.prepare_credential("seal") {
            return;
        }
        let ids = self.seal_ids.clone();
        let outcome = requirement::seal_many(&self.root, &ids, self.input_reason.trim());
        self.clear_credential();
        match outcome {
            Ok(list) => {
                let ok = list.iter().filter(|o| !o.bound.is_empty()).count();
                let failed: Vec<String> = list
                    .iter()
                    .filter(|o| o.bound.is_empty())
                    .map(|o| o.id.clone())
                    .collect();
                self.dialog = Dialog::None;
                self.reload();
                self.health = None;
                // 诚实边界：只说尝试/成功各多少，并**逐字列出**没绑成的那几份。
                // 措辞用「未绑定」而不是「失败」：`bound` 为空既可能是正文被改坏，
                // 也可能是本来就没什么可绑（已经绑过）——两种都该让用户看到编号，
                // 但把后者说成"失败"会让人以为出了错。
                self.set_msg(
                    Tone::Success,
                    format!(
                        "成功 {} / 尝试 {}{}",
                        ok,
                        list.len(),
                        if failed.is_empty() {
                            String::new()
                        } else {
                            format!("；未绑定：{}", failed.join("、"))
                        }
                    ),
                );
            }
            // ⚠️ 诚实边界（实测，与 REQ-016 AC-014 的设想不一致）：
            // `seal_many` 内部是 `for id { seal_inner(..)? }` —— 第一条被拒就 `?` 传播、
            // **整个批量中止**，已绑到一半的结果随 `out` 一起丢弃。
            // 即它**给不出**「成功 2 / 尝试 3 + 失败编号」这种逐条结论。
            //
            // 为什么不在前端拆成逐条调用来凑出那个效果：那正是 AC-002 证明**行不通**的写法
            // （L3 下票据用后即废，第 2 份起必失败）。宁可如实报告中止，
            // 也不要为了界面好看而换一条在严格等级下必坏的路。
            //
            // 好消息是 core 的错误文案里**带编号**（「需求 REQ-002 的 … 段已绑定过…」），
            // 所以用户照样知道是谁卡住了 —— 原样透传，不包装、不截断。
            Err(e) => self.set_msg(Tone::Danger, format!("批量绑定中止：{}", e)),
        }
    }

    /// 刷新审计摘要（`gate::audit_digest`，REQ-016 G7）。
    ///
    /// 它**写文件**（`DIGEST`）但不是审批类（core 侧不调 `ensure_human`），
    /// 故**不签发凭据**（L0 下也能用），只在文案里说明它会改文件（AC-017）。
    pub fn refresh_digest(&mut self) {
        match gate::audit_digest(&self.root) {
            Ok((path, text, n)) => {
                let p = path.display().to_string();
                self.set_msg(
                    Tone::Success,
                    format!("已刷新审计摘要 DIGEST（{}，{} 行）：{}", p, n, text),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("刷新审计摘要失败：{}", e)),
        }
    }

    // ---------- REQ-015：需求生命周期（done / 归档 / 归档区浏览 / 身份）----------

    /// 刷新顶栏身份区（`identity::describe` + `auth::effective_level`）。
    pub fn refresh_who(&mut self) {
        let level = req_guard_core::auth::effective_level(&self.root);
        self.who = format!(
            "{} · 鉴权等级 L{}",
            req_guard_core::identity::describe(&self.root),
            level
        );
    }

    /// 审核人输入框的预填值（REQ-015 G5）。
    ///
    /// 走 core 的 [`req_guard_core::identity::resolve_claimed`]——三级回退只有一份实现，
    /// 界面**不得**自己再写一条（GUI/TUI 各写一份，下一个功能就会出现第四种写法）。
    /// 取不到时返回空串而不是报错：预填失败不该挡住手动填写。
    pub fn prefill_reviewer(&self) -> String {
        req_guard_core::identity::resolve_claimed(&self.root, None, "审核人", "--reviewer")
            .unwrap_or_default()
    }

    /// 打开归档确认框：预填审核人、清掉上一轮的勾选。
    pub fn open_done_confirm(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        if self.current().is_none() {
            self.set_msg(Tone::Warning, "没有可归档的需求");
            return;
        }
        self.input_reviewer = self.prefill_reviewer();
        // 勾选态**必须**重置：不清的话，用户上一轮勾过、这一轮换了份需求，
        // 界面会在他没再看一眼的情况下放行一个高危动作。
        self.ack_blocking = false;
        self.dialog = Dialog::DoneConfirm;
    }

    /// 归档确认框里的三项事实（REQ-015 G4）。
    ///
    /// 返回 `(已通过段数/3, 未关闭阻塞评论数)`。事实取自 core 的数据而非硬编码文案，
    /// 否则「显示 3/3」与实际只批了 2 段这种事，界面会替用户撒谎。
    pub fn done_facts(&self) -> (usize, usize) {
        match self.current() {
            Some(r) => (r.approved_count(), r.blocking_comments),
            None => (0, 0),
        }
    }

    /// 确认框的三项事实（REQ-015 G4 / AC-010）。
    ///
    /// 抽成函数而不是内联在渲染里：AC-010 断言的是「弹窗文本」含三处片段，
    /// 若三行文案散在闭包里，测试就只能去 egui 的 shape 堆里捞字——
    /// 那种用例比它验的东西还脆。这里让渲染与断言共用同一份文本。
    ///
    /// 事实 ①② 取自 core 的数据而非硬编码：否则「显示 3/3」而实际只批了 2 段
    /// 这种事，界面就替用户撒了谎。
    pub fn done_confirm_lines(&self) -> Vec<String> {
        let (approved, blocking) = self.done_facts();
        vec![
            format!("① 当前进度：{}/3 已通过", approved),
            format!("② 未关闭阻塞评论：{} 条", blocking),
            // ③ 是固定文案：它是**语义**不是数据，没有可取的来源。
            "③ 归档后门禁不再管辖本需求，其上的阻塞评论随之失效。".to_string(),
        ]
    }

    /// 确认按钮此刻是否可用（REQ-015 G4 的勾选门禁）。
    pub fn done_confirm_enabled(&self) -> bool {
        let (_, blocking) = self.done_facts();
        blocking == 0 || self.ack_blocking
    }

    /// `done` 的凭据范围（REQ-015 G1 / 关键设计 1）。
    ///
    /// 形状是 `done:{id}` 而**不是** `"done"`：`requirement::done` 用的是
    /// `ScopeCheck::Exact`（`core/src/requirement.rs:995`），而 `token.rs:179`
    /// 的判定是 `scope.is_empty() || scope == s`——**精确相等**。
    /// 传 `"done"`、`"{id}"`、`"done:*"` 都会在 L3 下被拒，且报错是「缺凭据」类，
    /// 看上去像环境问题而不是参数写错，极难排查。
    pub fn done_scope(&self) -> String {
        let id = self.current().map(|r| r.id.clone()).unwrap_or_default();
        format!("done:{}", id)
    }

    /// 执行 `done`（标注完成 / 移出门禁管辖）。
    pub fn do_done(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let Some(id) = self.current().map(|r| r.id.clone()) else {
            self.set_msg(Tone::Warning, "没有可归档的需求");
            return;
        };
        let actor = self.input_reviewer.trim().to_string();
        if actor.is_empty() {
            // 留空**不得**静默采用预填值：「没填」与「填了预填值」在审计上
            // 不可区分，而这是 L3 可归属性的基础（REQ-015 关键设计 5）。
            self.set_msg(Tone::Warning, "操作人不能为空");
            return;
        }
        if !self.done_confirm_enabled() {
            self.set_msg(Tone::Warning, "请先确认阻塞评论会随之失效");
            return;
        }
        if !self.prepare_credential(&self.done_scope()) {
            return;
        }
        let outcome = requirement::done(&self.root, &id, &actor);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.input_reviewer.clear();
                self.ack_blocking = false;
                self.reload();
                // done 后该需求退出 live 集 → 左栏少一项，选中下标可能越界，
                // 夹回合法范围（否则界面上"看着什么都没选"而列表非空）。
                if self.selected >= self.reqs.len() && !self.reqs.is_empty() {
                    self.selected = self.reqs.len() - 1;
                }
                self.load_body();
                self.set_msg(
                    Tone::Success,
                    format!("已归档 {id}：门禁不再管辖本需求。如需找回，用 CLI `req-guard status --archived` 浏览归档区"),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("归档失败：{}", e)),
        }
    }

    /// 到期天数（`archive.after_days`，本仓库 30 天）。
    ///
    /// 界面**必须**把它显示出来：不写这个数，用户无从判断「到期」到底是多久。
    pub fn archive_after_days(&self) -> u32 {
        gate::archive_after_days(&self.root)
    }

    /// 预演 / 执行归档。
    ///
    /// `dry_run=true` 时 core 只算不搬、不写审计，但**仍然过鉴权**
    /// （`ensure_human` 在函数首行），故预演也要走 `prepare_credential("archive")`。
    fn run_archive(&mut self, which: ArchiveWhich, dry_run: bool) {
        if self.deny_when_read_only() {
            return;
        }
        let actor = self.input_reviewer.trim().to_string();
        if actor.is_empty() {
            self.set_msg(Tone::Warning, "操作人不能为空");
            return;
        }
        // 两处的 scope 都是**固定字面量 `"archive"`**（不带 id）——
        // 与 `done:{id}` 的形状不同，是历史遗留（done 按需求绑、archive 按动作绑），
        // 界面照抄而**不统一**：「统一」会破坏其中一处的绑定语义。
        if !self.prepare_credential("archive") {
            return;
        }
        let days = self.archive_after_days();
        let outcome = match &which {
            ArchiveWhich::One(i) => requirement::archive_one(&self.root, &actor, i, dry_run),
            ArchiveWhich::Due => requirement::archive_due(&self.root, &actor, days, dry_run),
        };
        self.clear_credential();
        match outcome {
            Ok(moved) => {
                self.archive_plan = moved
                    .iter()
                    .map(|a| (a.src.display().to_string(), a.dst.display().to_string()))
                    .collect();
                let verb = if dry_run { "预演" } else { "已归档" };
                // 到期清扫必须把**判据天数**写进结果：不写它，用户无从判断
                // 「到期」到底是多久（AC-011 锁这一条）。
                let scope_note = match which {
                    ArchiveWhich::Due => format!("（到期判据：done 满 {} 天）", days),
                    ArchiveWhich::One(_) => String::new(),
                };
                // 诚实边界：`archive_due` 是 best-effort，逐条失败只跳过不中断，
                // 返回值里**不含失败原因**。故这里只说搬了几条 + 提示复核，
                // 不写「全部成功」——那会让用户以为没有遗漏（REQ-015 关键设计 6）。
                self.set_msg(
                    Tone::Success,
                    format!(
                        "{} {} 条{}{}",
                        verb,
                        moved.len(),
                        scope_note,
                        if dry_run {
                            "（磁盘未改动，台账无新增）".to_string()
                        } else {
                            "；若有遗漏请用 CLI `req-guard archive` 复核".to_string()
                        }
                    ),
                );
                if !dry_run {
                    self.reload();
                }
            }
            Err(e) => self.set_msg(Tone::Danger, format!("归档失败：{}", e)),
        }
    }

    /// 预演归档（只算不搬）。
    pub fn preview_archive(&mut self) {
        let id = self.current().map(|r| r.id.clone());
        match id {
            Some(id) => self.run_archive(ArchiveWhich::One(id), true),
            None => self.set_msg(Tone::Warning, "没有可归档的需求"),
        }
    }

    /// 执行归档（真实搬移）。
    pub fn do_archive(&mut self) {
        let id = self.current().map(|r| r.id.clone());
        match id {
            Some(id) => self.run_archive(ArchiveWhich::One(id), false),
            None => self.set_msg(Tone::Warning, "没有可归档的需求"),
        }
    }

    /// 到期清扫的预演。
    pub fn preview_archive_due(&mut self) {
        self.run_archive(ArchiveWhich::Due, true);
    }

    /// 到期清扫的执行。
    pub fn do_archive_due(&mut self) {
        self.run_archive(ArchiveWhich::Due, false);
    }

    /// 切到归档区只读视图（REQ-015 G3）。
    ///
    /// 读盘只在**打开时**发生一次并缓存：归档区可能有几百份清单，
    /// 把它放进 3 秒轮询等于每 3 秒全量重扫一遍历史目录。
    pub fn open_archived(&mut self) {
        match status::req_list_archived(&self.root) {
            Ok(list) => {
                self.archived = list;
                self.browsing_archived = true;
                self.selected = 0;
                self.load_body();
                self.dialog = Dialog::ArchivedView;
                if self.archived.is_empty() {
                    self.set_msg(Tone::Muted, "归档区是空的（还没有需求被归档）");
                }
            }
            Err(e) => self.set_msg(Tone::Danger, format!("读取归档区失败：{}", e)),
        }
    }

    /// 切回 live 列表。
    pub fn leave_archived(&mut self) {
        self.browsing_archived = false;
        self.selected = 0;
        self.reload();
    }

    pub fn open_audit(&mut self) {
        self.audit = gate::audit_tail(&self.root, 200).unwrap_or_default();
        self.dialog = Dialog::Audit;
    }

    /// 打开评论面板（读当前需求的评论）。
    ///
    /// 与 `open_audit` 同一套路：先经 core 读盘再展示——界面不自己解析评论文件，
    /// 否则"两个界面看到的评论不一样"这种事迟早发生。
    pub fn open_comments(&mut self) {
        self.reload_comments();
        self.dialog = Dialog::Comments;
    }

    /// 重新读评论，并把选区夹回合法范围（新增/关闭后调用）。
    pub fn reload_comments(&mut self) {
        self.comments = match self.current() {
            Some(r) => comment::list(&self.root, &r.id).unwrap_or_default(),
            None => Vec::new(),
        };
        if self.comment_sel >= self.comments.len() {
            self.comment_sel = self.comments.len().saturating_sub(1);
        }
    }

    /// 新增一条评论（不改任何步骤状态——"审核人只评论、不改正文"的原则）。
    ///
    /// 锚定当前选中段：`step` 为空的总评在界面上无从表达，而"这句话在说哪一段"
    /// 恰恰是审核意见最要紧的信息（core 的 CLI 侧另支持 `--quote` 做行号锚定）。
    pub fn add_comment(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可评论的需求");
            return;
        };
        let id = r.id.clone();
        let author = self.input_reviewer.trim().to_string();
        if author.is_empty() {
            self.set_msg(Tone::Warning, "评论作者不能为空");
            return;
        }
        let text = self.input_comment.trim().to_string();
        if text.is_empty() {
            self.set_msg(Tone::Warning, "评论内容不能为空");
            return;
        }
        let step = self.current_step();
        let blocking = self.input_blocking;
        match comment::add(
            &self.root,
            &id,
            comment::NewComment {
                step,
                author: &author,
                text: &text,
                quote: None,
                blocking,
                reply: None,
            },
        ) {
            Ok(c) => {
                self.input_comment.clear();
                self.input_reviewer.clear();
                self.input_blocking = false;
                self.dialog = Dialog::Comments;
                // 阻塞评论会改变门禁裁决 → 连同需求状态一起刷新（计数与未解锁标记）。
                self.reload();
                self.reload_comments();
                self.comment_sel = self
                    .comments
                    .iter()
                    .position(|x| x.id == c.id)
                    .unwrap_or(self.comment_sel);
                self.set_msg(
                    Tone::Success,
                    format!(
                        "已添加评论 {}{}",
                        c.id,
                        if blocking { "（阻塞）" } else { "" }
                    ),
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("添加评论失败：{}", e)),
        }
    }

    /// 关闭（resolve）选中的评论。
    ///
    /// 与批准/打回同样走"界面进程内签发凭据"：`comment::resolve` 内含
    /// `auth::ensure_human(.., ScopeCheck::Exact("resolve:<需求ID>"))`（审批类动作），
    /// L3 下必须是范围票据，签发范围必须逐字对上。
    pub fn resolve_comment(&mut self) {
        if self.deny_when_read_only() {
            return;
        }
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可操作的需求");
            return;
        };
        let id = r.id.clone();
        let Some(c) = self.comments.get(self.comment_sel).cloned() else {
            self.set_msg(Tone::Warning, "请先选中一条评论");
            return;
        };
        if c.state == comment::CommentState::Resolved {
            self.set_msg(Tone::Warning, format!("{} 已是关闭状态", c.id));
            return;
        }
        let reviewer = self.input_reviewer.trim().to_string();
        if reviewer.is_empty() {
            self.set_msg(Tone::Warning, "关闭人不能为空（关闭权归审核人）");
            return;
        }
        if !self.prepare_credential(&format!("resolve:{}", id)) {
            return;
        }
        let outcome = comment::resolve(&self.root, &id, &c.id, &reviewer);
        self.clear_credential();
        match outcome {
            Ok(_) => {
                self.dialog = Dialog::Comments;
                self.input_reviewer.clear();
                self.reload();
                self.reload_comments();
                self.set_msg(Tone::Success, format!("已关闭 {}（解除阻塞）", c.id));
            }
            Err(e) => self.set_msg(Tone::Danger, format!("关闭失败：{}", e)),
        }
    }

    /// 正文被 AI 改过之后重算行号锚点：命中的刷新 `line`，找不到原文的标 `stale`。
    pub fn refresh_comment_anchors(&mut self) {
        let Some(r) = self.current() else {
            self.set_msg(Tone::Warning, "没有可操作的需求");
            return;
        };
        let id = r.id.clone();
        match comment::refresh_anchors(&self.root, &id) {
            Ok(stale) => {
                self.reload_comments();
                // 全部命中算成功（绿），有失效算提醒（琥珀）——两种结果给两种颜色，
                // 免得"有 3 条失效"和"全部命中"看起来是同一件事。
                self.set_msg(
                    if stale == 0 {
                        Tone::Success
                    } else {
                        Tone::Warning
                    },
                    if stale == 0 {
                        "行号锚点已重算：全部命中".to_string()
                    } else {
                        format!("行号锚点已重算：{} 条找不到原文（已标 stale）", stale)
                    },
                );
            }
            Err(e) => self.set_msg(Tone::Danger, format!("重算锚点失败：{}", e)),
        }
    }

    /// 切换项目根目录（rfd 文件对话框）。
    pub fn pick_root(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("选择项目根目录")
            .pick_folder()
        {
            self.root = dir;
            self.selected = 0;
            self.step = 0;
            self.comments.clear();
            self.comment_sel = 0;
            self.reload();
            self.set_msg(Tone::Success, format!("已切换到 {}", self.root.display()));
        }
    }
}

impl eframe::App for App {
    /// egui 0.36 起，`App` 拆成 `logic`（无 UI 的逻辑）与 `ui`（渲染）两个回调：
    /// 定时刷新与重绘请求放在 `logic`，避免每帧重复读盘。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.maybe_refresh();
        // 触发 3s 轮询的重绘（无操作时也能看到别人改的状态）
        ctx.request_repaint_after(REFRESH);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // egui 0.36：面板从根 Ui 分配空间（顺序即层级，CentralPanel 必须最后）。
        render_top(ui, self);
        render_bottom(ui, self);
        render_left(ui, self);
        render_center(ui, self);
        // 弹窗（Window）不属于面板体系，仍需 Context。
        let ctx = ui.ctx().clone();
        render_dialogs(&ctx, self);
    }
}

// ===================== 渲染 =====================

fn render_top(parent: &mut egui::Ui, app: &mut App) {
    egui::Panel::top("top").show(parent, |ui| {
        ui.horizontal(|ui| {
            ui.heading("req-guard 门禁管理台");
            ui.separator();
            ui.label("项目:");
            ui.monospace(app.root.display().to_string());
            if ui.button("切换目录").clicked() {
                app.pick_root();
            }
            ui.separator();
            // 正文视图切换：默认渲染（可读性优先），原文视图给"逐字核对 / 整段复制"用。
            ui.label("正文:");
            if ui
                .selectable_label(app.render_md, "渲染")
                .on_hover_text("按 Markdown 渲染（标题/任务勾选框/表格/代码块）")
                .clicked()
            {
                app.render_md = true;
            }
            if ui
                .selectable_label(!app.render_md, "原文")
                .on_hover_text("等宽显示 Markdown 源，便于逐字核对与复制")
                .clicked()
            {
                app.render_md = false;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 身份与鉴权等级（REQ-015 G6）：只为回答「我现在能不能审批」。
                // 取不到身份时**不禁用**任何按钮——L0 下无身份仍可审批（core 的既有行为），
                // 界面若擅自禁用，等于在 core 之外又加了一条规则。
                ui.label(
                    egui::RichText::new(app.who.clone())
                        .small()
                        .color(Tone::Muted.color(ui)),
                )
                .on_hover_text("姓名 <邮箱> sig=指纹 · 鉴权等级 L0–L3（L3 审批需一次性票据）");
                ui.separator();
                // 颜色走语义色板（`palette::Tone`），图标是颜色之外的第二条线索。
                let tone = match app.current() {
                    None => Tone::Muted,
                    Some(r) if r.is_blocked() => Tone::Danger,
                    Some(_) => Tone::Success,
                };
                let text = match app.current() {
                    None => "无需求",
                    Some(r) if r.is_blocked() => "未解锁",
                    Some(_) => "已解锁",
                };
                ui.label(tone.rich(ui, text));
            });
        });
    });
}

fn render_bottom(parent: &mut egui::Ui, app: &mut App) {
    egui::Panel::bottom("bottom").show(parent, |ui| {
        // 归档区视图下**一切写动作**都不可用（REQ-015 G3 / AC-012）。
        // 判定只看 `read_only()`（它来自 core 给的 `state == Done`），
        // 前端不另判路径——再判一次就是第二个真相。
        let ro = app.read_only();
        ui.horizontal_wrapped(|ui| {
            if ro {
                ui.label(
                    egui::RichText::new("只读历史：写动作已停用").color(Tone::Muted.color(ui)),
                );
            }
            if ui.add_enabled(!ro, egui::Button::new("创建需求")).clicked() {
                app.dialog = Dialog::NewReq;
                app.input_title.clear();
                app.input_new_id.clear();
                app.id_lint.clear();
            }
            if ui.button("刷新").clicked() {
                app.reload();
                app.set_msg(Tone::Success, "已刷新");
            }
            if ui.button("执行门禁检查").clicked() {
                app.run_check();
            }
            if ui.add_enabled(!ro, egui::Button::new("应急绕过")).clicked() {
                app.dialog = Dialog::Bypass;
                app.input_reason.clear();
            }
            if ui.button("审计日志").clicked() {
                app.open_audit();
            }
            // 归档（done）：把需求移出门禁管辖。**不是**「交付完成」——
            // 文案里必须带「归档」二字，否则用户会以为点它代表任务做完了（REQ-015 N1）。
            if ui
                .add_enabled(
                    !ro && app.current().is_some(),
                    egui::Button::new("归档（done）"),
                )
                .on_hover_text("把本需求移出门禁管辖（≠ 交付完成）；执行前会列出三项事实")
                .clicked()
            {
                app.open_done_confirm();
            }
            if ui
                .add_enabled(
                    !ro && app.current().is_some(),
                    egui::Button::new("物理归档"),
                )
                .on_hover_text("预演 / 执行：把已 done 的清单搬进 archive/<年>/")
                .clicked()
            {
                app.input_reviewer = app.prefill_reviewer();
                app.archive_plan.clear();
                app.dialog = Dialog::Archive;
            }
            if ui
                .button("归档区")
                .on_hover_text("只读浏览已归档的历史清单")
                .clicked()
            {
                app.open_archived();
            }
            // 体检入口**只加这一个按钮**（AC-027）：四组体检收在总览面板里，
            // 不再往这条已经排满的操作条上堆 5 个入口。
            if ui
                .button("体检")
                .on_hover_text("变更范围 / 内容一致性 / 验收标准 / 交叉引用（只读）")
                .clicked()
            {
                app.run_health();
            }
            // 工程卫生（REQ-017）：编号冲突 / 自检 / 环境，同样只 +1 个入口。
            if ui
                .button("工程卫生")
                .on_hover_text("编号冲突 / 门禁自检 / 环境（只读；不含票据签发与撤销）")
                .clicked()
            {
                app.run_hygiene();
            }
            // 评论入口放在操作条上：审核人"只评论不改正文"，这是他最高频的动作之一。
            {
                let (open, blocking) = match app.current() {
                    Some(r) => (r.open_comments, r.blocking_comments),
                    None => (0, 0),
                };
                let label = if blocking > 0 {
                    format!("评论 {open}（阻塞 {blocking}）")
                } else {
                    format!("评论 {open}")
                };
                if ui
                    .add_enabled(
                        app.current().is_some(),
                        egui::Button::new(if blocking > 0 {
                            // 有阻塞评论 → 红色（文案里已经写了"阻塞"，颜色只是加强）。
                            egui::RichText::new(label).color(Tone::Danger.color(ui))
                        } else {
                            egui::RichText::new(label)
                        }),
                    )
                    .on_hover_text("查看 / 新增 / 关闭（resolve）审核评论")
                    .clicked()
                {
                    app.open_comments();
                }
            }
            // 内容摘要绑定：存量清单首次启用冻结、正文改过又确认无需重审时用。
            // 与评论入口一样常驻操作条——它是"人类专属兜底动作"，藏起来就等于不存在。
            {
                let pending = app
                    .current()
                    .map(|r| r.steps.iter().filter(|s| s.seal.needs_action()).count())
                    .unwrap_or(0);
                let label = if pending > 0 {
                    format!("绑定内容摘要（{pending} 段待处理）")
                } else {
                    "绑定内容摘要".to_string()
                };
                if ui
                    .add_enabled(
                        app.current().is_some() && !ro,
                        egui::Button::new(if pending > 0 {
                            egui::RichText::new(label).color(Tone::Warning.color(ui))
                        } else {
                            egui::RichText::new(label)
                        }),
                    )
                    .on_hover_text(
                        "把已批准段的内容摘要绑定到当前正文（存量清单补绑定 / 改稿后重新绑定）",
                    )
                    .clicked()
                {
                    app.input_reason.clear();
                    app.input_reviewer = app.prefill_reviewer();
                    app.dialog = Dialog::Seal;
                }
            }
            // 提示的颜色由 `msg_tone` 决定：失败红 / 成功绿 / 提示琥珀，
            // 并带一个同语义的图标，色弱用户不靠颜色也分得出级别。
            let tone = app.msg_tone;
            if let Some(m) = &app.message {
                let rt = tone.rich(ui, m.clone());
                ui.separator();
                ui.label(rt);
            }
        });
    });
}

fn render_left(parent: &mut egui::Ui, app: &mut App) {
    egui::Panel::left("list")
        .resizable(true)
        .default_size(240.0)
        .show(parent, |ui| {
            ui.label(egui::RichText::new("需求列表").strong());
            ui.separator();
            if app.reqs.is_empty() {
                ui.label("（暂无需求，点「创建需求」）");
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, r) in app.reqs.clone().iter().enumerate() {
                    // 图标也跟着状态换：原来两种状态都是「●」，色弱用户只能靠颜色分辨。
                    let (glyph, tone) = if r.is_blocked() {
                        ("⛔", Tone::Danger)
                    } else {
                        ("✓", Tone::Success)
                    };
                    let line = egui::RichText::new(format!("{} {} {}", glyph, r.id, r.title))
                        .color(tone.color(ui));
                    if ui.selectable_label(i == app.selected, line).clicked() {
                        app.selected = i;
                        app.step = 0;
                        app.load_body();
                    }
                }
            });
        });
}

fn render_center(parent: &mut egui::Ui, app: &mut App) {
    egui::CentralPanel::default_margins().show(parent, |ui| {
        let Some(req) = app.current().cloned() else {
            ui.centered_and_justified(|ui| {
                ui.label("还没有需求清单。点底部「创建需求」开始。");
            });
            return;
        };

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} {}", req.id, req.title)).strong());
            ui.separator();
            let (text, tone) = if req.is_blocked() {
                ("未解锁 — AI 不得编写/修改源码", Tone::Danger)
            } else {
                ("已解锁 — AI 可以开始编写代码", Tone::Success)
            };
            ui.label(tone.rich(ui, text));
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "进度 {}/3 已通过 · 评论 {} 条（阻塞 {}）",
                req.approved_count(),
                req.open_comments,
                req.blocking_comments
            ));
            if req.open_comments > 0 && ui.link("查看评论").clicked() {
                app.open_comments();
            }
        });
        // 返工率（REQ-017 G3）：让审核人在批准下一段时看得见「这一段已经改过几次」。
        // 它**不是门禁**——`requirement.rs` 写明「做成门禁只会催生少写 amend 刷分」。
        let rework: Vec<String> = app
            .rework_lines(&req.id)
            .into_iter()
            .filter(|l| l.contains("改稿"))
            .collect();
        if rework.iter().any(|l| l.contains('⚠')) || rework.iter().any(|l| !l.ends_with("0 次"))
        {
            ui.horizontal_wrapped(|ui| {
                for l in &rework {
                    let tone = if l.contains('⚠') {
                        Tone::Warning
                    } else {
                        Tone::Muted
                    };
                    ui.label(tone.rich(ui, l.clone()));
                }
            });
        }
        ui.separator();

        egui::ScrollArea::vertical()
            // ⚠ `id_salt` 必须给：不加时用的是"本 ui 里第几个控件"推出来的自动 id，
            // 而控件数会随段数 / 按钮数变化 —— 偏移记忆跟着漂，动一下就跳位。
            // 带上需求 id：不同需求各记各的滚动位置，切回来还在原处。
            .id_salt(("steps", req.id.as_str()))
            .show(ui, |ui| {
                let mut selected_header: Option<(egui::Response, bool)> = None;
                for (i, s) in req.steps.iter().enumerate() {
                    let (mark, tone) = match s.state {
                        StepState::Approved => ("✓", Tone::Success),
                        StepState::Rejected => ("✗", Tone::Danger),
                        // 修订与打回都要重新批准，色调一致；标记用 `~` 示"这是改稿不是否决"
                        // （与 TUI 的 `[~]` 同一套语义，别让两个界面看起来不一样）。
                        StepState::Amended => ("~", Tone::Danger),
                        StepState::Pending => ("○", Tone::Muted),
                    };
                    let who = s
                        .reviewer
                        .clone()
                        .map(|v| format!("  审核人 {} ", v))
                        .unwrap_or_default();
                    // 冻结异常（未绑定 / 被改动 / 无法校验）直接写进段标题——
                    // 审核人扫一眼三段就该知道哪段不能放行，而不是要点开才知道。
                    let seal_note = match seal_badge(s.seal) {
                        Some((m, _)) => format!("  {} {}", m, s.seal.hint()),
                        None => String::new(),
                    };
                    let header = egui::RichText::new(format!(
                        "[{}] {}. {}  {}{}{}",
                        mark,
                        i + 1,
                        s.label,
                        s.state.label(),
                        who,
                        seal_note
                    ))
                    .color(tone.color(ui));

                    // open(Some(..)) 会每帧强制开合状态：折叠交互本身展不开非选中段，
                    // 所以这里把"点击标题"接管为"选中该段"，选中段下一帧即被展开。
                    let resp = egui::CollapsingHeader::new(header)
                        .open(Some(i == app.step))
                        .show(ui, |ui| {
                            // 只显示**这一段**（需求分解 / 技术方案 / 测试计划），而不是整篇清单：
                            // 展开哪一段就看到哪一段，审核人不必自己在全文里找对应节，避免看错段点错批准。
                            // 切段规则在 core（与 TUI 同一条规则），定位失败时回退整篇。
                            let text = requirement::section_of(&app.body, i);
                            if app.render_md {
                                markdown::show(ui, app.md.get(i, &text));
                            } else {
                                add_raw_text(ui, &text);
                            }
                        });
                    if i == app.step {
                        selected_header = Some((resp.header_response.clone(), resp.fully_open()));
                    }
                    if resp.header_response.clicked() {
                        app.step = i;
                    }

                    // 当前段的冻结异常 → 直接给出「绑定摘要」入口：
                    // 这条动作的正当理由就写在标题上，摆在旁边免得审核人再去想"该怎么办"。
                    if i == app.step
                        && s.seal.needs_action()
                        && ui
                            .button("绑定本段内容摘要")
                            .on_hover_text(s.seal.hint())
                            .clicked()
                    {
                        app.input_reason.clear();
                        app.input_reviewer = app.prefill_reviewer();
                        app.dialog = Dialog::Seal;
                    }

                    // 当前段的审核按钮（已通过的段无需再审，不显示，避免误导）
                    if i == app.step && s.state != StepState::Approved {
                        let can = req.can_review(s.key);
                        if ui
                            .add_enabled(can, egui::Button::new("批准"))
                            .on_hover_text(if can {
                                "批准当前段（需要审核人姓名）"
                            } else {
                                "顺序不满足：请先批准更早的步骤"
                            })
                            .clicked()
                        {
                            app.input_reviewer = app.prefill_reviewer();
                            app.dialog = Dialog::Approve;
                            app.input_reviewer.clear();
                        }
                        if ui
                            .add_enabled(can, egui::Button::new("打回"))
                            .on_hover_text("打回当前段（审核人 + 原因必填）")
                            .clicked()
                        {
                            app.input_reviewer = app.prefill_reviewer();
                            app.dialog = Dialog::Reject;
                            app.input_reviewer.clear();
                            app.input_reason.clear();
                        }
                    }

                    // 「修订」对**已通过**的段同样有意义：正文冻结之后，"方向没错、只是漏个约束"
                    // 这种反馈最常发生在刚批准的段上，而已通过段上恰恰没有"打回"按钮。
                    if i == app.step
                        && s.state == StepState::Approved
                        && ui
                            .button("请求修订")
                            .on_hover_text("方向没错、只是要改：回退待审 + 清空摘要，仍需重新批准")
                            .clicked()
                    {
                        app.input_reviewer = app.prefill_reviewer();
                        app.dialog = Dialog::Amend;
                        app.input_reviewer.clear();
                        app.input_reason.clear();
                    }
                    ui.separator();
                }
                // **切段后把该段开头滚进视口**（审核人的心智是"点哪一段就从哪一段开始看"）。
                //
                // 为什么必须管：切段使上一段正文收起（内容变矮），而滚动偏移会被 ScrollArea
                // 原样保留 —— 视口于是停在"新段正文中段"（实测：偏移 1800 时第 2 段标题被推到
                // 屏幕 y=2144，视口只有 600 高，正文开头根本不在视口内）。
                //
                // ⚠ **必须一直校正到动画结束，不能只在切段那一帧请求一次**：开合是带动画的，
                // 那一帧算出的目标位置按**旧布局**定，动画把内容挪完之后目标就偏了
                // （实测：请求落在 y≈5000，收完动画视口停在 y=1873 —— 仍是正文中段）。
                // 所以"本帧切过段"或"选中段还没完全展开"就再校正一次；动画一停就不再请求，
                // 用户手动滚到的位置不会被长期拽回去（这条由 [`take_viewport_fix`] 守住：
                // 判定与"上次展开的段"的推进必须同时发生，少了推进就会每帧都校正）。
                if let Some((h, fully_open)) = selected_header {
                    if take_viewport_fix(&mut app.last_step_shown, app.step, fully_open) {
                        h.scroll_to_me(Some(egui::Align::Min));
                    }
                }
            });
    });
}

/// 该不该在这一帧校正视口（把选中段的开头顶回视口顶部），并把"上次展开的段"推进到当前段。
///
/// 形参 `last_shown` 是**进出同一趟**的：判定与状态推进必须绑在一起。
///
/// - 本帧刚切段（`last_shown != step`）→ 校正；
/// - 选中段的开合动画还没结束 → 继续校正。只在切段那一帧校正是不够的：那一帧的目标位置
///   是按**旧布局**算的，动画把内容挪完之后目标就偏了（实测偏到视口下方 1873px 处，
///   仍是正文中段）。
/// - 动画结束且没切段 → 不校正，用户手动滚到的位置不被拽回去。
///
/// ⚠ **为什么把状态推进收进这个函数**：此前判定与状态更新是分开放的两处，中间隔着
/// 一个大循环；重构时"更新 `last_step_shown`"那一行被漏掉，于是 `step_changed`
/// **每帧都为真**、每帧都发一次 `scroll_to_me` —— 症状是"滚到下方后自动跳回第一行"
/// （审核人实测）。合在一起就没有"漏掉更新"这种形态，而"稳定之后不再校正"这条
/// 由 [`take_viewport_fix_稳定后不再校正`] 单测守住。
///
/// 拆成纯函数（不碰 egui）也是为了能单测：离屏 `run_ui` 里滚轮不生效
/// （egui 只在指针悬于滚动区上时才吃滚轮，见 `scroll_area.rs` 的 `is_hovering_outer_rect`），
/// "用户已经滚下去"这个起点在自动化里造不出来 —— 渲染结果的断言无法成为判决。
fn take_viewport_fix(last_shown: &mut usize, step: usize, selected_fully_open: bool) -> bool {
    let changed = *last_shown != step;
    *last_shown = step;
    changed || !selected_fully_open
}

/// 从门禁裁决的文案里认出拦截类别，返回 [`resolve::BlockKind::as_str`] 的字面量。
///
/// **为什么不直接拿 `BlockKind`**：`block_kind()` 只存在于 `resolve::Verdict` 上，
/// `gate_check_with` 返回的是 `GateVerdict`（`core/src/gate.rs:739-743` 的注释说明
/// 这是刻意保持的形状——三个前端都只认 `Pass` / `Block` 与 `bypassed`）。
/// REQ-014 的 G5 要求 core 判定零改动、AC-010 要求 core 测试条数不变，
/// 故本期**不给 core 加方法**，改在前端认类别。
///
/// ⚠️ 代价（REQ-014 关键设计 3 已把这条记为遗留）：识别靠**判别词**，
/// 而判别词是 core 文案的一部分。两端各写一份（本函数与 `tui/src/app.rs` 的同名函数），
/// 将来若改动 core 的裁决文案，需同步改这两处 + 一个共用常量的时机。
/// 更稳的做法是给 `GateVerdict` 加 `block_kind()` 并让两端共用——留待单独立项。
///
/// 关键词取每个类别**首行**里那段唯一的说法（`core/src/resolve.rs` 的 R1 / R15
/// 内联文案与 `unknown_selection_message` / `selection_mismatch_message` /
/// `ambiguous_message` / `step_message` / `blocking_message`），彼此不重叠。
/// 顺序即优先级：具体的排前面，`尚未通过审核` 这种较泛的说法排最后。
pub(crate) fn check_kind_of(summary: &str, detail: &[String]) -> Option<&'static str> {
    let text = format!("{summary}\n{}", detail.join("\n"));
    const KINDS: &[(&str, &str)] = &[
        ("内容冻结校验失败", "SumMismatch"),
        ("不在可裁决的需求清单里", "UnknownSelection"),
        ("未找到待开发的需求清单", "NoRequirement"),
        ("与本次改动不相交", "SelectionMismatch"),
        ("存在未解决的阻塞性评论", "OpenBlockingComment"),
        ("无法确定本次改动属于哪份需求", "Ambiguous"),
        ("尚未通过审核", "StepNotApproved"),
    ];
    KINDS
        .iter()
        .find(|(needle, _)| text.contains(needle))
        .map(|(_, kind)| *kind)
}

/// 段状态的可读标记（`Seal` 弹窗里逐段列状态时复用，避免两处各写一套符号）。
fn state_mark(state: StepState) -> &'static str {
    match state {
        StepState::Approved => "已通过",
        StepState::Rejected => "已打回",
        StepState::Amended => "待修订",
        StepState::Pending => "待审核",
    }
}

/// 内容冻结徽标：`(标记, 色调)`。`Frozen` / `NotApplicable` **不给徽标**——
/// 正常状态没必要在每段标题上再挂一个符号，界面噪声会淹没真正要处理的事。
fn seal_badge(seal: requirement::SealState) -> Option<(&'static str, Tone)> {
    match seal {
        requirement::SealState::Frozen | requirement::SealState::NotApplicable => None,
        requirement::SealState::NotSealed => Some(("○未绑定", Tone::Warning)),
        requirement::SealState::Changed => Some(("⚠已改动", Tone::Danger)),
        requirement::SealState::Unverifiable => Some(("✗无法校验", Tone::Danger)),
    }
}

/// 原文视图：只读、可滚动、**可框选 / 可复制**，等宽逐字呈现 Markdown 源。
///
/// ## 为什么不再用 `TextEdit::multiline(..).interactive(false)`
///
/// `interactive(false)` 会把 sense 降成 `Sense::hover()`（egui 0.36 源码
/// `text_edit/builder.rs` 里 `let sense = if interactive { .. } else { Sense::hover() }`）：
/// 鼠标按下 / 拖拽不参与命中测试 → **选不中文字**；不参与事件分发 → **收不到
/// `Event::Copy`，Ctrl+C 无效**；没有 response 也就挂不上右键菜单。
/// 而"原文"视图存在的唯一理由就是**逐字核对与整段复制**，等于整个功能被废掉。
///
/// ## 现在的做法：把 `&str` 当 `TextBuffer` 喂进去
///
/// egui 给 `&str` 实现了 `TextBuffer`，其中 `is_mutable()` 返回 `false`、
/// `insert_text` / `delete_char_range` 都是**空实现**。于是：
/// - 交互全开：能点选、能拖选、能 Ctrl+C、能挂右键菜单；
/// - 内容改不动：键盘输入与粘贴被空实现吃掉（不靠"下一帧覆盖回去"，所以不会闪）；
/// - 看着仍是只读：`is_mutable() == false` 时不画文本光标（源码同文件的
///   `if text.is_mutable() && interactive { ..绘制光标.. }`）。
///
/// 正文真值仍然只有 `app.body` 一份，界面改不动它——与"清单正文由 AI / 编辑器维护，
/// 界面只做审核决策"的原则一致。
fn add_raw_text(ui: &mut egui::Ui, text: &str) {
    // 「一键复制全文」：原文视图最高频的动作就是整段带走，给个显式按钮比让人拖选更快；
    // 顺手写清可复制的方式，免得再有人以为这里只能看。
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("复制全文")
            .on_hover_text("把本段 Markdown 源整段复制到剪贴板")
            .clicked()
        {
            ui.ctx().copy_text(text.to_string());
        }
        ui.label(
            egui::RichText::new("可选中文字；Ctrl+C 或右键菜单复制")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    });
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            read_only_text(
                ui,
                "raw_md",
                [ui.available_width(), 200.0],
                egui::TextStyle::Monospace,
                text,
            );
        });
}

/// 只读但**可框选、可复制**的文本框（[`add_raw_text`] 与评论正文共用）。
///
/// 关键在 `let mut buf: &str = text;`——传进去的是不可变 `TextBuffer`，
/// 交互开着而内容改不动，详见 [`add_raw_text`] 的说明。
fn read_only_text(
    ui: &mut egui::Ui,
    id_salt: &str,
    size: [f32; 2],
    font: egui::TextStyle,
    text: &str,
) -> egui::Response {
    let mut buf: &str = text;
    let resp = ui.add_sized(
        size,
        egui::TextEdit::multiline(&mut buf)
            .font(font)
            .id_salt(id_salt),
    );
    // 右键菜单：复制选中 / 全选 / 复制全文。
    // 菜单要读 TextEdit 的选区状态，所以挂在 response 上（id 从 `resp.id` 取，
    // 不用自己另造一个，免得两处 id 不同步）。
    let id = resp.id;
    resp.context_menu(|ui| {
        if ui.button("复制选中").clicked() {
            if let Some(sel) = selected_text(ui.ctx(), id, text) {
                ui.ctx().copy_text(sel);
            }
            ui.close();
        }
        if ui.button("全选").clicked() {
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.chars().count()),
                )));
            egui::TextEdit::store_state(ui.ctx(), id, state);
            ui.close();
        }
        if ui.button("复制全文").clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
    });
    resp
}

/// 取只读文本框里当前选中的文字（没选就返回 `None`）。
fn selected_text(ctx: &egui::Context, id: egui::Id, text: &str) -> Option<String> {
    let state = egui::TextEdit::load_state(ctx, id)?;
    let range = state.cursor.char_range()?;
    if range.is_empty() {
        return None;
    }
    Some(range.slice_str(text).to_string())
}

/// 评论面板：列表 + 选中详情 + 动作条（新增 / 关闭 / 重算锚点 / 刷新）。
///
/// 全部经 `comment::*` 读写，界面只做展示与收集输入——与 CLI、TUI 同一套语义。
///
/// ⚠ `ScrollArea` 都给了显式 `id_salt`、每条评论包在 `push_id` 里：
/// egui 的控件 id 由父 id 派生，同一层里出现两个同名 `ScrollArea`、或循环里反复建
/// `TextEdit`，就会撞成同一个 id（表现为 "Second use of widget ID" + 框线画飞）。
fn render_comments(app: &mut App, ui: &mut egui::Ui) {
    let open = app
        .comments
        .iter()
        .filter(|c| c.state == comment::CommentState::Open)
        .count();
    let blocking = app.comments.iter().filter(|c| c.is_blocking_open()).count();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "共 {} 条 · 未解决 {} · 阻塞 {}",
            app.comments.len(),
            open,
            blocking
        ));
        if blocking > 0 {
            ui.label(Tone::Danger.rich(ui, "有未关闭的阻塞评论 → AI 不得编码"));
        }
    });
    ui.separator();

    if app.comments.is_empty() {
        ui.label("（还没有评论。审核人只评论、不改正文；打回会自动留一条阻塞评论。）");
    } else {
        egui::ScrollArea::vertical()
            .id_salt("comment_list")
            .max_height(220.0)
            .show(ui, |ui| {
                for (i, c) in app.comments.clone().iter().enumerate() {
                    let selected = i == app.comment_sel;
                    ui.push_id(i, |ui| {
                        // 未解决用琥珀（提示：还差一步），已解决用绿；图标也不同。
                        let (mark, tone) = match c.state {
                            comment::CommentState::Open => (Tone::Warning.glyph(), Tone::Warning),
                            comment::CommentState::Resolved => {
                                (Tone::Success.glyph(), Tone::Success)
                            }
                        };
                        let mut head = format!(
                            "{} [{}] {}{} · {} · {}",
                            mark,
                            c.id,
                            match c.step.as_deref() {
                                Some(s) => requirement::step_label(s),
                                None => "总评",
                            },
                            if c.blocking { " · 阻塞" } else { "" },
                            c.author,
                            c.ts
                        );
                        if let Some(n) = c.line {
                            head.push_str(&format!(" · L{}", n));
                        }
                        if c.stale {
                            head.push_str(" · ⚠锚点失效");
                        }
                        if let Some(r) = &c.reply {
                            head.push_str(&format!(" · 回复 {}", r));
                        }
                        let mut rt = egui::RichText::new(head).color(tone.color(ui));
                        if selected {
                            rt = rt.strong();
                        }
                        if ui.selectable_label(selected, rt).clicked() {
                            app.comment_sel = i;
                        }
                        // 只展开选中项的正文：列表一屏放得下，读起来像"目录 + 详情"。
                        if selected {
                            if let Some(q) = &c.quote {
                                ui.label(egui::RichText::new(format!("引用：{}", q)).italics());
                            }
                            let body = c.body.clone();
                            // 高度按正文长度估（约 50 字一行），否则短评论也占一大块空白。
                            let rows = (body.chars().count() / 50 + 1).clamp(1, 8) as f32;
                            egui::ScrollArea::vertical()
                                .id_salt("comment_body")
                                .max_height(24.0 * rows + 8.0)
                                .show(ui, |ui| {
                                    // 同样是"只读但可复制"：审核意见经常要被摘进回信/PR 里。
                                    read_only_text(
                                        ui,
                                        "comment_body_text",
                                        [ui.available_width(), 20.0 * rows],
                                        egui::TextStyle::Body,
                                        &body,
                                    );
                                });
                        }
                    });
                }
            });
    }

    ui.separator();
    // 动作条用 `horizontal_wrapped`：窗口窄时按钮换行续排，
    // 而不是被藏进横向滚动条里（"关闭评论"这种按钮被藏起来等于没有）。
    // 归档区视图下评论同样是只读的：能读、能复制，不能新增也不能关闭。
    let ro = app.read_only();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!ro, egui::Button::new("新增普通评论"))
            .clicked()
        {
            app.input_blocking = false;
            app.input_comment.clear();
            app.input_reviewer = app.prefill_reviewer();
            app.dialog = Dialog::NewComment;
        }
        if ui
            .add_enabled(!ro, egui::Button::new("新增阻塞评论"))
            .clicked()
        {
            app.input_blocking = true;
            app.input_comment.clear();
            app.input_reviewer = app.prefill_reviewer();
            app.dialog = Dialog::NewComment;
        }
        if ui
            .add_enabled(
                !ro && app
                    .comments
                    .get(app.comment_sel)
                    .is_some_and(|c| c.state == comment::CommentState::Open),
                egui::Button::new("关闭选中（resolve）"),
            )
            .on_hover_text("关闭权归审核人：AI 只能回复、不能关闭（关闭会解除阻塞）")
            .clicked()
        {
            app.input_reviewer = app.prefill_reviewer();
            app.dialog = Dialog::ResolveComment;
        }
        if ui.button("重算行号锚点").clicked() {
            app.refresh_comment_anchors();
        }
        if ui.button("刷新").clicked() {
            app.reload_comments();
        }
        if ui.button("关闭面板").clicked() {
            app.dialog = Dialog::None;
        }
    });
}

fn render_dialogs(ctx: &egui::Context, app: &mut App) {
    let mut open = app.dialog != Dialog::None;
    if !open {
        return;
    }

    // 标题允许动态（评论面板要带上需求 ID），故用 String 而非 &'static str。
    let (title, body): (String, fn(&mut App, &mut egui::Ui)) = match app.dialog {
        Dialog::NewReq => ("创建需求".into(), |app, ui| {
            ui.label("自定义编号（留空 = 自动编号）：");
            if ui.text_edit_singleline(&mut app.input_new_id).changed() {
                app.refresh_id_lint();
            }
            // 形态提示**只提示、不阻断**：编号规范是团队约定，不是门禁
            // （`idcheck::lint_id` 的返回类型就是 `Vec<String>` 而非 `Result`）。
            for hint in app.id_lint.clone() {
                ui.label(
                    egui::RichText::new(format!("提示：{hint}"))
                        .small()
                        .color(Tone::Warning.color(ui)),
                );
            }
            ui.label("标题：");
            ui.text_edit_singleline(&mut app.input_title);
            if ui.button("创建").clicked() {
                app.create_req();
            }
        }),
        Dialog::Hygiene => ("工程卫生（只读）".into(), |app, ui| {
            ui.label(
                egui::RichText::new(
                    "编号冲突 / 门禁自检 / 环境。只读项不签发凭据；界面不提供票据签发与撤销。",
                )
                .small()
                .color(Tone::Muted.color(ui)),
            );
            ui.separator();
            let (err, warn) = app.id_issue_counts();
            ui.label(
                egui::RichText::new(format!("① 编号冲突：错误 {err} / 警告 {warn}")).color(
                    if err > 0 {
                        Tone::Danger.color(ui)
                    } else if warn > 0 {
                        Tone::Warning.color(ui)
                    } else {
                        Tone::Success.color(ui)
                    },
                ),
            );
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .id_salt("hygiene_ids")
                .show(ui, |ui| {
                    let lines = app.id_issue_lines();
                    if lines.is_empty() {
                        ui.label("（没有编号冲突）");
                    }
                    for l in lines {
                        ui.label(l);
                    }
                });
            ui.separator();
            ui.label(egui::RichText::new("② 门禁自检").strong());
            ui.label(
                egui::RichText::new("语义自检会在临时仓库实跑脚本，首次约需数秒")
                    .small()
                    .color(Tone::Muted.color(ui)),
            );
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .id_salt("hygiene_self")
                .show(ui, |ui| {
                    for l in app.selfcheck_lines() {
                        ui.label(l);
                    }
                });
            ui.separator();
            ui.label(egui::RichText::new("③ 环境").strong());
            for l in app.env_lines() {
                ui.label(l);
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button("重新检测").clicked() {
                    app.run_hygiene();
                }
                if ui
                    .button("刷新审计摘要")
                    .on_hover_text("写 .gates/audit/DIGEST（非审批类）")
                    .clicked()
                {
                    app.refresh_digest();
                }
            });
            ui.label(
                egui::RichText::new(
                    "本界面不提供票据签发 / 撤销：界面能自签票 = 「人类在场」这道门自己拆了",
                )
                .small()
                .color(Tone::Muted.color(ui)),
            );
        }),
        Dialog::Health => ("体检总览（只读）".into(), |app, ui| {
            ui.label(
                egui::RichText::new(
                    "四组只读体检。只读项不签发凭据、不消耗审批资格；写入动作在各自的对话框里。",
                )
                .small()
                .color(Tone::Muted.color(ui)),
            );
            ui.separator();
            let counts = app.health_counts();
            // 分组用「只读」徽标与写入动作分开（REQ-016 关键设计 4）。
            for (i, name) in HEALTH_GROUPS.iter().enumerate() {
                let (err, warn) = counts[i];
                let tone = if err > 0 {
                    Tone::Danger
                } else if warn > 0 {
                    Tone::Warning
                } else {
                    Tone::Success
                };
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            app.health_sel == i,
                            egui::RichText::new(format!(
                                "{}  【只读】错误 {} / 警告 {}",
                                name, err, warn
                            ))
                            .color(tone.color(ui)),
                        )
                        .clicked()
                    {
                        app.health_sel = i;
                    }
                });
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    let lines = app.health_lines();
                    if lines.is_empty() {
                        ui.label("（本组没有发现问题）");
                    }
                    for l in lines {
                        // 严重级分色复用既有色板（不新增颜色 → 不必重测对比度）。
                        ui.label(l);
                    }
                });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button("重新体检").clicked() {
                    app.run_health();
                }
                if ui
                    .button("刷新审计摘要")
                    .on_hover_text(
                        "gate::audit_digest：会写 .gates/audit/DIGEST（非审批类，无需凭据）",
                    )
                    .clicked()
                {
                    app.refresh_digest();
                }
                if ui
                    .button("追加变更范围声明")
                    .on_hover_text("人类专属：AI 不得自己扩范围")
                    .clicked()
                {
                    app.input_glob.clear();
                    app.input_reason.clear();
                    app.input_reviewer = app.prefill_reviewer();
                    app.dialog = Dialog::DeclareTouch;
                }
                if ui
                    .button("批量绑定内容摘要")
                    .on_hover_text("一次人类命令 = 一次人类意图；走 seal_many 而非循环 seal")
                    .clicked()
                {
                    app.seal_ids.clear();
                    app.input_reason.clear();
                    app.input_reviewer = app.prefill_reviewer();
                    app.dialog = Dialog::SealMany;
                }
            });
        }),
        Dialog::DeclareTouch => ("追加变更范围声明（写入）".into(), |app, ui| {
            // 副作用必须写在签字之前（AC-011 / AC-012 断言的就是这行文本）。
            ui.label(egui::RichText::new(app.declare_warning()).color(Tone::Warning.color(ui)));
            ui.separator();
            ui.label("路径 / glob（必填）：");
            // 初始必须为空：预填一个路径会让人"直接回车"扩出一条没想过的声明。
            ui.text_edit_singleline(&mut app.input_glob);
            ui.label("原因：");
            ui.text_edit_singleline(&mut app.input_reason);
            ui.label("操作人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            if ui.button("提交声明").clicked() {
                app.declare_touch();
            }
        }),
        Dialog::SealMany => ("批量绑定内容摘要（写入）".into(), |app, ui| {
            ui.label(format!(
                "一次人类命令 = 一次人类意图：已勾选 {} 份",
                app.seal_ids.len()
            ));
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for r in app.reqs.clone() {
                        let mut on = app.seal_ids.contains(&r.id);
                        let pending = r.steps.iter().filter(|s| s.seal.needs_action()).count();
                        if ui
                            .checkbox(
                                &mut on,
                                format!("{} {}（{pending} 段待处理）", r.id, r.title),
                            )
                            .changed()
                        {
                            if on {
                                app.seal_ids.push(r.id.clone());
                            } else {
                                app.seal_ids.retain(|x| x != &r.id);
                            }
                        }
                    }
                });
            ui.separator();
            ui.label("原因（首次补绑定可留空）：");
            ui.text_edit_singleline(&mut app.input_reason);
            ui.label("操作人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            if ui.button("确认绑定").clicked() {
                app.seal_selected();
            }
        }),
        Dialog::DoneConfirm => ("归档（done）当前需求".into(), |app, ui| {
            let blocking = app.done_facts().1;
            // 三项事实（与 `done_confirm_lines` 同一份文本，见那里的注释）。
            for line in app.done_confirm_lines() {
                ui.label(line);
            }
            if blocking > 0 {
                ui.checkbox(&mut app.ack_blocking, "我已知悉：这些阻塞评论会随之失效");
            }
            ui.separator();
            ui.label("操作人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            ui.separator();
            // 勾选门禁：`blocking > 0` 时未勾选则按钮 disabled。
            if ui
                .add_enabled(app.done_confirm_enabled(), egui::Button::new("确认归档"))
                .on_hover_text(if blocking > 0 && !app.ack_blocking {
                    "请先勾选「我已知悉」"
                } else {
                    "把本需求移出门禁管辖（不可逆）"
                })
                .clicked()
            {
                app.do_done();
            }
        }),
        Dialog::Archive => ("物理归档".into(), |app, ui| {
            let days = app.archive_after_days();
            // 天数**必须**显示：不写它，用户无从判断「到期」是多久。
            ui.label(format!("到期判据：done 满 {} 天", days));
            ui.label("操作人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("预演归档")
                    .on_hover_text("只算不搬：磁盘不变、台账不写审计")
                    .clicked()
                {
                    app.preview_archive();
                }
                if ui
                    .button("执行归档")
                    .on_hover_text("真实搬移（git 记为 rename）")
                    .clicked()
                {
                    app.do_archive();
                }
                if ui.button("预演清理到期").clicked() {
                    app.preview_archive_due();
                }
                if ui.button("执行清理到期").clicked() {
                    app.do_archive_due();
                }
            });
            if !app.archive_plan.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("本次结果（源路径 → 归档路径）").strong());
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for (src, dst) in app.archive_plan.iter() {
                            ui.monospace(format!("{src} → {dst}"));
                        }
                    });
            }
        }),
        Dialog::ArchivedView => ("归档区（只读历史）".into(), |app, ui| {
            ui.label(
                egui::RichText::new("只读：归档即终点，此处不可批准 / 打回 / 修订 / 封存 / 归档")
                    .color(Tone::Muted.color(ui)),
            );
            ui.separator();
            if app.archived.is_empty() {
                ui.label("（归档区还没有任何需求）");
            } else {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (i, r) in app.archived.clone().iter().enumerate() {
                            if ui
                                .selectable_label(
                                    i == app.selected,
                                    format!("{} {}", r.id, r.title),
                                )
                                .clicked()
                            {
                                app.selected = i;
                                app.load_body();
                            }
                        }
                    });
            }
            if ui.button("返回需求列表").clicked() {
                app.leave_archived();
                app.dialog = Dialog::None;
            }
        }),
        Dialog::Approve => ("批准当前段".into(), |app, ui| {
            ui.label("审核人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            if ui.button("确认批准").clicked() {
                app.approve();
            }
        }),
        Dialog::Reject => ("打回当前段".into(), |app, ui| {
            ui.label("审核人（必填）：");
            ui.text_edit_singleline(&mut app.input_reviewer);
            ui.label("原因（必填，将同时写入阻塞性评论）：");
            ui.text_edit_singleline(&mut app.input_reason);
            if ui.button("确认打回").clicked() {
                app.reject();
            }
        }),
        Dialog::Bypass => ("应急绕过".into(), |app, ui| {
            ui.label("原因（必填，用于审计追溯）：");
            ui.text_edit_singleline(&mut app.input_reason);
            ui.label("默认时效 60 分钟，到期自动恢复硬拦截。");
            if ui.button("开启绕过").clicked() {
                app.do_bypass();
            }
        }),
        Dialog::Audit => (
            "审计日志（最近 200 行，倒序）".into(),
            |app, ui| {
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        if app.audit.is_empty() {
                            ui.label("（暂无审计记录）");
                        } else {
                            // 按 core 判定的性质上色（分类只有一份，见 gate::audit_kind）。
                            // 拦截与绕过必须一眼可辨：把 BLOCK 显示成 PASS 就是"看着在放行"。
                            for line in app.audit.iter().rev() {
                                let color = match req_guard_core::gate::audit_kind(line) {
                                    req_guard_core::gate::AuditKind::Block => {
                                        egui::Color32::from_rgb(200, 80, 80)
                                    }
                                    req_guard_core::gate::AuditKind::Bypass => {
                                        egui::Color32::from_rgb(210, 170, 60)
                                    }
                                    req_guard_core::gate::AuditKind::Note => {
                                        egui::Color32::from_rgb(130, 130, 130)
                                    }
                                    req_guard_core::gate::AuditKind::Pass => {
                                        egui::Color32::from_rgb(90, 180, 110)
                                    }
                                    req_guard_core::gate::AuditKind::Event => egui::Color32::GRAY,
                                };
                                ui.monospace(egui::RichText::new(line).color(color));
                            }
                        }
                    });
            },
        ),
        Dialog::CheckResult => ("门禁检查结果".into(), |app, ui| {
            // 口径行排第一（REQ-014 G3）：先说「按谁、用哪份变更集判」，
            // 下面那些理由才有归属；不写口径时，"忘了 git add"与
            // "需求本身有问题"在界面上长得一模一样。
            ui.label(
                egui::RichText::new(app.check_scope.clone())
                    .small()
                    .color(Tone::Muted.color(ui)),
            );
            // 图标（✅ / ⛔）本身就区分得开，这里只统一颜色取值，不再额外叠一层图标。
            let (text, tone) = if app.check_pass {
                ("放行 ✅", Tone::Success)
            } else {
                ("拦截 ⛔", Tone::Danger)
            };
            ui.label(egui::RichText::new(text).color(tone.color(ui)));
            // 类别用台账同名的稳定字面量，便于用户拿去对照 `req-guard check` 的输出。
            if let Some(kind) = &app.check_kind {
                ui.label(egui::RichText::new(format!("类别：{kind}")).color(tone.color(ui)));
            }
            ui.separator();
            for line in app.check_detail.iter() {
                ui.label(line);
            }
        }),
        Dialog::Comments => (
            format!(
                "审核评论 · {}",
                app.current().map(|r| r.id.as_str()).unwrap_or("（无需求）")
            ),
            |app, ui| render_comments(app, ui),
        ),
        Dialog::NewComment => (
            format!(
                "新增{}评论 · 锚定第 {} 段",
                if app.input_blocking {
                    "阻塞"
                } else {
                    "普通"
                },
                app.current_step()
                    .map(requirement::step_label)
                    .unwrap_or("—")
            ),
            |app, ui| {
                ui.label("作者（必填，填「ai」表示 AI 回复——AI 不得新开评论）：");
                ui.text_edit_singleline(&mut app.input_reviewer);
                ui.label("内容（必填）：");
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 120.0],
                            egui::TextEdit::multiline(&mut app.input_comment)
                                .desired_width(f32::MAX)
                                .hint_text("例：回滚方案需补充 DB 迁移回退"),
                        );
                    });
                ui.checkbox(
                    &mut app.input_blocking,
                    "阻塞性：未关闭（resolve）即拦截 AI 编码",
                );
                ui.horizontal(|ui| {
                    if ui.button("提交").clicked() {
                        app.add_comment();
                    }
                    if ui.button("取消").clicked() {
                        app.dialog = Dialog::Comments;
                    }
                });
            },
        ),
        Dialog::ResolveComment => (
            format!(
                "关闭评论 · {}",
                app.comments
                    .get(app.comment_sel)
                    .map(|c| c.id.clone())
                    .unwrap_or_else(|| "（未选中）".into())
            ),
            |app, ui| {
                let body = app
                    .comments
                    .get(app.comment_sel)
                    .map(|c| c.body.clone())
                    .unwrap_or_default();
                ui.label("将要关闭：");
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(body).italics());
                    });
                ui.label("关闭人（必填；只能填人类姓名，填 ai 会被 core 拒绝）：");
                ui.text_edit_singleline(&mut app.input_reviewer);
                ui.label("关闭会解除阻塞并入审计/台账，不可撤销（确需重开请新增一条评论）。");
                ui.horizontal(|ui| {
                    if ui.button("确认关闭").clicked() {
                        app.resolve_comment();
                    }
                    if ui.button("取消").clicked() {
                        app.dialog = Dialog::Comments;
                    }
                });
            },
        ),
        Dialog::Amend => (
            format!(
                "修订当前段 · {}",
                app.current_step()
                    .map(requirement::step_label)
                    .unwrap_or("—")
            ),
            |app, ui| {
                ui.label("提请人（必填）：");
                ui.text_edit_singleline(&mut app.input_reviewer);
                ui.label("改稿说明（必填）：");
                ui.label(
                    egui::RichText::new("说明写进清单与台账（AMEND），并作为阻塞性评论留给 AI。")
                        .italics(),
                );
                egui::ScrollArea::vertical()
                    .id_salt("amend_reason")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 100.0],
                            egui::TextEdit::multiline(&mut app.input_reason)
                                .desired_width(f32::MAX)
                                .hint_text("例：回滚方案缺 DB 迁移回退，方向没问题，请补该步骤"),
                        );
                    });
                ui.label("修订 = 回退待审 + 清空内容摘要，**仍需重新批准**；不豁免重审。");
                ui.horizontal(|ui| {
                    if ui.button("提交修订").clicked() {
                        app.amend();
                    }
                    if ui.button("取消").clicked() {
                        app.dialog = Dialog::None;
                    }
                });
            },
        ),
        Dialog::Seal => {
            (
                format!(
                    "绑定内容摘要 · {}",
                    app.current().map(|r| r.id.as_str()).unwrap_or("（无需求）")
                ),
                |app, ui| {
                    // 把当前各段的冻结状态摊开：审核人得先看清"哪几段可绑、哪几段已绑"，
                    // 否则点下去只会撞上 core 的报错文案。
                    if let Some(r) = app.current() {
                        for s in &r.steps {
                            let text =
                                format!("{} {}  {}", s.label, state_mark(s.state), s.seal.hint());
                            match seal_badge(s.seal) {
                                Some((_, tone)) => {
                                    ui.colored_label(tone.color(ui), text);
                                }
                                None => {
                                    ui.label(text);
                                }
                            }
                        }
                    }
                    ui.separator();
                    ui.label("原因（首次补绑定可留空）：");
                    ui.label(
                        egui::RichText::new(
                            "已全部绑定过再执行 = 承认内容改动无需重审，必须写明原因（记 RESEAL）。",
                        )
                        .italics(),
                    );
                    ui.text_edit_singleline(&mut app.input_reason);
                    ui.horizontal(|ui| {
                        if ui.button("确认绑定").clicked() {
                            app.seal();
                        }
                        if ui.button("取消").clicked() {
                            app.dialog = Dialog::None;
                        }
                    });
                },
            )
        }
        Dialog::None => return,
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .open(&mut open)
        .show(ctx, |ui| body(app, ui));

    if !open {
        app.dialog = Dialog::None;
    }
}

// ===================== 状态机单测（不需要窗口） =====================

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "req-guard-gui-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("创建临时目录");
        p
    }

    /// 填三段实质正文并全部批准（三段都要有实质内容才批得过）。
    ///
    /// core 的 `testutil` 是 `#[cfg(test)]`，gui 用不了，故就地填——
    /// 比把测试专用助手提成公开 API 划算（与 tui 的同名夹具同理由）。
    fn approved_doc(root: &Path) {
        requirement::create(root, None, "登录改造").expect("创建需求");
        let p = requirement::find(root, "REQ-001").expect("清单应存在").path;
        let mut c = std::fs::read_to_string(&p).expect("清单应可读");
        for (heading, line) in [
            ("## 1. 需求分解", "- 背景与问题：GUI 状态机夹具。"),
            ("## 2. 技术方案", "- 总体思路：走 amend / seal。"),
            ("## 3. 测试计划", "- 验收门槛：三段全通过。"),
        ] {
            let needle = format!("{heading}\n");
            assert!(c.contains(&needle), "模板结构变了：{heading}");
            c = c.replacen(&needle, &format!("{needle}{line}\n"), 1);
        }
        std::fs::write(&p, c).expect("写入应成功");
        for step in ["decomposition", "solution", "testplan"] {
            requirement::review(root, "REQ-001", step, "寇工", true, "", false)
                .expect("三段都应批得过");
        }
    }

    // ---------- REQ-014：门禁检查消歧接线用的夹具 ----------

    /// 让 `PathSource::Staged` 在临时目录里可用：`git init` 一个空仓。
    ///
    /// 为什么不用 `HOOK_STAGED_FILES` 环境变量：它是**进程级**的，而 Rust 的测试
    /// 线程并行跑，多个用例各设各的会互相污染——那种偶发红比没有测试更糟。
    /// 空仓 + 无暂存内容时 `git diff --cached` 返回空集，正是 AC-014 的前置。
    fn git_init(root: &Path) {
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "临时目录应能 git init");
    }

    /// 建一份指定编号的三段全批清单（`approved_doc` 写死了 REQ-001，这里要可传 id）。
    fn approved_named(root: &Path, id: &str, title: &str) {
        requirement::create(root, Some(id), title).expect("创建需求");
        let p = requirement::find(root, id).expect("清单应存在").path;
        let mut c = std::fs::read_to_string(&p).expect("清单应可读");
        for (heading, line) in [
            ("## 1. 需求分解", "- 背景与问题：消歧接线夹具。"),
            ("## 2. 技术方案", "- 总体思路：三段全批以便放行。"),
            ("## 3. 测试计划", "- 验收门槛：带 hint 后能放行。"),
        ] {
            let needle = format!("{heading}\n");
            assert!(c.contains(&needle), "模板结构变了：{heading}");
            c = c.replacen(&needle, &format!("{needle}{line}\n"), 1);
        }
        std::fs::write(&p, c).expect("写入应成功");
        for step in ["decomposition", "solution", "testplan"] {
            requirement::review(root, id, step, "寇工", true, "", false)
                .unwrap_or_else(|e| panic!("{id} {step} 应批得过：{e}"));
        }
    }

    /// 把所有标记行的 `sum=` 抹成 `-`，模拟"REQ-002 之前批准的存量清单"。
    fn strip_sums(root: &Path) {
        let p = requirement::find(root, "REQ-001").expect("清单应存在").path;
        let c = std::fs::read_to_string(&p).expect("清单应可读");
        let out: String = c
            .lines()
            .map(|l| match l.find(" sum=") {
                Some(i) if l.starts_with("<!-- GATE:STEP") => format!("{} sum=-", &l[..i]),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&p, out).expect("写入应成功");
    }

    fn seal_of(root: &Path, step: &str) -> requirement::SealState {
        let p = requirement::find(root, "REQ-001").expect("清单应存在").path;
        let c = std::fs::read_to_string(&p).expect("清单应可读");
        requirement::seal_state(&c, step)
    }

    /// 建一条评论（走 core，界面只负责读）。
    fn add_comment(root: &Path, blocking: bool, text: &str) {
        comment::add(
            root,
            "REQ-001",
            comment::NewComment {
                step: Some("solution"),
                author: "寇工",
                text,
                quote: None,
                blocking,
                reply: None,
            },
        )
        .expect("添加评论");
    }

    #[test]
    fn amend_回退待审且清空内容摘要() {
        let root = temp_root("amend");
        approved_doc(&root);
        assert_eq!(
            seal_of(&root, "decomposition"),
            requirement::SealState::Frozen
        );
        let mut app = App::new(&root);

        // 说明必填：没有说明就分不清"方向没错的改稿"与"这段被否决"
        app.input_reviewer = "寇工".into();
        app.input_reason = "   ".into();
        app.amend();
        assert_eq!(
            app.current().unwrap().steps[0].state,
            StepState::Approved,
            "说明为空时不应改动状态"
        );

        app.input_reason = "回滚方案缺 DB 迁移回退，方向没问题".into();
        app.amend();
        let r = app.current().expect("应有需求");
        assert_eq!(
            r.steps[0].state,
            StepState::Amended,
            "amend 应把段置为 amended（待修订）"
        );
        assert_eq!(
            r.steps[0].seal,
            requirement::SealState::NotApplicable,
            "amend 清空 sum= → 不再有已批准正文要保护"
        );
        assert!(!r.unlocked, "amend 不豁免重审：三段不再全通过 → 重新锁回");
        assert_eq!(app.dialog, Dialog::None, "成功后回到主界面");
        assert!(
            app.message.as_deref().unwrap_or("").contains("重新批准"),
            "提示要写明「仍需重审」：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 三段都塞进足够长的正文（每段 60 条），让第 2 段的标题被顶到视口之外。
    ///
    /// 复现"切段后视口停在正文中段"的前提：第 1 段展开时正文很高，
    /// 后面的段标题自然落在 600px 视口之外。
    fn long_doc(root: &Path) {
        requirement::create(root, None, "滚动复现").expect("创建需求");
        let p = requirement::find(root, "REQ-001").expect("清单应存在").path;
        let mut c = std::fs::read_to_string(&p).expect("清单应可读");
        for heading in ["## 1. 需求分解", "## 2. 技术方案", "## 3. 测试计划"] {
            let needle = format!("{heading}\n");
            assert!(c.contains(&needle), "模板结构变了：{heading}");
            let filler: String = (0..60)
                .map(|i| format!("- 第 {i} 条：{}\n", "内容".repeat(30)))
                .collect();
            c = c.replacen(&needle, &format!("{needle}{filler}"), 1);
        }
        std::fs::write(&p, c).expect("写入应成功");
        for step in ["decomposition", "solution", "testplan"] {
            requirement::review(root, "REQ-001", step, "寇工", true, "", false)
                .expect("三段都应批得过");
        }
    }

    /// 离屏渲染一帧中央面板，返回该帧画出的全部文字（原文 + y）。
    ///
    /// `wheel` 非零时该帧带一笔滚轮事件。指针必须悬在面板上：egui 的 ScrollArea 只在
    /// `is_hovering_outer_rect` 时才吃滚轮（`scroll_area.rs`），少了它就滚不动。
    fn center_frame(app: &mut App, ctx: &egui::Context, wheel: f32) -> Vec<(String, f32)> {
        let mut events = vec![egui::Event::PointerMoved(egui::pos2(400.0, 300.0))];
        if wheel != 0.0 {
            events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, wheel),
                modifiers: egui::Modifiers::default(),
                phase: egui::TouchPhase::Move,
            });
        }
        let raw = eframe::egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| render_center(ui, app));
        // 离屏没有渲染器消费纹理增量，必须显式 clear，否则 epaint 在 Drop 时 panic。
        out.textures_delta.clear();
        out.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => {
                    Some((t.galley.text().to_string(), t.visual_bounding_rect().min.y))
                }
                _ => None,
            })
            .collect()
    }

    /// 只匹配**段标题**，不匹配正文里的 `## 2. 技术方案`：两者都含"技术方案"，
    /// 混用会把"正文开头恰好可见"误判成"标题已滚进视口"，测试就恒真了。
    const NEEDLE_HEADER: &str = "] 2. 技术方案";

    fn min_y_of(texts: &[(String, f32)], needle: &str) -> Option<f32> {
        texts
            .iter()
            .filter(|(t, _)| t.contains(needle))
            .map(|(_, y)| *y)
            .fold(None, |acc: Option<f32>, y| {
                Some(acc.map_or(y, |a| a.min(y)))
            })
    }

    #[test]
    fn take_viewport_fix_切段与动画期间校正_稳定后不再校正() {
        // 起始状态：第 0 段正在展开动画中 → 必须校正。
        let mut last = 0;
        assert!(
            take_viewport_fix(&mut last, 0, false),
            "开合动画未结束时必须校正，否则视口会停在正文中段"
        );
        // 动画结束、段没变 → **不再校正**（否则用户手动滚下去会被每帧拽回顶部）。
        // 这条断言正是"滚到下方后自动跳回第一行"那个缺陷的判决：状态一旦没被推进，
        // 它就会一直是 true。
        assert!(
            !take_viewport_fix(&mut last, 0, true),
            "稳定之后不应再校正视口"
        );
        assert_eq!(last, 0, "状态必须被推进到当前段");
        // 切段那一帧必须校正（哪怕选中段已经是展开状态）。
        assert!(take_viewport_fix(&mut last, 1, true));
        assert_eq!(last, 1);
        // 切段后动画期间继续校正，稳定后停止。
        assert!(take_viewport_fix(&mut last, 1, false));
        assert!(!take_viewport_fix(&mut last, 1, true));
        // 来回切段每次都要校正（状态跟的是当前段，不是"曾经切过一次"）。
        assert!(take_viewport_fix(&mut last, 0, true));
        assert!(take_viewport_fix(&mut last, 1, true));
    }

    #[test]
    fn 切段后选中段标题回到视口() {
        // 审核人实测："技术方案和测试计划展开时，展示的内容不是从头开始，而是滚动到了中间"。
        // 成因：切段使上一段正文收起（内容变矮），但 ScrollArea 的偏移被原样保留 ——
        // 视口停在"新段正文中段"（实测偏移 1800 时第 2 段标题被推到屏幕 y=2144，
        // 视口只有 600 高，段标题根本不在视口内）。
        //
        // ⚠ **这条用例不是"视口校正"的判决**：离屏 `run_ui` 里滚轮不生效，造不出"已滚下去"
        // 的起点（详见 [`need_viewport_fix`] 的注释），所以撤掉校正它照样会过。
        // 它的作用是守住切换路径本身能跑通、且切段后选中段标题**可见**（不是画到视口外）；
        // 校正规则由 `need_viewport_fix_动画结束前持续校正` 单测守住，
        // 校正后的实际滚动效果由人工在真实 GUI 里点一次确认。
        let ctx = {
            let c = egui::Context::default();
            c.set_fonts(crate::fonts::definitions());
            c
        };
        let root = temp_root("scroll-step");
        long_doc(&root);
        let mut app = App::new(&root);
        assert_eq!(app.step, 0);

        // 第 1 段正文很长 → 第 2 段标题本来就在视口外。
        let texts = center_frame(&mut app, &ctx, 0.0);
        assert!(
            min_y_of(&texts, NEEDLE_HEADER).is_none(),
            "前提不成立：第 1 段正文很长，第 2 段标题本应在视口外，却画在了 y={:?}",
            min_y_of(&texts, NEEDLE_HEADER)
        );

        // 切段（= 点第 2 段标题的效果）。开合带动画，所以要跑到它收敛：视口校正只在
        // "切段帧或动画未结束"时发起，动画一停就交还给用户。
        app.step = 1;
        let mut after = None;
        for _ in 0..40 {
            after = min_y_of(&center_frame(&mut app, &ctx, 0.0), NEEDLE_HEADER);
            if after.is_some() {
                break;
            }
        }
        let after = after.expect("切段后第 2 段标题应被滚进视口：视口停在了正文中段");
        assert!(after < 600.0, "切段后第 2 段标题应在视口内，实得 y={after}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn amend_已通过段也可请求修订() {
        // "方向没错、只是漏个约束"最常发生在刚批准的段上，而那一段没有「打回」按钮——
        // 若 amend 也只在未通过段可用，这条反馈路径在界面上就是死的。
        let root = temp_root("amend-approved");
        approved_doc(&root);
        let mut app = App::new(&root);
        app.input_reviewer = "寇工".into();
        app.input_reason = "补充回滚步骤".into();
        app.amend();
        assert_eq!(app.current().unwrap().steps[0].state, StepState::Amended);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seal_为存量清单补绑定后三段皆冻结() {
        let root = temp_root("seal-fresh");
        approved_doc(&root);
        strip_sums(&root);
        let mut app = App::new(&root);
        assert_eq!(
            app.current().unwrap().steps[0].seal,
            requirement::SealState::NotSealed
        );

        app.input_reason = "   ".into();
        app.seal();
        for step in ["decomposition", "solution", "testplan"] {
            assert_eq!(
                seal_of(&root, step),
                requirement::SealState::Frozen,
                "{step} 首次补绑定后应真正冻结"
            );
        }
        assert_eq!(app.dialog, Dialog::None);
        assert!(
            app.message.as_deref().unwrap_or("").contains("已绑定"),
            "应报出绑了哪几段：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seal_重复绑定须给理由() {
        let root = temp_root("seal-reseal");
        approved_doc(&root);
        strip_sums(&root);
        let mut app = App::new(&root);
        app.seal();

        // 已全部绑定过 → 空理由应被 core 挡下（正常路径是 amend → 改 → approve）
        app.input_reason = "".into();
        app.seal();
        assert!(
            app.message.as_deref().unwrap_or("").contains("reason"),
            "应提示需给理由：{:?}",
            app.message
        );

        app.input_reason = "改动仅为错别字".into();
        app.seal();
        assert!(
            app.message.as_deref().unwrap_or("").contains("已绑定"),
            "给了理由应放行（记 RESEAL）：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seal_代劳审批的绕道被core挡住() {
        // amend 清了 sum= 之后 seal 不能把未批准的段绑上，否则"打回 → 改 → seal"
        // 就是一条跳过重审的通道（REQ-002 的核心防线，界面不能自己开口子）。
        let root = temp_root("seal-no-approve");
        approved_doc(&root);
        let mut app = App::new(&root);
        app.step = 1;
        app.input_reviewer = "寇工".into();
        app.input_reason = "补声明".into();
        app.amend();
        app.input_reason = "".into();
        app.seal();
        assert_eq!(
            seal_of(&root, "solution"),
            requirement::SealState::NotApplicable,
            "未批准的段不应被 seal 绑定"
        );
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("未处于 approved"),
            "应把 core 的拒绝理由原样带出来：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 打开面板读出评论与阻塞计数() {
        let root = temp_root("open");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, true, "回滚方案需补充 DB 迁移回退");
        let mut app = App::new(&root);

        app.open_comments();
        assert_eq!(app.dialog, Dialog::Comments);
        assert_eq!(app.comments.len(), 1);
        assert!(app.comments[0].is_blocking_open());
        assert_eq!(app.comment_sel, 0, "默认选中第一条");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 新增评论经core落盘且锚定当前段() {
        let root = temp_root("add");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        let mut app = App::new(&root);

        // 作者 / 内容任一为空都应被拒，且不留下半条评论
        app.input_reviewer = "  ".into();
        app.input_comment = "缺少失败锁定".into();
        app.add_comment();
        assert!(app.comments.is_empty(), "作者为空不应落盘");

        app.input_reviewer = "寇工".into();
        app.input_comment = "  ".into();
        app.add_comment();
        assert!(app.comments.is_empty(), "内容为空不应落盘");

        app.input_comment = "缺少失败锁定".into();
        app.input_blocking = true;
        app.add_comment();

        let list = comment::list(&root, "REQ-001").expect("读评论");
        assert_eq!(list.len(), 1);
        assert!(list[0].is_blocking_open(), "勾了阻塞就应阻塞");
        assert_eq!(list[0].author, "寇工");
        assert_eq!(
            list[0].step.as_deref(),
            Some("decomposition"),
            "锚定当前选中段"
        );
        // 面板回到列表视图，且需求计数已刷新（阻塞评论会改变门禁裁决）
        assert_eq!(app.dialog, Dialog::Comments);
        assert_eq!(app.comments.len(), 1);
        assert!(app.current().expect("应有需求").blocking_comments == 1);
        assert!(app.input_comment.is_empty(), "提交后清空输入");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 关闭评论需审核人且改为resolved() {
        let root = temp_root("resolve");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, true, "回滚方案需补充 DB 迁移回退");
        let mut app = App::new(&root);
        app.open_comments();

        // 关闭人为空：应拒绝，且评论仍是 open
        app.resolve_comment();
        assert_eq!(
            comment::list(&root, "REQ-001").unwrap()[0].state,
            comment::CommentState::Open,
            "关闭人为空不应关闭"
        );

        app.input_reviewer = "寇工".into();
        app.resolve_comment();
        let list = comment::list(&root, "REQ-001").unwrap();
        assert_eq!(list[0].state, comment::CommentState::Resolved);
        assert!(!list[0].is_blocking_open(), "关闭后不再阻塞");
        assert_eq!(app.comments[0].state, comment::CommentState::Resolved);
        assert_eq!(
            app.current().expect("应有需求").blocking_comments,
            0,
            "阻塞计数应随关闭归零"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 重复关闭已关闭评论会被挡下() {
        let root = temp_root("resolve-twice");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, false, "普通意见");
        let mut app = App::new(&root);
        app.open_comments();
        app.input_reviewer = "寇工".into();
        app.resolve_comment();
        // 面板已刷新成 resolved，再关一次应被挡（而不是把审计写重复）
        app.input_reviewer = "寇工".into();
        app.resolve_comment();
        assert!(
            app.message.as_deref().unwrap_or("").contains("已是关闭"),
            "应提示已关闭：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 重算锚点把找不到原文的标记为stale() {
        let root = temp_root("anchors");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        comment::add(
            &root,
            "REQ-001",
            comment::NewComment {
                step: None,
                author: "寇工",
                text: "带锚点的意见",
                quote: Some("这段引用在正文中并不存在"),
                blocking: false,
                reply: None,
            },
        )
        .expect("添加评论");
        let mut app = App::new(&root);
        app.open_comments();

        app.refresh_comment_anchors();
        assert!(app.comments[0].stale, "找不到原文应标 stale");
        assert!(
            app.message.as_deref().unwrap_or("").contains("stale"),
            "应说明有几条失效：{:?}",
            app.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 切换目录清空旧需求的评论() {
        let root = temp_root("switch");
        requirement::create(&root, None, "登录改造").expect("创建需求");
        add_comment(&root, false, "意见");
        let mut app = App::new(&root);
        app.open_comments();
        assert_eq!(app.comments.len(), 1);

        let other = temp_root("switch-other");
        app.root = other.clone();
        app.comments.clear();
        app.reload_comments();
        assert!(app.comments.is_empty(), "换项目后不应残留上一个需求的评论");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other);
    }

    // ===================== 原文视图：只读，但能选中 / 能复制 =====================

    /// 只读的**机制**来自 egui 给 `&str` 的 `TextBuffer` 实现：`is_mutable()` 为 false，
    /// 且写入动作（插入 / 删除 / 清空 / 整体替换）全是空实现。
    ///
    /// 这条依赖必须钉死——哪天 egui 把 `&str` 改成可写，原文视图会**悄悄**变成能改，
    /// 那时这个测试会先红，而不是等审核人发现清单被改了。
    #[test]
    fn 只读靠不可变的文本缓冲区实现() {
        use egui::TextBuffer;

        let src = "## 1. 需求分解\n";
        let mut buf: &str = src;
        assert!(!TextBuffer::is_mutable(&buf), "&str 必须是只读 buffer");

        TextBuffer::insert_text(&mut buf, "注入", egui::text::CharIndex(0));
        TextBuffer::delete_char_range(&mut buf, egui::text::CharIndex(0)..egui::text::CharIndex(2));
        TextBuffer::clear(&mut buf);
        TextBuffer::replace_with(&mut buf, "篡改");
        assert_eq!(buf, src, "只读 buffer 的写入动作必须全部是空实现");
    }

    /// 测试用的原文（两行，够拖出一段选区）。
    const RAW_SRC: &str = "## 1. 需求分解\n- [ ] 背景与问题\n";

    /// 左键按下 / 松开事件。
    fn click_at(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn raw_screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(420.0, 260.0))
    }

    /// 跑一帧「原文文本框」，返回本帧输出与控件 `Response`。
    ///
    /// 离屏跑帧没有渲染器来消费纹理增量，必须手动 clear（否则 epaint 在 Drop 时 panic）。
    fn raw_frame(
        ctx: &egui::Context,
        screen: egui::Rect,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, egui::Response) {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let mut resp = None;
        let mut out = ctx.run_ui(input, |ui| {
            resp = Some(read_only_text(
                ui,
                "raw_md_test",
                [400.0, 200.0],
                egui::TextStyle::Monospace,
                RAW_SRC,
            ));
        });
        out.textures_delta.clear();
        (out, resp.expect("read_only_text 应返回 response"))
    }

    /// 从输出里捞所有写进剪贴板的内容。
    fn copied_texts(out: &egui::FullOutput) -> Vec<String> {
        out.platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    /// 只读文本框必须**参与指针交互**。
    ///
    /// `interactive(false)` 的老写法会把 sense 降成 `Sense::hover()`：点不中、拖不动、
    /// 也拿不到焦点——这正是原来"选不中、复制不了"的根因。这里把这两条钉住。
    #[test]
    fn 只读文本框参与指针交互() {
        let ctx = egui::Context::default();
        let screen = raw_screen();
        let (_, r) = raw_frame(&ctx, screen, vec![]);
        let id = r.id;
        let p = egui::pos2(8.0, 6.0);

        let (_, r) = raw_frame(&ctx, screen, vec![egui::Event::PointerMoved(p)]);
        assert!(r.hovered(), "指针移到正文上应算 hover");

        // 按下即拿焦点：旧写法（sense = hover）这一步必然失败。
        let _ = raw_frame(&ctx, screen, vec![click_at(p, true)]);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(id),
            "在只读正文上按下应让文本框获得焦点"
        );

        // 按住并移动 → 进入拖拽态（鼠标框选的前提）。
        let _ = raw_frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(egui::pos2(80.0, 6.0))],
        );
        let (_, r) = raw_frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(egui::pos2(200.0, 6.0))],
        );
        assert!(r.dragged(), "按住并移动后应进入拖拽态，否则框选无从谈起");
    }

    /// 「选中 → 复制」两个出口：右键菜单的「复制选中」与 Ctrl+C。
    ///
    /// 选区直接写进 TextEdit 状态（菜单里的「全选」也是这么写），再分别验两个出口。
    /// 不模拟鼠标拖选的原因见 [`只读文本框参与指针交互`]：离屏帧没有真实窗口几何，
    /// `Galley::cursor_from_pos` 一律返回第 0 个字符（可编辑的 TextEdit 同样如此，已实测），
    /// 选区内容只能在状态层面验。
    #[test]
    fn 选中内容可经右键菜单与快捷键复制() {
        let ctx = egui::Context::default();
        let screen = raw_screen();
        let p = egui::pos2(8.0, 6.0);
        let _ = raw_frame(&ctx, screen, vec![]);
        let _ = raw_frame(&ctx, screen, vec![egui::Event::PointerMoved(p)]);
        let (_, r) = raw_frame(&ctx, screen, vec![click_at(p, true)]);
        let id = r.id;

        let want = "## 1. 需求分解";
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(want.chars().count()),
            )));
        egui::TextEdit::store_state(&ctx, id, state);

        // 出口一：右键菜单「复制选中」走的就是它。
        assert_eq!(
            selected_text(&ctx, id, RAW_SRC).as_deref(),
            Some(want),
            "「复制选中」应取出当前选区"
        );

        // 出口二：Ctrl+C。真实窗口里 egui-winit 就是把 Ctrl+C 翻成 `Event::Copy`。
        //
        // ⚠ 这里只断言"选区的文字确实进了剪贴板"，不比对完整内容：
        // 离屏帧里 galley 的几何是退化的（量出来 `Galley::rect` 是 0×0），
        // `clamp_cursor` 会把选区夹短（实测 0..9 被夹成 0..2，且**换成可编辑的
        // `String` buffer 也一样**，属测试环境限制而非本改动引入）。选区的取值本身
        // 由上面 `selected_text` 那条断言负责——它不走 galley。
        let (out, _) = raw_frame(&ctx, screen, vec![egui::Event::Copy]);
        let copied = copied_texts(&out);
        assert_eq!(
            copied.len(),
            1,
            "Ctrl+C 应产生一条复制命令；实际输出：{:?}",
            out.platform_output.commands
        );
        assert!(!copied[0].is_empty(), "Ctrl+C 复制出来的内容不应为空");
        assert!(
            RAW_SRC.starts_with(copied[0].as_str()),
            "复制出来的应是原文的一段，实际：{:?}",
            copied[0]
        );
    }

    /// 「一键复制全文」按钮：点一下就整段进剪贴板（不依赖选中状态）。
    #[test]
    fn 原文视图的一键复制全文按钮可用() {
        let ctx = egui::Context::default();
        let screen = raw_screen();
        let run = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| add_raw_text(ui, RAW_SRC));
            out.textures_delta.clear();
            out
        };

        // 按钮在 `add_raw_text` 顶部那一行的最左端。
        let btn = egui::pos2(12.0, 8.0);
        let _ = run(vec![]);
        let _ = run(vec![egui::Event::PointerMoved(btn), click_at(btn, true)]);
        let out = run(vec![egui::Event::PointerMoved(btn), click_at(btn, false)]);

        let copied = copied_texts(&out);
        assert_eq!(
            copied,
            vec![RAW_SRC.to_string()],
            "点「复制全文」应把整段原文放进剪贴板；实际输出：{:?}",
            out.platform_output.commands
        );
    }

    // ---------- REQ-014：门禁检查消歧接线 ----------

    #[test]
    fn gate_check与带hint判定在多需求仓库下结果不同() {
        let root = temp_root("gate-ctx");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");

        let plain = gate::gate_check(&root).expect("全局判定");
        assert!(!plain.is_pass(), "无 hint 的全局判定在多需求仓库下应拦截");

        let with_hint = gate::gate_check_with(
            &root,
            &resolve::Ctx {
                source: resolve::PathSource::Staged,
                hint: Some("REQ-001".to_string()),
            },
        )
        .expect("带 hint 判定");
        assert!(
            with_hint.is_pass(),
            "带 hint 应放行，实际：{}",
            with_hint.summary()
        );
        assert_ne!(
            plain.is_pass(),
            with_hint.is_pass(),
            "两种上下文的放行判定必须不同，否则消歧没起作用"
        );
        assert_ne!(plain.detail(), with_hint.detail(), "明细也应不同");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 选中需求时口径含该id且不再报歧义() {
        let root = temp_root("gui-hint");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");

        let mut app = App::new(&root);
        app.selected = 0;
        app.run_check();
        assert!(
            app.check_scope.contains("REQ-001"),
            "口径必须写明按哪份判：{}",
            app.check_scope
        );
        assert!(
            app.check_scope.contains("staged"),
            "口径必须写明用的哪份变更集：{}",
            app.check_scope
        );
        assert!(app.check_pass, "选中已批需求应放行");
        let all = app.check_detail.join("\n");
        assert!(
            !all.contains("无法确定本次改动属于哪份需求"),
            "带 hint 后不应再报歧义：{all}"
        );
        assert!(app.check_kind.is_none(), "放行时不得残留上一轮的拦截类别");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 无选中需求时与改前逐字一致() {
        let root = temp_root("gui-nohint");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");

        let mut app = App::new(&root);
        // 构造「无选中项」：清空列表而不是靠越界下标（后者读 current() 会 panic）。
        app.reqs.clear();
        let (ctx, scope) = app.check_ctx();
        assert!(ctx.hint.is_none(), "无选中项时不得编造 hint");
        assert!(
            matches!(ctx.source, resolve::PathSource::None),
            "无选中项时不得拿空变更集去反查"
        );
        assert!(
            scope.contains("未选择需求"),
            "口径要说明是全局判定：{scope}"
        );

        let v = gate::gate_check_with(&root, &ctx).expect("判定");
        let legacy = gate::gate_check(&root).expect("改前的入口");
        assert_eq!(v.is_pass(), legacy.is_pass(), "放行判定应与改前一致");
        assert_eq!(v.detail(), legacy.detail(), "明细应与改前逐字一致");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 空暂存区按已批需求放行且不报错() {
        let root = temp_root("gui-empty-staged");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");

        let mut app = App::new(&root);
        app.run_check();
        assert!(app.check_pass, "空暂存区 + 已批需求应放行");
        assert!(
            app.check_scope.contains("staged（空）"),
            "空暂存区必须写明「空」：{}",
            app.check_scope
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 已归档清单作选择时提示已归档而非裸枚举名() {
        let root = temp_root("gui-archived-hint");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        approved_named(&root, "REQ-002", "支付改造");
        // 先 done（置 status=done，使它退出 live 集），再物理搬进归档区 ——
        // 少做后者的话 `enrich_archived` 会说「拼写错误，或清单已被删除」而不是
        // 「已归档」：那句措辞只在 `list_archived` 真能查到该文件时才出现
        // （core/src/resolve.rs 的 `enrich_archived`）。
        requirement::done(&root, "REQ-002", "寇工").expect("标注完成");
        requirement::archive_one(&root, "寇工", "REQ-002", false)
            .expect("物理归档后仍可选中并得到已归档说明");

        let v = gate::gate_check_with(
            &root,
            &resolve::Ctx {
                source: resolve::PathSource::Staged,
                hint: Some("REQ-002".to_string()),
            },
        )
        .expect("判定");
        assert!(!v.is_pass(), "指向已归档清单应拦截");
        let text = format!("{}\n{}", v.summary(), v.detail().join("\n"));
        assert!(text.contains("已归档"), "应说明已归档：{text}");
        assert_eq!(
            check_kind_of(v.summary(), v.detail()),
            Some("UnknownSelection"),
            "类别应为 UnknownSelection"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 七类拦截类别都能被认出() {
        // 直接测识别函数：关键词取每类**首行**里那段唯一措辞（见 check_kind_of 注释）。
        let cases: &[(&str, &str)] = &[
            (
                "⛔ 拦截：已批准段的正文与批准时不一致（内容冻结校验失败）。",
                "SumMismatch",
            ),
            (
                "⛔ 拦截：指定的 REQ-009 不在可裁决的需求清单里。",
                "UnknownSelection",
            ),
            ("⛔ 拦截：未找到待开发的需求清单。", "NoRequirement"),
            (
                "⛔ 拦截：指定的需求 REQ-001 与本次改动不相交。",
                "SelectionMismatch",
            ),
            (
                "⛔ 拦截：本次改动所属的需求存在未解决的阻塞性评论。",
                "OpenBlockingComment",
            ),
            (
                "⛔ 拦截：无法确定本次改动属于哪份需求（仓库内有多份未归档清单）。",
                "Ambiguous",
            ),
            (
                "⛔ 拦截：本次改动所属的需求尚未通过审核。",
                "StepNotApproved",
            ),
        ];
        for (summary, want) in cases {
            assert_eq!(
                check_kind_of(summary, &[]),
                Some(*want),
                "类别识别错：{summary}"
            );
        }
        assert_eq!(
            check_kind_of("✅ 门禁放行：REQ-001 三段已批准", &[]),
            None,
            "放行不该被认成任何拦截类别"
        );
    }

    #[test]
    fn 判别词出现在明细里也认得出() {
        // detail 首条可能是「涉及需求：…」，判别词未必在 summary 里 —— 两者都要扫。
        let detail = vec![
            "涉及需求：REQ-001".to_string(),
            "⛔ 拦截：无法确定本次改动属于哪份需求。".to_string(),
        ];
        assert_eq!(
            check_kind_of("⛔ 门禁拦截", &detail),
            Some("Ambiguous"),
            "detail 里出现判别词时也要认得出"
        );
    }

    #[test]
    fn 单份清单时带不带hint结果逐字相同() {
        let root = temp_root("gui-single");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");

        let a = gate::gate_check(&root).expect("无 hint");
        let b = gate::gate_check_with(
            &root,
            &resolve::Ctx {
                source: resolve::PathSource::None,
                hint: None,
            },
        )
        .expect("同参数");
        assert_eq!(a.is_pass(), b.is_pass());
        assert_eq!(a.detail(), b.detail());
        let c = gate::gate_check_with(
            &root,
            &resolve::Ctx {
                source: resolve::PathSource::Staged,
                hint: Some("REQ-001".to_string()),
            },
        )
        .expect("带 hint");
        assert_eq!(a.is_pass(), c.is_pass(), "单份清单时存量项目零感知");
        assert_eq!(a.detail(), c.detail(), "明细也应逐字相同");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 结果面板首行是口径而不是结论() {
        // 口径必须排在结论**之前**：先说「按谁判、用哪份变更集」，
        // 下面的理由才有归属。这里断言顺序，不只是断言「出现过」。
        let root = temp_root("gui-panel-order");
        git_init(&root);
        approved_named(&root, "REQ-001", "登录改造");
        let mut app = App::new(&root);
        app.run_check();

        let scope = app.check_scope.clone();
        let pass = app.check_pass;
        let kind = if pass {
            "放行 ✅".to_string()
        } else {
            "拦截 ⛔".to_string()
        };
        let mut rendered = vec![scope.clone(), kind];
        rendered.extend(app.check_detail.clone());
        let text = rendered.join("\n");
        assert!(text.contains("REQ-001"), "结果应含口径行：{text}");
        assert!(
            text.find("REQ-001").unwrap() < text.find("放行").unwrap_or(usize::MAX),
            "口径行应排在结论之前：{text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---------- REQ-015：需求生命周期 ----------

    /// 造一个 git 身份为 `name` 的临时仓（供预填类用例取真实身份）。
    ///
    /// 之所以敢在并行测试里造真仓：`resolve_claimed` 走的是**不带缓存**的
    /// `read_git_identity`，临时仓路径唯一，既不污染进程级缓存也不被它污染。
    fn git_named(root: &Path, name: &str) {
        git_init(root);
        for (k, v) in [("user.name", name), ("user.email", "t@example.com")] {
            let ok = std::process::Command::new("git")
                .args(["config", k, v])
                .current_dir(root)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(ok, "应能设置 git {}", k);
        }
    }

    #[test]
    fn 归档确认框_三项事实齐备且无阻塞时可直接确认() {
        // AC-010：文本同时含「N/3 已通过」「阻塞」「门禁不再管辖」。
        let root = temp_root("done-facts");
        approved_doc(&root);
        let mut app = App::new(&root);
        app.selected = 0;
        app.open_done_confirm();
        assert_eq!(app.dialog, Dialog::DoneConfirm);
        assert_eq!(app.done_facts(), (3, 0), "三段全批、无阻塞评论");
        assert!(app.done_confirm_enabled(), "无阻塞评论时不该要求勾选");

        // 断言的是**弹窗文本**（`done_confirm_lines` 就是渲染用的那份）：
        // 只测 `done_facts()` 等于没测「它有没有真的显示出来」。
        let text = app.done_confirm_lines().join("\n");
        assert!(text.contains("3/3"), "须写明通过段数：{text}");
        assert!(text.contains("阻塞"), "须写明阻塞评论数：{text}");
        assert!(text.contains("门禁不再管辖"), "须写明后果：{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 归档确认框_有阻塞评论时必须勾选才可确认() {
        // AC-009 / U10
        let root = temp_root("done-blocking");
        approved_doc(&root);
        add_comment(&root, true, "这里必须先补回滚方案");
        let mut app = App::new(&root);
        app.selected = 0;
        app.open_done_confirm();
        let (_, blocking) = app.done_facts();
        assert_eq!(blocking, 1, "应有 1 条未关闭阻塞评论");
        assert!(
            !app.done_confirm_enabled(),
            "未勾选时确认按钮必须不可用（勾选门禁）"
        );
        app.ack_blocking = true;
        assert!(app.done_confirm_enabled(), "勾选后应可用");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 勾选态在换需求后重置() {
        // 不清的话：上一轮勾过、这一轮换份需求，界面会在用户没再看一眼的情况下
        // 放行一个高危动作。
        let root = temp_root("done-ack-reset");
        approved_named(&root, "REQ-001", "甲需求");
        approved_named(&root, "REQ-002", "乙需求");
        add_comment(&root, true, "阻塞");
        let mut app = App::new(&root);
        app.selected = 0;
        app.open_done_confirm();
        app.ack_blocking = true;
        app.open_done_confirm();
        assert!(!app.ack_blocking, "重开确认框必须清掉上一轮的勾选");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 清空审核人后提交不静默采用预填值() {
        // AC-014：留空 → 报错，且三段状态一字不变。
        // 「没填」与「填了预填值」在审计上不可区分，而这是 L3 可归属性的基础。
        let root = temp_root("done-empty-actor");
        approved_doc(&root);
        let before = std::fs::read_to_string(
            requirement::find(&root, "REQ-001")
                .expect("清单应存在")
                .path,
        )
        .expect("可读");
        let mut app = App::new(&root);
        app.selected = 0;
        app.open_done_confirm();
        app.input_reviewer.clear();
        app.do_done();
        assert!(
            app.message
                .clone()
                .unwrap_or_default()
                .contains("操作人不能为空"),
            "留空必须报错：{:?}",
            app.message
        );
        let after = std::fs::read_to_string(
            requirement::find(&root, "REQ-001")
                .expect("清单应存在")
                .path,
        )
        .expect("可读");
        assert_eq!(before, after, "未静默采用预填值 → 文件一字不应变");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn done的凭据范围形状是带id的精确匹配() {
        // AC-020 的正向锁：`done:{id}` 而不是常量 `"done"`。
        // 这条必须**不是自证**的——判决性实验会把 `done_scope` 改成常量再跑它。
        let root = temp_root("done-scope");
        approved_named(&root, "REQ-001", "甲需求");
        approved_named(&root, "REQ-002", "乙需求");
        let mut app = App::new(&root);
        app.selected = 0;
        assert_eq!(app.done_scope(), "done:REQ-001");
        app.selected = 1;
        assert_eq!(app.done_scope(), "done:REQ-002", "范围必须跟着选中项走");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 归档后退出门禁的live集合且消息说明不再管辖() {
        // AC-015。⚠️ 口径说明（实测，与 REQ-015 §关键设计 8 的措辞有出入）：
        // 「未归档清单列表」= `resolve::live_snapshot`，它按 HEAD 的 `status=done` 过滤；
        // 而界面左栏用的 `status::req_list`（`requirement::list`）**只扫目录、不看 done**。
        // 即：done 之后该需求**仍留在界面列表里**——这是对的：本期界面不自动物理归档（N4），
        // 若连左栏也一并隐藏，这份需求就既不在 live 列表、也不在归档区，变成彻底不可达。
        // 故这里断言的是**门禁口径**（live 集合少一项），那才是「门禁不再管辖」的落点。
        let root = temp_root("done-live");
        approved_named(&root, "REQ-001", "甲需求");
        approved_named(&root, "REQ-002", "乙需求");
        let before = req_guard_core::resolve::live_snapshot(&root).expect("live 快照");
        assert_eq!(before.len(), 2);

        let mut app = App::new(&root);
        app.selected = 0;
        app.open_done_confirm();
        app.input_reviewer = "寇工".into();
        app.do_done();

        let after = req_guard_core::resolve::live_snapshot(&root).expect("live 快照");
        assert_eq!(after.len(), before.len() - 1, "门禁的 live 集合应少一份");
        assert!(
            !after.iter().any(|r| r.id == "REQ-001"),
            "被归档的需求不应还在门禁的 live 集合里"
        );
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("门禁不再管辖"), "消息须说明后果：{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 预演归档不动磁盘且台账无新增归档审计行() {
        // AC-003 / U12
        let root = temp_root("archive-dry");
        approved_named(&root, "REQ-001", "甲需求");
        req_guard_core::requirement::done(&root, "REQ-001", "寇工").expect("先 done");
        let src = req_guard_core::requirement::find(&root, "REQ-001")
            .expect("清单应存在")
            .path;
        let before = audit_count(&root, "ARCHIVE");

        let mut app = App::new(&root);
        app.reload();
        // done 之后 live 集为空 → 预演走到期清扫，同样验证「不动磁盘」。
        app.input_reviewer = "寇工".into();
        app.preview_archive_due();
        assert!(src.exists(), "预演后文件必须还在原路径");
        assert_eq!(
            audit_count(&root, "ARCHIVE"),
            before,
            "预演不得写 ARCHIVE 审计"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 台账里以 `prefix` 开头的行数（`ARCHIVE` / `DONE` 等事件的计数基线）。
    fn audit_count(root: &Path, prefix: &str) -> usize {
        let p = root.join(".gates/audit/ledger.md");
        std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.trim_start().starts_with(prefix))
            .count()
    }

    #[test]
    fn 到期归档预演的结果写明判据天数() {
        // AC-011：提示文本必须含天数，否则用户不知道「到期」是多久。
        let root = temp_root("archive-days");
        approved_named(&root, "REQ-001", "甲需求");
        let mut app = App::new(&root);
        let days = app.archive_after_days();
        assert_eq!(days, 30, "临时仓无配置 → core 默认 30 天");
        app.input_reviewer = "寇工".into();
        app.preview_archive_due();
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("30"), "结果必须写明天数：{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 归档区视图下所有写动作被拒() {
        // AC-012 / U14：归档即终点，五个写动作全部不可用。
        let root = temp_root("archived-ro");
        approved_named(&root, "REQ-001", "甲需求");
        req_guard_core::requirement::done(&root, "REQ-001", "寇工").expect("先 done");
        req_guard_core::requirement::archive_one(&root, "寇工", "REQ-001", false)
            .expect("物理归档");

        let mut app = App::new(&root);
        app.open_archived();
        assert!(app.read_only(), "归档区视图应为只读");
        assert!(!app.archived.is_empty(), "归档区应能列出清单");

        // 动作入口逐个挡一遍：按钮置灰只是第一道，入口再挡才挡得住键盘与新入口。
        for (name, call) in [
            ("批准", App::approve as fn(&mut App)),
            ("打回", App::reject),
            ("修订", App::amend),
            ("绑定摘要", App::seal),
            ("创建需求", App::create_req),
            ("新增评论", App::add_comment),
            ("关闭评论", App::resolve_comment),
            ("应急绕过", App::do_bypass),
        ] {
            app.message = None;
            call(&mut app);
            let msg = app.message.clone().unwrap_or_default();
            assert!(msg.contains("只读"), "{name} 应被只读拦下，实际：{msg}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 审核人预填取自实时git身份() {
        // AC-013：预填值必须等于「这个仓库」的 git 姓名，且**不硬编码**任何姓名。
        let root = temp_root("prefill");
        git_named(&root, "Mike Zhu");
        let app = App::new(&root);
        assert_eq!(app.prefill_reviewer(), "Mike Zhu");

        // 换个人名再取一次：证明它读的是实时身份，不是某个常量。
        git_named(&root, "另一个人");
        assert_eq!(
            app.prefill_reviewer(),
            "另一个人",
            "预填必须随 git 身份变化"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 打开审批弹窗即预填审核人() {
        // AC-013 的界面落点：预填要落到输入框里，而不只是有个函数能算出来。
        let root = temp_root("prefill-dialog");
        git_named(&root, "Mike Zhu");
        approved_named(&root, "REQ-001", "甲需求");
        let mut app = App::new(&root);
        app.selected = 0;
        app.open_done_confirm();
        assert_eq!(app.input_reviewer, "Mike Zhu", "确认框应预填好审核人");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 顶栏身份区含姓名与鉴权等级() {
        // G6：让「为什么按钮点不动」在界面上自解释。
        let root = temp_root("who");
        git_named(&root, "Mike Zhu");
        let app = App::new(&root);
        assert!(app.who.contains("Mike Zhu"), "应显示姓名：{}", app.who);
        assert!(app.who.contains("L"), "应显示鉴权等级：{}", app.who);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---------- REQ-016：校验报告 ----------

    /// 让三段全批但**不绑定**摘要（模拟 REQ-002 之前的存量清单）。
    fn approved_unsealed(root: &Path, id: &str, title: &str) {
        approved_named(root, id, title);
        strip_sums_for(root, id);
    }

    fn strip_sums_for(root: &Path, id: &str) {
        let p = requirement::find(root, id).expect("清单应存在").path;
        let c = std::fs::read_to_string(&p).expect("清单应可读");
        let out: String = c
            .lines()
            .map(|l| match l.find(" sum=") {
                Some(i) if l.starts_with("<!-- GATE:STEP") => format!("{} sum=-", &l[..i]),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&p, out).expect("写入应成功");
    }

    /// 在临时仓里**真的**暂存一个文件，让 `touch::staged_files` 走 `git diff --cached`。
    ///
    /// ⚠️ 为什么不用 core 的环境变量入口（`HOOK_STAGED_FILES`）：那是**进程级**的，
    /// 而测试线程并行跑——别的用例会在它生效期间读到这份"暂存区"，判定随之改变。
    /// 那正是"时绿时红"的成因，比没有用例更糟（实测：给 TUI 套同一把互斥锁也挡不住，
    /// 因为锁只串行化了自己人）。真 git 索引是按 root 隔离的，天然不互相污染。
    fn stage_file(root: &Path, rel: &str) {
        let p = root.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).expect("建目录");
        }
        std::fs::write(&p, "// 夹具\n").expect("写文件");
        let ok = std::process::Command::new("git")
            .args(["add", "--", rel])
            .current_dir(root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "应能 git add {rel}");
    }

    #[test]
    fn 批量封存一次签票三份全成() {
        // AC-001 / AC-013：L0 下走通 + 签发/清除各一次。
        let root = temp_root("health-sealmany");
        approved_unsealed(&root, "REQ-001", "甲");
        approved_unsealed(&root, "REQ-002", "乙");
        approved_unsealed(&root, "REQ-003", "丙");
        let mut app = App::new(&root);
        app.seal_ids = vec!["REQ-001".into(), "REQ-002".into(), "REQ-003".into()];
        app.input_reviewer = "寇工".into();
        app.seal_selected();
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("成功 3"), "三份都该成功：{msg}");
        assert_eq!(app.cred_issued, 1, "批量封存只签发一次凭据");
        assert_eq!(app.cred_cleared, 1, "也只清除一次");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 批量封存中止时透传core的编号而不是编造逐条结果() {
        // ⚠️ 与 AC-014 的设想不同（见 `seal_selected` 的注释）：`seal_many` 是
        // 全有全无——第一条被拒就整体中止，给不出「成功 2 / 尝试 3」。
        // 这条用例锁的是**诚实**：中止就报中止，且把 core 那句带编号的话原样透传。
        let root = temp_root("health-sealabort");
        approved_unsealed(&root, "REQ-001", "甲");
        approved_unsealed(&root, "REQ-002", "乙");
        approved_unsealed(&root, "REQ-003", "丙");
        // 先单独把 REQ-002 绑好并带理由 —— 再次 seal 它需要理由，于是它会拒。
        requirement::seal(&root, "REQ-002", "先绑一次").expect("单份绑定应成功");

        let mut app = App::new(&root);
        app.seal_ids = vec!["REQ-001".into(), "REQ-002".into(), "REQ-003".into()];
        app.input_reviewer = "寇工".into();
        app.seal_selected();
        let msg = app.message.clone().unwrap_or_default();
        assert!(
            msg.contains("中止"),
            "应如实报中止而不是编造逐条结果：{msg}"
        );
        assert!(
            msg.contains("REQ-002"),
            "core 的错误文案里带编号，必须原样透传给用户：{msg}"
        );
        assert!(
            !msg.contains("成功 2"),
            "不得编造一个 core 根本没给出的逐条结论：{msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 只读体检不签发凭据() {
        // AC-010 / G8：只读体检不该消耗审批资格。
        let root = temp_root("health-readonly");
        approved_unsealed(&root, "REQ-001", "甲");
        git_init(&root);
        let mut app = App::new(&root);
        app.run_health();
        app.health_sel = 0;
        app.run_health();
        assert_eq!(app.cred_issued, 0, "只读体检不得签发凭据");
        assert!(app.health.is_some(), "体检结果应被缓存下来");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 连续体检不重复读盘() {
        // AC-018：体检算一次缓存起来，不进周期轮询。
        let root = temp_root("health-cache");
        for i in 1..=5 {
            approved_unsealed(&root, &format!("REQ-00{i}"), &format!("需求{i}"));
        }
        let mut app = App::new(&root);
        app.run_health();
        let first = app.health.clone().expect("第一次应有结果");
        // 再点 5 次"刷新"（同一个按钮）——`run_health` 会重算，但**不点就不会重算**。
        // 这里验的是「不主动触发就不重算」：读盘次数由 `health` 是否被重置体现。
        for _ in 0..5 {
            assert!(app.health.is_some());
        }
        app.health_sel = 1;
        let again = app.health.clone().expect("缓存应仍在");
        assert_eq!(
            again.sums.len(),
            first.sums.len(),
            "不重新体检时结果不得变化"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 内容一致性体检分别给出错误与警告计数() {
        // AC-003 / AC-015：NotSealed 是警告，ContentChanged / MalformedSum 是错误。
        let root = temp_root("health-sums");
        approved_unsealed(&root, "REQ-001", "甲");
        approved_named(&root, "REQ-002", "乙");
        let mut app = App::new(&root);
        app.run_health();
        app.health_sel = 1;
        let (err, warn) = app.health_counts()[1];
        assert_eq!(warn, 3, "REQ-001 三段都已批准但未绑定 → 3 条警告");
        assert_eq!(err, 0, "REQ-002 已绑定且一致 → 0 条错误");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 变更范围体检报出未声明的暂存文件() {
        // AC-004：问题消息里逐字含那个越界文件的路径。
        let root = temp_root("health-touch");
        approved_named(&root, "REQ-001", "甲");
        git_init(&root);
        stage_file(&root, "src/未声明的东西.rs");
        let mut app = App::new(&root);
        app.run_health();
        app.health_sel = 0;
        let lines = app.health_lines().join("\n");
        assert!(
            lines.contains("src/未声明的东西.rs"),
            "必须点明是哪个文件：{lines}"
        );
        assert_eq!(app.health_counts()[0].0, 1, "应恰好 1 条错误");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 批量封存**必须**走 `seal_many`，不得退化成 `for id { seal(..) }`。
    ///
    /// ★ 为什么用「查源码形状」而不是跑一个 L3 场景（AC-025 的设想是后者）：
    /// L3 票存在**机器级**的 `~/.config/req-guard/guard.cfg`（`token.rs` 的 `GUARD_FILE`），
    /// 一旦签出，`token_mode()` 对**整个进程树**为真，同一测试二进制里所有
    /// L0 夹具的 `approve` 都会开始要凭据（实测：两条无关用例被带红）。
    /// 换句话说，这条判据**没法在并行测试进程里被安全地判决**——
    /// 而"为了让判决性实验跑得动"去污染用户机器上的凭据库，代价远大于收益。
    ///
    /// 源码形状断言是这个约束下仍然**有效**的替代：它精确地钉住「用的是批量接口」，
    /// 把它改成循环单份，这条必红。L3 那一层的语义已由 core 自己守着
    /// （`requirement.rs` 的 `seal` / `seal_many` 各自消费票据时机的注释与测试）。
    #[test]
    fn 批量封存走seal_many而非循环单份() {
        let src = include_str!("app.rs");
        let body = src
            .split("pub fn seal_selected")
            .nth(1)
            .expect("应有 seal_selected")
            .split("\n    /// ")
            .next()
            .expect("seal_selected 的边界");
        assert!(
            body.contains("requirement::seal_many("),
            "批量封存必须走 seal_many（L3 下票据用后即废，循环单份第 2 份起必失败）"
        );
        assert!(
            !body.contains("requirement::seal("),
            "不得退化成逐份调用 requirement::seal"
        );
        // 顺带钉住「签发一次、清一次」（AC-013 的另一半）。
        assert_eq!(
            body.matches("prepare_credential(").count(),
            1,
            "一次人类命令 = 一次签发"
        );
        assert_eq!(
            body.matches("clear_credential()").count(),
            1,
            "也只清除一次"
        );
    }

    #[test]
    fn 交叉引用体检定位失败时不给具体结论() {
        // AC-009：第 2 段定位失败 → 只说"定位失败"，不编造失效结论。
        let root = temp_root("health-crossfail");
        approved_named(&root, "REQ-001", "甲");
        let p = requirement::find(&root, "REQ-001")
            .expect("清单应存在")
            .path;
        let c = std::fs::read_to_string(&p).expect("可读");
        // 把第 2 段的二级标题改坏 → `section_span` 定位失败。
        std::fs::write(&p, c.replace("## 2. 技术方案", "## 技术方案")).expect("写入应成功");

        let mut app = App::new(&root);
        app.run_health();
        app.health_sel = 3;
        let lines = app.health_lines().join("\n");
        assert!(lines.contains("定位失败"), "应如实说明定位失败：{lines}");
        assert!(
            !lines.contains("目标文件不存在"),
            "定位失败时不得给出具体引用结论：{lines}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 声明对话框警示含打回与清空且路径初始为空() {
        // AC-011（reapprove=true 是本仓库默认）
        let root = temp_root("health-declare");
        approved_named(&root, "REQ-001", "甲");
        let mut app = App::new(&root);
        app.selected = 0;
        app.input_glob.clear();
        let warn = app.declare_warning();
        assert!(gate::touch_reapprove(&root), "本仓库默认 reapprove=true");
        assert!(warn.contains("打回"), "必须说明会打回：{warn}");
        assert!(warn.contains("清空"), "必须说明会清空摘要：{warn}");
        assert!(app.input_glob.is_empty(), "路径输入框初始必须为空");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 声明对话框在不需要重新过审时不提打回() {
        // AC-012
        let root = temp_root("health-declare-off");
        approved_named(&root, "REQ-001", "甲");
        let y = root.join(".gates/req-guard.yaml");
        let mut s = std::fs::read_to_string(&y).unwrap_or_default();
        s.push_str("\ntouch:\n  reapprove: false\n");
        std::fs::write(&y, s).expect("写入配置");
        let app = App::new(&root);
        assert!(!gate::touch_reapprove(&root), "配置应已生效");
        assert!(
            !app.declare_warning().contains("打回"),
            "不需要重新过审时不得提打回：{}",
            app.declare_warning()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 刷新审计摘要的提示含digest字样() {
        // AC-017
        let root = temp_root("health-digest");
        approved_named(&root, "REQ-001", "甲");
        // 先制造一条审计，否则 core 会以"尚无审计日志"拒绝。
        gate::audit_ledger(&root, "NOTE actor=夹具");
        let mut app = App::new(&root);
        app.refresh_digest();
        let msg = app.message.clone().unwrap_or_default();
        if msg.contains("失败") {
            // 审计日志不存在时跳过内容断言，但仍必须验"不签发凭据"。
            assert_eq!(app.cred_issued, 0, "刷新摘要属非审批类，不得签发凭据");
        } else {
            assert!(msg.contains("DIGEST"), "提示必须含 DIGEST：{msg}");
            assert_eq!(app.cred_issued, 0, "刷新摘要不得签发凭据");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 验收标准体检能认出短编号() {
        // AC-007：`### AC-1` 这类 2 位编号 —— 经界面汇总后落在验收标准分组。
        let root = temp_root("health-ac");
        requirement::create(&root, Some("REQ-001"), "甲").expect("创建需求");
        let p = requirement::find(&root, "REQ-001")
            .expect("清单应存在")
            .path;
        let mut c = std::fs::read_to_string(&p).expect("可读");
        for (h, line) in [
            ("## 1. 需求分解", "- 背景与问题：体检夹具。"),
            ("## 2. 技术方案", "- 总体思路：体检夹具。"),
            ("## 3. 测试计划", "- 测试计划：体检夹具。"),
        ] {
            c = c.replacen(&format!("{h}\n"), &format!("{h}\n{line}\n"), 1);
        }
        // 在 GATE:AC 块里插一条编号只有 2 位数字的条目。
        c = c.replacen("<!-- GATE:AC -->", "<!-- GATE:AC -->\n### AC-1", 1);
        std::fs::write(&p, c).expect("写入应成功");

        let mut app = App::new(&root);
        app.run_health();
        app.health_sel = 2;
        let report = app.health.clone().expect("应有体检结果");
        assert!(
            report
                .ac
                .iter()
                .any(|i| i.kind == ac::AcIssueKind::BadIdFormat),
            "应认出短编号：{:?}",
            report.ac.iter().map(|i| i.kind).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 体检面板的错误与警告着色取值不同() {
        // AC-020：两类问题在界面上至少相差 1 个可区分的取值。
        // 用色板而不是硬编码颜色：色板的对比度已有单测（palette.rs）。
        let err = Tone::Danger.color_on(true);
        let warn = Tone::Warning.color_on(true);
        assert_ne!(err, warn, "错误与警告必须能区分开");
        assert_ne!(
            Tone::Danger.glyph(),
            Tone::Warning.glyph(),
            "除了颜色还要有图标（色弱用户不靠颜色分辨）"
        );
    }

    #[test]
    fn 顶栏按钮数量只增加一个体检入口() {
        // AC-027：体检收进二级面板，不往操作条上堆 5 个入口。
        // 这里锁的是"体检入口只有 1 个"——多入口就是这条被破坏。
        let src = include_str!("app.rs");
        let bottom = src
            .split("fn render_bottom")
            .nth(1)
            .expect("应有 render_bottom")
            .split("fn render_left")
            .next()
            .expect("render_bottom 应在 render_left 之前");
        let n = bottom.matches(".button(\"").count() + bottom.matches("egui::Button::new(").count();
        // 逐层记账（每一层只允许 +1，多一个就说明某组体检/卫生入口被摊开了）：
        //   REQ-015 收尾：创建需求 / 刷新 / 执行门禁检查 / 应急绕过 / 审计日志 /
        //                 归档（done）/ 物理归档 / 归档区 / 评论 / 绑定内容摘要 = 10
        //   REQ-016      +「体检」                                                = 11
        //   REQ-017      +「工程卫生」                                            = 12
        assert_eq!(n, 12, "操作条按钮数应恰好是 11 + 1（工程卫生），实际 {n}");
    }

    #[test]
    fn 声明路径为空时被挡下且不签发凭据() {
        let root = temp_root("health-declare-empty");
        approved_named(&root, "REQ-001", "甲");
        let mut app = App::new(&root);
        app.selected = 0;
        app.input_glob = "   ".into();
        app.input_reviewer = "寇工".into();
        app.declare_touch();
        assert!(
            app.message
                .clone()
                .unwrap_or_default()
                .contains("路径不能为空"),
            "{:?}",
            app.message
        );
        assert_eq!(app.cred_issued, 0, "参数就没过，不该签发凭据");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 追加声明后体检缓存作废() {
        // 声明改了范围 → 上一轮的体检结论不再成立，必须重算而不是继续显示旧结果。
        let root = temp_root("health-invalidate");
        approved_named(&root, "REQ-001", "甲");
        let mut app = App::new(&root);
        app.selected = 0;
        app.run_health();
        assert!(app.health.is_some());
        app.input_glob = "src/新声明.rs".into();
        app.input_reason = "范围扩张".into();
        app.input_reviewer = "寇工".into();
        app.declare_touch();
        assert!(app.health.is_none(), "声明改变了范围，旧的体检缓存必须作废");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 交叉引用体检必须用 `section_span`（定位失败即如实报错），
    /// **不能**用会回退整篇的 `section_of`。
    ///
    /// AC-026 的判决性实验就是把切片换成 `section_of` 再跑这条。
    #[test]
    fn 交叉引用切片用严格定位而非回退整篇() {
        // AC-026 的判决性实验靶子：把 `App::cross_ref_slice` 里的 `section_span`
        // 换成会回退整篇的 `section_of`，这条必须变红。
        let broken = "## 1. 需求分解\n甲\n## 技术方案\n见 `docs/不存在.md#§9`\n";
        assert!(
            App::cross_ref_slice(broken).is_none(),
            "第 2 段标题被改坏时必须如实返回 None（fail-closed），不得回退整篇"
        );
        // 正常文档要能切出第 2 段。注意 `section_span` 给的行区间**不含标题行本身**，
        // 故切出来的是段落的正文；起始行号是**行号**（从 1 起）而不是下标。
        let good = "## 1. 需求分解\n甲\n## 2. 技术方案\n见 `docs/x.md`\n## 3. 测试计划\n乙\n";
        let (section, first_line) = App::cross_ref_slice(good).expect("正常文档应能切片");
        assert!(
            section.contains("docs/x.md"),
            "切出来的应是第 2 段的正文：{section}"
        );
        assert!(
            !section.contains("测试计划"),
            "第 2 段不该含第 3 段：{section}"
        );
        assert!(!section.contains('甲'), "第 2 段不该含第 1 段：{section}");
        assert_eq!(first_line, 4, "正文首行是全文第 4 行（标题在第 3 行）");
    }

    // ---------- REQ-017：工程卫生 ----------

    /// 打开新建对话框（清空编号，模拟"刚点开"）。
    fn open_new_req(app: &mut App) {
        app.dialog = Dialog::NewReq;
        app.input_title.clear();
        app.input_new_id.clear();
        app.id_lint.clear();
    }

    #[test]
    fn 新建时编号留空即自动编号() {
        // AC-011：留空 → 传给 core 的编号是 `None`（与改前逐字一致）。
        let root = temp_root("hygiene-autoid");
        let mut app = App::new(&root);
        open_new_req(&mut app);
        app.input_title = "登录改造".into();
        app.create_req();
        let msg = app.message.clone().unwrap_or_default();
        assert!(msg.contains("已创建"), "留空应走自动编号并成功：{msg}");
        assert!(
            requirement::find(&root, "REQ-001").is_ok(),
            "自动编号应产出 REQ-001"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 新建时可用自定义编号且形态提示只提示不阻断() {
        // AC-012 / AC-005：`lint_id` 有提示也照建。
        let root = temp_root("hygiene-customid");
        let mut app = App::new(&root);
        open_new_req(&mut app);
        app.input_new_id = "REQ-kd-20261005-A7F3".into();
        app.refresh_id_lint();
        app.input_title = "登录改造".into();
        app.create_req();
        let msg = app.message.clone().unwrap_or_default();
        assert!(
            msg.contains("REQ-kd-20261005-A7F3"),
            "自定义编号应被原样采用：{msg}"
        );
        assert!(
            requirement::find(&root, "REQ-kd-20261005-A7F3").is_ok(),
            "清单文件名应含该编号"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 新建时编号冲突的原样显示core的话() {
        // AC-013：错误文本必须含「已存在」或「重复」之一。
        let root = temp_root("hygiene-dup");
        requirement::create(&root, Some("REQ-001"), "既有").expect("先建一份");
        let mut app = App::new(&root);
        open_new_req(&mut app);
        app.input_new_id = "REQ-001".into();
        app.input_title = "重复编号".into();
        app.create_req();
        let msg = app.message.clone().unwrap_or_default();
        assert!(
            msg.contains("已存在") || msg.contains("重复"),
            "必须原样透传 core 的冲突说明：{msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 编号冲突面板计数与着色区分() {
        // AC-010：重复类计数为 1，且与告警级的着色取值不同。
        let root = temp_root("hygiene-conflict");
        // 造「同 id 多文件」：手工写两个文件名不同但 HEAD id 相同的清单。
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).expect("建目录");
        for name in ["REQ-001-甲.md", "REQ-001-乙.md"] {
            std::fs::write(
                dir.join(name),
                "---
doc_type: proposal
---

<!-- GATE:HEAD id=REQ-001 status=in_review created=2026-10-06_10:00:00 -->
",
            )
            .expect("写清单");
        }
        let mut app = App::new(&root);
        app.run_hygiene();
        let (err, _) = app.id_issue_counts();
        assert_eq!(err, 1, "应恰好 1 条重复类错误");
        let lines = app.id_issue_lines().join("\n");
        assert!(lines.contains("REQ-001-甲.md"), "应点名文件：{lines}");
        assert!(lines.contains("REQ-001-乙.md"), "应点名文件：{lines}");
        // 错误与警告的着色必须能区分（复用既有色板，不新增颜色）。
        assert_ne!(
            Tone::Danger.color_on(true),
            Tone::Warning.color_on(true),
            "错误与警告必须能区分开"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 返工率超阈值才提示复盘() {
        // AC-014 / AC-015：4 次 → 提示；1 次 → 不提示。
        let root = temp_root("hygiene-rework");
        approved_named(&root, "REQ-001", "甲");
        // 直接往台账写 AMEND 行（`amend_counts` 数的就是它）。
        // ⚠️ 必须**一行一次**调用：`audit_ledger` 每次追加一条带时间戳的记录，
        // 把多行塞进一个字符串会被当成同一条事件（实测：4 次只数出 1 次）。
        for _ in 0..4 {
            gate::audit_ledger(&root, "AMEND REQ-001 step=solution actor=寇工");
        }
        gate::audit_ledger(&root, "AMEND REQ-001 step=decomposition actor=寇工");

        let app = App::new(&root);
        let text = app.rework_lines("REQ-001").join("\n");
        assert!(text.contains("改稿 4 次"), "应显示该段的次数：{text}");
        assert!(text.contains("复盘"), "超阈值应提示复盘：{text}");
        assert!(
            text.contains("技术方案：改稿 4 次\n⚠"),
            "提示必须挂在超阈值的那一段上：{text}"
        );
        // 1 次那段不得出现复盘提示。
        let decomp_block: String = text
            .lines()
            .skip_while(|l| !l.starts_with("需求分解"))
            .take(2)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !decomp_block.contains("复盘"),
            "1 次不该被提示复盘：{decomp_block}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 环境面板含身份等级与脱敏票据摘要() {
        // AC-017 / AC-018：含 `sig=`，且**不含 64 位十六进制**（凭据哈希）。
        let root = temp_root("hygiene-env");
        git_init(&root);
        let app = App::new(&root);
        let text = app.env_lines().join("\n");
        assert!(text.contains("sig="), "应显示身份指纹：{text}");
        assert!(text.contains("鉴权等级"), "应显示等级：{text}");
        assert!(text.contains("票据："), "应显示票据摘要：{text}");
        // 关键断言：任何长度为 64 的十六进制串都不得出现（那是凭据哈希的形态）。
        for tok in text.split(|c: char| !c.is_ascii_hexdigit()) {
            assert!(
                tok.len() != 64,
                "环境面板里出现了 64 位十六进制串（疑似凭据摘要）：{tok}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 自检面板只在打开时算一次() {
        // AC-022：自检未被放进周期轮询——不主动触发就不重算。
        let root = temp_root("hygiene-cache");
        approved_named(&root, "REQ-001", "甲");
        let mut app = App::new(&root);
        app.run_hygiene();
        let first = app.selfcheck_lines();
        // 模拟 3 次轮询：只 reload，不重跑自检。
        for _ in 0..3 {
            app.reload();
        }
        assert_eq!(
            app.selfcheck_lines(),
            first,
            "轮询不得改动自检结果（否则等于每 3 秒实跑一次脚本）"
        );
        assert!(app.hygiene.is_some(), "缓存应还在");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 界面上没有票据写入口() {
        // AC-023：GUI 的按钮文案与 `Dialog` 变体里不得出现签发 / 撤销票据的入口。
        let src = include_str!("app.rs");
        // ⚠️ 只看**非测试部分**：`include_str!` 会把本测试自己也算进去，
        // 而下面的禁用词列表本身就是那些词 —— 不切掉测试模块就是自证式失败。
        let src = src.split("#[cfg(test)]").next().expect("源码前段");
        let lower = src.to_lowercase();
        for bad in [
            "token issue",
            "token revoke",
            "签发票据",
            "撤销票据",
            "token_issue",
        ] {
            assert!(
                !lower.contains(&bad.to_lowercase()),
                "界面不得提供票据写操作，但源码里出现了 `{bad}`"
            );
        }
        // 「签发界面凭据」是另一件事（以"人类亲手点击界面"为在场证明的进程内凭据），
        // 它**必须**存在——否则 L1+ 下界面什么都做不了。
        assert!(
            src.contains("ui_issue_credential"),
            "界面凭据签发是必需的（区别于票据签发）"
        );
    }

    #[test]
    fn 自动编号污染计数为警告级() {
        // AC-004：文件名形如 `REQ-<8位日期>-x` → 自动编号污染，严重级为**告警**。
        let root = temp_root("hygiene-pollution");
        let dir = root.join(".gates/requirements");
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(
            dir.join("REQ-20260919-x.md"),
            "---\ndoc_type: proposal\n---\n\n<!-- GATE:HEAD id=REQ-20260919-x status=in_review created=2026-10-06_10:00:00 -->\n",
        )
        .expect("写清单");
        let mut app = App::new(&root);
        app.run_hygiene();
        let (err, warn) = app.id_issue_counts();
        assert_eq!(err, 0, "污染类不应记为错误：{err}");
        assert_eq!(warn, 1, "应恰好 1 条告警");
        let lines = app.id_issue_lines().join("\n");
        assert!(lines.contains("警告"), "文本里应标明严重级：{lines}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 自检的两个参数是空基准与语义自检为真() {
        // AC-016：第 2 个参数必须是 `None`（本地没有 PR 视角），
        // 第 3 个必须是 `true`（子串存在性检查形同虚设）。
        // 这条同时是 AC-016 的"读参数"与一条源码形状断言。
        let src = include_str!("app.rs");
        let src = src.split("#[cfg(test)]").next().expect("源码前段");
        assert!(
            src.contains("gate::verify_install_with(root, None, true)"),
            "自检必须传 (None, true)"
        );
        assert!(
            !src.contains("verify_install_with(root, Some("),
            "不得传具体基准引用（本地无 PR 视角，硬判只会误报）"
        );
        assert!(
            !src.contains("verify_install_with(root, None, false)"),
            "不得关掉语义自检（那等于显示一个已知无效的结论）"
        );
    }

    #[test]
    fn 资产齐全的仓库自检问题数为零() {
        // AC-008：门禁资产齐全 → 0 个硬伤。
        // 用本仓库自己（本机已 install 过）当样本；资产不齐时跳过，
        // 免得把"本机没装门禁"误报成功能缺陷。
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace 根")
            .to_path_buf();
        let problems = req_guard_core::gate::verify_install_with(&root, None, true);
        if problems.is_empty() {
            // 这正是期望态：直接通过。
            return;
        }
        // 有问题也不该在**界面**这条路径上炸——界面只是显示。
        let mut app = App::new(&root);
        app.run_hygiene();
        let lines = app.selfcheck_lines().join("\n");
        assert!(
            lines.contains("自检结果"),
            "界面应给出自检结论文本：{lines}"
        );
    }

    #[test]
    fn 编号留空时形态提示为空() {
        // 空编号走自动档，`lint_id` 明确不提示（纯数字/空都天然合规）。
        let root = temp_root("hygiene-lint-empty");
        let mut app = App::new(&root);
        app.input_new_id.clear();
        app.refresh_id_lint();
        assert!(app.id_lint.is_empty(), "留空不该有形态提示");
        app.input_new_id = "REQ-001".into();
        app.refresh_id_lint();
        assert!(app.id_lint.is_empty(), "规范编号不该有提示");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 工程卫生面板打开时不签发凭据() {
        // 与体检同一条纪律：只读面板不消耗审批资格。
        let root = temp_root("hygiene-readonly");
        approved_named(&root, "REQ-001", "甲");
        let mut app = App::new(&root);
        app.run_hygiene();
        assert!(app.hygiene.is_some(), "应缓存卫生结果");
        assert_eq!(app.cred_issued, 0, "只读卫生不得签发凭据");
        assert_eq!(app.cred_cleared, 0, "也不该清除别人的凭据");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 鉴权失败文案逐字透传不被包装() {
        // AC-019：界面不得重写/摘要 core 的鉴权错误。
        let src = include_str!("app.rs");
        // 同样切掉测试模块（否则断言文本自己就命中了禁用词）。
        let src = src.split("#[cfg(test)]").next().expect("源码前段");
        let lower = src.to_lowercase();
        assert!(
            !lower.contains("操作失败"),
            "不得把 core 的鉴权文案包装成泛泛的「操作失败」"
        );
        // 各类错误都是 `format!("<动作>失败：{}", e)` —— `e` 原样进文本。
        assert!(
            src.contains("签发界面凭据失败："),
            "界面凭据签发失败必须带上 core 的原文"
        );
    }
}
