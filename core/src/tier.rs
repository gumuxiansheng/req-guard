//! 分级门禁的档位模型与 `classify` **纯函数**（REQ-019）。
//!
//! # 核心思想
//!
//! 「这份改动有多重」是一个**可复算的函数**，三层（L1 hook / L2 pre-commit / L3 CI）
//! 共用它 —— 判定逻辑只有一份是本仓的架构前提（见 [`crate::lib`] 模块文档）。
//!
//! ```text
//! final_tier = max(声明档 frontmatter.tier, 派生档 classify(...))
//! ```
//!
//! 声明只能往上抬、不能往下压；派生值由 L3 的完整变更集算出，是权威。
//!
//! # 为什么 `classify` 不碰文件系统
//!
//! 入参是「变更集的有效行明细 + 受管路径集」，不是 git 调用结果本身。这样
//! U-14/U-15/U-18/U-32 这类判据能 table-driven 单测覆盖，而不必每个用例都造仓库。
//! 取 git 的薄壳在 [`crate::tierdiff`]，组装在 [`gate_for`]。
//!
//! # 两条不可混为一谈的路（REQ-019 §2.10）
//!
//! - **G2**：算了之后判为 0 有效行（只改注释/空行）→ `trivial`；
//! - **R2b**：剥除 `touch.exempt` 后为空 → **根本不进入计算**，直接放行。
//!
//! 故本模块的入参是 `managed`（已剥除），**绝不在 classify 内部再读一次豁免集** ——
//! 豁免集单源（REQ-005 G3），两处各剥一次必然漂移。

use std::path::Path;

use crate::error::{GateError, Result};
use crate::tierdiff::{self, FileStat};

/// 档位（与 doc-guard 的 frontmatter `tier` **同一个枚举**，不是平行键 —— REQ-019 §2.9）。
///
/// 可用取值清单 [`TIERS`] 的顺序即「由轻到重」，两处（枚举校验文案、doc-guard
/// `freshness.rs` 的 `TIERS`）必须逐字一致 —— U-31 以字面量数组锁死这一点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Trivial,
    Light,
    Standard,
    Critical,
}

/// 可用取值清单（**唯一一份**，文案与校验都由它生成）。
pub const TIERS: [&str; 4] = ["trivial", "light", "standard", "critical"];

/// 逗号连接的取值清单（报错文案用）。
pub fn tiers_joined() -> String {
    TIERS.join("/")
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Trivial => "trivial",
            Tier::Light => "light",
            Tier::Standard => "standard",
            Tier::Critical => "critical",
        }
    }

    /// 解析档位字面量。**非法值返回 `None`** —— 绝不退化成某个合法值。
    ///
    /// 为什么不「非法即 `standard`」（REQ-019 §2.4 初稿口径）：AC-017 要求
    /// `tier: urgent` 被判为 `GateError::Validation`。**响亮的红优于沉默的默认值** ——
    /// 静默把非法值当 `standard` 会让维护者以为降级生效了，而实际生效的是另一套语义。
    /// 缺省（键不存在）才取 `standard`（存量清单零迁移，见 [`declared_of`]）。
    pub fn parse(s: &str) -> Option<Tier> {
        match s.trim() {
            "trivial" => Some(Tier::Trivial),
            "light" => Some(Tier::Light),
            "standard" => Some(Tier::Standard),
            "critical" => Some(Tier::Critical),
            _ => None,
        }
    }

    /// 档位序（0 轻 → 3 重），用于 `max()`。
    pub fn rank(self) -> u8 {
        match self {
            Tier::Trivial => 0,
            Tier::Light => 1,
            Tier::Standard => 2,
            Tier::Critical => 3,
        }
    }

    pub fn max_of(a: Tier, b: Tier) -> Tier {
        if a.rank() >= b.rank() {
            a
        } else {
            b
        }
    }

    /// 审批形态（人读；文案与设计文档共用一份，避免两处措辞漂移）。
    pub fn approval_hint(self) -> &'static str {
        match self {
            Tier::Trivial => "免审（免建清单；已建清单则只留 audit 行）",
            Tier::Light => "approve --all-steps（一条命令批三段，三段仍各留一条台账）",
            Tier::Standard | Tier::Critical => "三段逐段批准（strict_order 生效）",
        }
    }

    /// 轻档（含免审档）才是 AC 选填的范围。
    pub fn ac_optional(self) -> bool {
        matches!(self, Tier::Trivial | Tier::Light)
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 内建锁定路径（**常量，配置改不了**）：命中即恒为 `critical`。
///
/// 「门禁源码默认最高档不可修改」的实现口径是**报错而不是忽略**（见
/// [`crate::gate::tier_config`]）：配置里出现 `locked_paths` 键直接 `Validation`。
/// 静默忽略会让维护者以为降级生效了 —— 那是「看着在拦、其实没拦」的同族失效。
pub const LOCKED_PATHS: [&str; 4] = [
    "core/src/**",
    "templates/hooks/**",
    "templates/ci/**",
    ".gates/req-guard.yaml",
];

/// 分级配置（`.gates/req-guard.yaml` 的 `tier` 段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierConfig {
    /// 派生档是否生效。缺省段 / 段内全空 → `false`（回滚口：判定退回三段语义）。
    pub enabled: bool,
    pub trivial_max_lines: usize,
    pub light_max_lines: usize,
    /// 可配置高危路径（命中即**至少** `standard`）。
    pub risky_paths: Vec<String>,
}

impl Default for TierConfig {
    fn default() -> Self {
        TierConfig {
            enabled: false,
            trivial_max_lines: DEFAULT_TRIVIAL_MAX_LINES,
            light_max_lines: DEFAULT_LIGHT_MAX_LINES,
            risky_paths: Vec::new(),
        }
    }
}

/// 阈值初值（**待回放校准，不是结论** —— REQ-019 G7）。
pub const DEFAULT_TRIVIAL_MAX_LINES: usize = 5;
pub const DEFAULT_LIGHT_MAX_LINES: usize = 80;

impl TierConfig {
    /// 生效中的配置（`enabled=false` 时派生档一律按 `standard` 处理）。
    pub fn effective(&self) -> bool {
        self.enabled
    }
}

/// 路径敏感度判定结果（命中哪条 glob、来自内建还是配置）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSensitivity {
    /// 命中内建 `locked_paths` → 恒为 `critical`。
    Locked { glob: String },
    /// 命中配置 `risky_paths` → 至少 `standard`。
    Risky { glob: String },
}

impl PathSensitivity {
    /// 命中来源的字面量（输出理由用；AC-005 要求理由含 `locked`）。
    pub fn source(&self) -> &'static str {
        match self {
            PathSensitivity::Locked { .. } => "locked",
            PathSensitivity::Risky { .. } => "risky",
        }
    }

    pub fn glob(&self) -> &str {
        match self {
            PathSensitivity::Locked { glob } | PathSensitivity::Risky { glob } => glob,
        }
    }
}

/// `classify` 的入参（全部是**已取好的事实**，不含 I/O）。
#[derive(Debug, Clone)]
pub struct ClassifyInput<'a> {
    /// 声明档（frontmatter `tier`；缺省 → `standard`）。
    pub declared: Tier,
    /// 受管路径集（**R2b 已剥除豁免**，见模块文档）。
    pub managed: &'a [String],
    /// 逐文件有效行明细（可为空 —— 无变更行时按 0 参与）。
    pub stats: &'a [FileStat],
    pub config: &'a TierConfig,
}

/// `classify` 的输出（档位 + **人类可读的理由**）。
///
/// 理由必须可复算：否则「AI 少走流程」又变成一个无法质疑的黑箱（REQ-019 §2.8）。
#[derive(Debug, Clone)]
pub struct Classify {
    /// 声明档（原样带出，供输出对照）。
    pub declared: Tier,
    /// 按行数与路径敏感度算出的档位。
    pub derived: Tier,
    /// `max(声明, 派生)`。
    pub final_tier: Tier,
    pub total_effective: usize,
    /// 逐条理由（命中了哪条 glob、有效行总数、阈值）。
    pub reasons: Vec<String>,
    /// 逐文件有效行明细（排序后，供输出）。
    pub detail: Vec<String>,
}

impl Classify {
    /// 是否升档（派生 > 声明）→ `TierEscalation`。
    pub fn escalated(&self) -> bool {
        self.derived.rank() > self.declared.rank()
    }

    /// 一行摘要（`tier=<档>` + 有效行 + 声明/派生）。
    pub fn summary(&self) -> String {
        format!(
            "tier={}（声明 {} / 派生 {}，有效改动行 {}）",
            self.final_tier, self.declared, self.derived, self.total_effective
        )
    }
}

/// 档位判定核心（**纯函数**：不碰 git、不碰文件系统、不读配置）。
///
/// 判据（取最高者）：
///
/// | 档位 | 判据 |
/// | --- | --- |
/// | `critical` | 命中内建 `locked_paths`（**有效行为 0 也仍是 critical**） |
/// | `standard` | 命中 `risky_paths`，或有效行 > `light_max_lines` |
/// | `light` | 有效行 > `trivial_max_lines` 且未命中 risky/locked |
/// | `trivial` | 有效行 ≤ `trivial_max_lines` 且未命中 risky/locked |
pub fn classify(input: ClassifyInput<'_>) -> Classify {
    let ClassifyInput {
        declared,
        managed,
        stats,
        config,
    } = input;

    let total: usize = stats.iter().map(|f| f.effective()).sum();
    let mut reasons: Vec<String> = Vec::new();
    // 分级未启用时**整个派生过程都不发生**（含路径敏感度）——回滚口必须是真的
    // 「判定退回现行三段语义」，否则「配置置空」仍会把 `core/src/**` 判成 critical，
    // 那不是回滚，是半回滚（`TierConfig::enabled`）。
    let mut derived = declared;
    if config.effective() {
        derived = if total <= config.trivial_max_lines {
            Tier::Trivial
        } else if total <= config.light_max_lines {
            Tier::Light
        } else {
            Tier::Standard
        };
        reasons.push(format!(
            "有效改动行 {total}（阈值 trivial≤{} / light≤{}）",
            config.trivial_max_lines, config.light_max_lines
        ));
    }

    // 路径敏感度按**路径集**枚举（不看有效行）：锁定项必须连「有效行 0」也压得住（U-09）。
    // 逐文件命中在输出里单独点名，故这里按路径去重。
    if config.effective() {
        let mut seen: Vec<String> = Vec::new();
        for p in managed {
            let Some(hit) = path_sensitivity(p, config) else {
                continue;
            };
            let line = format!("命中 {} 路径 `{}`（{}）", hit.source(), p, hit.glob());
            if !seen.contains(&line) {
                seen.push(line.clone());
                reasons.push(line);
            }
            derived = Tier::max_of(derived, hit.floor());
        }
    }

    let final_tier = Tier::max_of(declared, derived);

    let detail = stats
        .iter()
        .map(|f| {
            format!(
                "  {}：有效新增 {} / 有效删除 {}{}",
                f.path,
                f.added,
                f.deleted,
                if f.binary {
                    "（二进制文件，按 1 行计）"
                } else {
                    ""
                }
            )
        })
        .collect();

    Classify {
        declared,
        derived,
        final_tier,
        total_effective: total,
        reasons,
        detail,
    }
}

impl PathSensitivity {
    /// 该命中把档位**至少**抬到多少。
    pub fn floor(self) -> Tier {
        match self {
            PathSensitivity::Locked { .. } => Tier::Critical,
            PathSensitivity::Risky { .. } => Tier::Standard,
        }
    }
}

/// 路径敏感度匹配（**纯函数**）。
///
/// glob 语义**复用** [`crate::touch::glob_match`]（`**` 跨层级、`*` 单层、精确文件名），
/// 另接受目录前缀写法（`core/src/`）并按向下递归归一 —— 与变更范围契约同一份语义，
/// 不另写一套（否则两套 glob 在边界上必然漂移）。
pub fn path_sensitivity(path: &str, config: &TierConfig) -> Option<PathSensitivity> {
    let p = crate::touch::normalize_path(path);
    for g in LOCKED_PATHS {
        if glob_hit(g, &p) {
            return Some(PathSensitivity::Locked {
                glob: g.to_string(),
            });
        }
    }
    for g in &config.risky_paths {
        if glob_hit(g, &p) {
            return Some(PathSensitivity::Risky {
                glob: g.to_string(),
            });
        }
    }
    None
}

/// 归一后的 glob 是否命中（目录前缀 → 向下递归；**纯函数**）。
pub fn glob_hit(pattern: &str, path: &str) -> bool {
    let raw = pattern.trim().replace('\\', "/");
    if raw.is_empty() {
        return false;
    }
    crate::touch::glob_match(&raw, path) || {
        let dir = raw.trim_end_matches('/');
        !dir.is_empty() && path.starts_with(&format!("{dir}/"))
    }
}

// ───────────────────────────── 声明档（frontmatter） ─────────────────────────────

/// 读清单 frontmatter 的 `tier` 声明档。
///
/// - 键不存在 → `standard`（存量 18 份清单全部写着 `tier: standard`，零迁移）；
/// - 键存在但取值非法 → [`GateError::Validation`]，文案带完整可用取值
///   （AC-017 要求含 `trivial` 与 `critical` 两个取值字样）。
pub fn declared_of(content: &str) -> Result<Tier> {
    let Some(raw) = frontmatter_scalar(content, "tier") else {
        return Ok(Tier::Standard);
    };
    let v = raw.trim();
    if v.is_empty() {
        return Err(tier_validation_err(v));
    }
    Tier::parse(v).ok_or_else(|| tier_validation_err(v))
}

fn tier_validation_err(v: &str) -> GateError {
    GateError::Validation(format!(
        "frontmatter 的 `tier` 取值非法：{:}。\n\
         可用取值：{}\n\
         （该字段与 doc-guard 的 `tier` 是同一个枚举，两侧取值必须一致）",
        v,
        tiers_joined()
    ))
}

/// 取 frontmatter 里的标量字段（**纯函数**）。
fn frontmatter_scalar(content: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines
        .iter()
        .position(|l| !l.trim().is_empty() && l.trim().trim_start_matches('\u{feff}') == "---")?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|i| start + 1 + i)?;
    lines[start + 1..end]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_string())
}

// ───────────────────────────── 组装（薄壳） ─────────────────────────────

/// 一次定档的完整结果（含「未进入定档」的短路情形）。
#[derive(Debug, Clone)]
pub enum TierGate {
    /// 定档已执行。
    Classified(Classify),
    /// R2b：剥除豁免后为空 ⇒ **不进入定档**（输出不得出现任何档位字段）。
    NoManagedPath,
}

/// 对一次变更集定档（薄壳：取 git → 剥豁免 → [`classify`]）。
///
/// `paths` 是**原始**变更集路径；豁免集单源由调用方传入（不在此处二次读取）。
pub fn gate_for(
    root: &Path,
    side: &tierdiff::Side,
    paths: &[String],
    exempt: &[String],
    declared: Tier,
    config: &TierConfig,
) -> Result<TierGate> {
    let is_exempt = |p: &str| {
        let p = crate::touch::normalize_path(p);
        exempt.iter().any(|g| crate::touch::glob_match(g, &p))
    };
    let managed: Vec<String> = paths
        .iter()
        .filter(|p| !p.trim().is_empty() && !is_exempt(p))
        .cloned()
        .collect();
    // R2b 短路：**根本没有变更集**（空路径）与「全豁免」是两件事，前者不短路。
    if managed.is_empty() || paths.iter().all(|p| p.trim().is_empty()) {
        return Ok(TierGate::NoManagedPath);
    }
    if !config.effective() {
        // 分级未启用：不算 git（省一次 `diff` + N 次 `git show`），只透出声明档。
        return Ok(TierGate::Classified(Classify {
            declared,
            derived: declared,
            final_tier: declared,
            total_effective: 0,
            reasons: Vec::new(),
            detail: Vec::new(),
        }));
    }
    let stat = tierdiff::stat(root, side, &managed)?;
    Ok(TierGate::Classified(classify(ClassifyInput {
        declared,
        managed: &managed,
        stats: &stat.files,
        config,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(path: &str, added: usize, deleted: usize) -> FileStat {
        FileStat {
            path: path.to_string(),
            added,
            deleted,
            binary: false,
        }
    }

    fn cfg() -> TierConfig {
        TierConfig {
            enabled: true,
            ..Default::default()
        }
    }

    fn run(
        declared: Tier,
        managed: &[&str],
        stats: Vec<FileStat>,
        config: &TierConfig,
    ) -> Classify {
        let owned: Vec<String> = managed.iter().map(|s| s.to_string()).collect();
        classify(ClassifyInput {
            declared,
            managed: &owned,
            stats: &stats,
            config,
        })
    }

    // ── 枚举 ────────────────────────────────────────────────────
    #[test]
    fn tiers_可用取值清单四值且顺序由轻到重() {
        assert_eq!(TIERS, ["trivial", "light", "standard", "critical"]);
        for (i, s) in TIERS.iter().enumerate() {
            assert_eq!(Tier::parse(s).unwrap().rank(), i as u8);
        }
    }

    #[test]
    fn declared_of_非法取值报错且文案含两端取值() {
        let err = declared_of("---\ntier: urgent\n---\n# x\n").unwrap_err();
        let m = err.to_string();
        assert!(m.contains("trivial") && m.contains("critical"), "{m}");
    }

    #[test]
    fn declared_of_缺省与历史值取standard() {
        assert_eq!(
            declared_of("---\nowner: -\n---\n# x\n").unwrap(),
            Tier::Standard
        );
        assert_eq!(
            declared_of("---\ntier: standard  \n---\n# x\n").unwrap(),
            Tier::Standard
        );
    }

    #[test]
    fn declared_of_空串取值报错而非默认() {
        assert!(declared_of("---\ntier: \"\"\n---\n").is_err());
    }

    // ── 档位判定 ────────────────────────────────────────────────
    #[test]
    fn u14_有效行三与六分别落trivial与light() {
        let c = cfg();
        let t = run(Tier::Standard, &[], vec![stat("a.txt", 3, 0)], &c);
        assert_eq!(t.derived, Tier::Trivial);
        let l = run(Tier::Standard, &[], vec![stat("a.txt", 6, 0)], &c);
        assert_eq!(l.derived, Tier::Light);
    }

    #[test]
    fn 有效行超阈值落standard() {
        let c = cfg();
        let r = run(Tier::Standard, &[], vec![stat("a.txt", 81, 0)], &c);
        assert_eq!(r.derived, Tier::Standard);
    }

    #[test]
    fn u15_声明不能压低派生档() {
        let c = cfg();
        let r = run(Tier::Light, &[], vec![stat("a.txt", 200, 0)], &c);
        assert_eq!(r.final_tier, Tier::Standard);
        assert!(r.escalated());
    }

    #[test]
    fn u16_声明可以抬高最终档() {
        let c = cfg();
        let r = run(Tier::Standard, &[], vec![stat("a.txt", 3, 0)], &c);
        assert_eq!(r.final_tier, Tier::Standard);
        assert!(!r.escalated());
    }

    #[test]
    fn u17_未启用分级时最终档逐字等于声明档() {
        let c = TierConfig::default();
        assert!(!c.effective());
        let r = run(
            Tier::Standard,
            &["core/src/gate.rs"],
            vec![stat("core/src/gate.rs", 999, 0)],
            &c,
        );
        assert_eq!(r.final_tier, Tier::Standard);
        assert_eq!(r.derived, Tier::Standard);
        assert!(r.reasons.is_empty());
    }

    // ── 路径敏感度 ──────────────────────────────────────────────
    #[test]
    fn u09_命中内建锁定即便零有效行也是critical() {
        let c = cfg();
        let r = run(
            Tier::Standard,
            &["core/src/gate.rs"],
            vec![stat("core/src/gate.rs", 0, 0)],
            &c,
        );
        assert_eq!(r.derived, Tier::Critical);
        assert!(
            r.reasons.iter().any(|x| x.contains("locked")),
            "{:?}",
            r.reasons
        );
    }

    #[test]
    fn u11_命中risky即至少standard() {
        let c = TierConfig {
            risky_paths: vec!["gui/src/**".into()],
            ..cfg()
        };
        let r = run(
            Tier::Standard,
            &["gui/src/app.rs"],
            vec![stat("gui/src/app.rs", 1, 0)],
            &c,
        );
        assert_eq!(r.derived, Tier::Standard);
        assert!(
            r.reasons.iter().any(|x| x.contains("risky")),
            "{:?}",
            r.reasons
        );
    }

    #[test]
    fn u12_目录前缀写法与星号写法等价() {
        let c1 = TierConfig {
            risky_paths: vec!["gui/src/**".into()],
            ..cfg()
        };
        let c2 = TierConfig {
            risky_paths: vec!["gui/src/".into()],
            ..cfg()
        };
        let p = ["gui/src/app.rs"];
        let r1 = run(Tier::Standard, &p, vec![stat("gui/src/app.rs", 1, 0)], &c1);
        let r2 = run(Tier::Standard, &p, vec![stat("gui/src/app.rs", 1, 0)], &c2);
        assert_eq!(r1.derived, r2.derived);
        assert_eq!(r2.derived, Tier::Standard);
    }

    #[test]
    fn u13_单星不跨层双星跨层与touch同语义() {
        for (pat, path, want) in [
            ("core/src/*.rs", "core/src/a.rs", true),
            ("core/src/*.rs", "core/src/x/a.rs", false),
            ("core/src/**", "core/src/x/a.rs", true),
            ("*.rs", "core/src/a.rs", false),
        ] {
            assert_eq!(
                crate::touch::glob_match(pat, path),
                want,
                "glob {pat} 对 {path}"
            );
            assert_eq!(glob_hit(pat, path), want, "glob_hit {pat} 对 {path}");
        }
    }

    #[test]
    fn u18_内建锁定压过配置项() {
        // 配置里把 core/src 列为 risky（想降级），锁定项仍判 critical。
        let c = TierConfig {
            risky_paths: vec!["core/src/**".into()],
            ..cfg()
        };
        let r = run(
            Tier::Trivial,
            &["core/src/tier.rs"],
            vec![stat("core/src/tier.rs", 1, 0)],
            &c,
        );
        assert_eq!(r.final_tier, Tier::Critical);
        assert!(
            r.reasons.iter().any(|x| x.contains("locked")),
            "{:?}",
            r.reasons
        );
    }

    #[test]
    fn 理由文本含locked与文件路径() {
        let c = cfg();
        let r = run(
            Tier::Standard,
            &["core/src/gate.rs"],
            vec![stat("core/src/gate.rs", 1, 0)],
            &c,
        );
        let joined = r.reasons.join("\n");
        assert!(joined.contains("locked"), "{joined}");
        assert!(joined.contains("core/src/gate.rs"), "{joined}");
        assert!(r.summary().contains("tier=critical"), "{}", r.summary());
    }

    #[test]
    fn path_sensitivity_内建锁定恒为四项() {
        let pairs = [
            ("core/src/**", "core/src/gate.rs"),
            (
                "templates/hooks/**",
                "templates/hooks/fragments/reqguard-check.sh",
            ),
            ("templates/ci/**", "templates/ci/req-guard-ci.yml"),
            (".gates/req-guard.yaml", ".gates/req-guard.yaml"),
        ];
        assert_eq!(LOCKED_PATHS.len(), pairs.len());
        for (g, p) in pairs {
            let hit = path_sensitivity(p, &cfg()).unwrap_or_else(|| panic!("{p} 未命中锁定项"));
            assert_eq!(hit.source(), "locked");
            assert_eq!(hit.glob(), g);
        }
    }

    #[test]
    fn path_sensitivity_未命中交由行数决定() {
        assert!(path_sensitivity("docs/设计/x.md", &cfg()).is_none());
        assert!(path_sensitivity("README.md", &cfg()).is_none());
    }

    #[test]
    fn 详情输出逐文件给有效行并含总行数() {
        let c = cfg();
        let r = run(
            Tier::Standard,
            &[],
            vec![stat("a.rs", 2, 1), stat("b.rs", 0, 3)],
            &c,
        );
        assert_eq!(r.total_effective, 6);
        let d = r.detail.join("\n");
        assert!(d.contains("a.rs：有效新增 2 / 有效删除 1"), "{d}");
        assert!(r.summary().contains("有效改动行 6"), "{}", r.summary());
    }

    // ── R2b 接口（§2.10） ───────────────────────────────────────
    #[test]
    fn u32_豁免路径不贡献有效行() {
        // 入参是**已剥除**的 managed：豁免路径根本不在里面，行数只来自受管路径。
        let c = cfg();
        let r = run(
            Tier::Standard,
            &["core/src/gate.rs"],
            vec![stat("core/src/gate.rs", 3, 0)],
            &c,
        );
        assert_eq!(r.total_effective, 3);
        assert!(
            !r.detail.iter().any(|d| d.contains(".gates/drafts/")),
            "{:?}",
            r.detail
        );
    }

    #[test]
    fn gate_for_空变更集走短路而不是trivial() {
        let c = cfg();
        let g = gate_for(
            std::path::Path::new("."),
            &tierdiff::Side::Staged,
            &[],
            &[".gates/requirements/**".to_string()],
            Tier::Standard,
            &c,
        )
        .unwrap();
        assert!(matches!(g, TierGate::NoManagedPath));
    }

    #[test]
    fn gate_for_全豁免走短路且不带档位字段() {
        let c = cfg();
        let g = gate_for(
            std::path::Path::new("."),
            &tierdiff::Side::Staged,
            &[".gates/requirements/REQ-004.md".to_string()],
            &[".gates/requirements/**".to_string()],
            Tier::Standard,
            &c,
        )
        .unwrap();
        assert!(matches!(g, TierGate::NoManagedPath));
    }
}
