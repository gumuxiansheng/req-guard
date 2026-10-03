//! 清单 frontmatter 的最小解析（规格时效契约的**声明侧**）。
//!
//! # 为什么不复用 doc-guard 的 matter 解析
//!
//! 两个 crate 各自独立发版、互不依赖（req-guard core 坚持零外部依赖，且是
//! "判定逻辑只有一份在 core" 的架构前提——见 lib.rs）。跨 crate 复用会把
//! req-guard 的门禁可用性绑到 doc-guard 的发布节奏上：doc-guard 挂了，
//! req-guard 的 approve 也用不了。这个方向的反依赖是不能接受的。
//!
//! 真正需要共享的是**字段名与取值枚举**，不是代码——它们必须与 doc-guard 的
//! FRS 族逐字对齐（`doc_type`/`tier`/`owner`/`review_policy`/`verified_at`/
//! `source_refs`），这样同一份清单能被 doc-guard 的 FRS001/003/004/005/007 与
//! DRF001 直接接管，本模块只负责"批准技术方案前不许留空 source_refs"。
//!
//! # 支持的子集（其余一律当作"没写"）
//!
//! - 首个非空行必须是 `---`，以独立一行 `---` 结束（与 doc-guard 同约定）
//! - `key: value` 与 `key: [a, b]` 行内数组
//! - `#` 整行注释、空行
//!
//! 与 doc-guard 的差别（**有意**）：本模块对语法越界**不报错**，只当作字段缺失。
//! 越界的 frontmatter 会由 doc-guard 的 FRS001 报"无法解析"并给出行号，
//! 两边都报等于把维护者引到两个地方；而门禁自身的拒绝理由必须是**可执行的**
//! （"source_refs 为空，请声明"），不是"你的 YAML 缩进错了"。

/// 从清单正文解析出的 frontmatter 字段（只保留本模块关心的键）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecMeta {
    /// `doc_type`（取值枚举由 doc-guard 的 FRS007 校验）
    pub doc_type: Option<String>,
    /// `tier`（`critical` / `standard`）
    pub tier: Option<String>,
    /// `owner`
    pub owner: Option<String>,
    /// `review_policy`（`periodic` / `codebound` / `historical`）
    pub review_policy: Option<String>,
    /// `verified_at`（`YYYY-MM-DD`）
    pub verified_at: Option<String>,
    /// `source_refs`（行内数组；空数组与缺字段等价为空）
    pub source_refs: Vec<String>,
}

impl SpecMeta {
    /// 是否已声明 `source_refs`（至少一个非空条目）。
    pub fn has_source_refs(&self) -> bool {
        self.source_refs.iter().any(|r| !r.trim().is_empty())
    }

    /// 是否为「绑定代码」策略——只有该策略下 `source_refs` 才有约束力。
    pub fn is_codebound(&self) -> bool {
        self.review_policy
            .as_deref()
            .map(|s| s.trim().eq_ignore_ascii_case("codebound"))
            .unwrap_or(false)
    }

    /// 缺 `source_refs` 时的可执行指引（与 doc-guard 的 source_refs 语义一致：
    /// 目录前缀 + 向下递归，不支持 glob）。
    pub fn missing_source_refs_hint(&self) -> String {
        let strategy = if self.is_codebound() {
            "当前 review_policy=codebound"
        } else {
            "未设 review_policy=codebound"
        };
        format!(
            "{strategy}：本清单未声明 source_refs，规格与代码之间没有绑定，\
             doc-guard 的 FRS004（源已改而规格未同步）与 DRF001（源路径不存在）都无从判断。\n\
             请在清单顶部的 frontmatter 里声明本次要改的文件或模块目录，例如：\n\
             \x20 source_refs: [backend/src/main/java/com/x, backend/src/main/resources/mapper]\n\
             注意：元素是**目录前缀 + 向下递归**，不支持 glob（写 backend/** 不会生效）。\n\
             存量清单可先声明一个宽前缀（如改动涉及的顶层模块目录），后续再收窄。"
        )
    }
}

/// 从清单正文提取 frontmatter。首个非空行不是 `---` 时返回 `None`。
///
/// 无 frontmatter 与「有 frontmatter 但没读到字段」都返回 `Some(全空)`——
/// 调用方据 `has_source_refs()` 判定，两者对它等价。
pub fn extract(content: &str) -> Option<SpecMeta> {
    let mut meta = SpecMeta::default();
    let mut in_block = false;
    for raw in content.lines() {
        let line = raw.trim_end_matches('\r');
        let t = line.trim();
        if !in_block {
            if t.is_empty() {
                continue;
            }
            // BOM 容错（与 doc-guard 的 matter::extract 一致）
            if t.trim_start_matches('\u{feff}').trim() == "---" {
                in_block = true;
                continue;
            }
            return None; // 首个非空行不是定界符 → 无 frontmatter
        }
        if t == "---" {
            break; // 结束定界
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            match key {
                "doc_type" => meta.doc_type = Some(val.to_string()),
                "tier" => meta.tier = Some(val.to_string()),
                "owner" => meta.owner = Some(val.to_string()),
                "review_policy" => meta.review_policy = Some(val.to_string()),
                "verified_at" => meta.verified_at = Some(val.to_string()),
                "source_refs" => meta.source_refs = parse_inline_list(val),
                _ => {}
            }
        }
    }
    if in_block {
        Some(meta)
    } else {
        None
    }
}

/// 解析行内数组 `[a, b]`；非数组形态按单个值处理。
fn parse_inline_list(v: &str) -> Vec<String> {
    let inner = v
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(v);
    inner
        .split(',')
        .map(|s| s.trim().trim_matches(['"', '\'']).trim().to_string())
        .filter(|s| !s.is_empty() && s != "-" && s != "null")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FM: &str = "---\ndoc_type: proposal\ntier: standard\nowner: 寇工\n\
                     review_policy: codebound\nverified_at: 2026-10-03\n\
                     source_refs: [backend/src/main/java/com/x, docs/guide]\n---\n\n# REQ-001\n";

    #[test]
    fn 解析_标准模板() {
        let m = extract(FM).expect("应有 frontmatter");
        assert_eq!(m.doc_type.as_deref(), Some("proposal"));
        assert_eq!(m.tier.as_deref(), Some("standard"));
        assert_eq!(m.owner.as_deref(), Some("寇工"));
        assert!(m.is_codebound());
        assert_eq!(m.verified_at.as_deref(), Some("2026-10-03"));
        assert_eq!(
            m.source_refs,
            vec!["backend/src/main/java/com/x", "docs/guide"]
        );
        assert!(m.has_source_refs());
    }

    #[test]
    fn 解析_无frontmatter返回_none() {
        assert!(extract("# REQ-001\n\n正文").is_none());
        assert!(extract("").is_none());
        // 前置空行不影响（doc-guard 同约定：首个**非空**行）
        assert!(extract("\n\n# x").is_none());
    }

    #[test]
    fn 解析_空数组视为未声明() {
        let m = extract("---\nsource_refs: []\nreview_policy: codebound\n---\n\n# x").unwrap();
        assert!(!m.has_source_refs(), "空数组 = 未声明，不能当作通过");
        let m2 = extract("---\nsource_refs:\nreview_policy: codebound\n---\n\n# x").unwrap();
        assert!(!m2.has_source_refs(), "只有键无值同样算未声明");
        let m3 = extract("---\nreview_policy: codebound\n---\n\n# x").unwrap();
        assert!(!m3.has_source_refs(), "缺键算未声明");
    }

    #[test]
    fn 解析_占位符不算声明() {
        // AI 常填 `-` / `null` / 空项占位；这些都不构成真实绑定
        let m = extract("---\nsource_refs: [-, null, \"\", ]\n---\n\n# x").unwrap();
        assert!(!m.has_source_refs());
    }

    #[test]
    fn 解析_容忍_bom与_crlf() {
        let m = extract("\u{feff}---\r\nsource_refs: [a/b]\r\n---\r\n\r\n# x").unwrap();
        assert_eq!(m.source_refs, vec!["a/b"]);
    }

    #[test]
    fn 解析_未闭合块按已读到内容() {
        // 无结束定界：doc-guard 会报 FRS001 语法错；本模块只取已读到的字段，
        // 不代替它报错（两处都报会把维护者引到两个地方）
        let m = extract("---\nsource_refs: [a/b]\n\n# x").unwrap();
        assert_eq!(m.source_refs, vec!["a/b"]);
    }

    #[test]
    fn 指引文案_区分是否codebound() {
        let cb = SpecMeta {
            review_policy: Some("codebound".into()),
            ..Default::default()
        };
        assert!(cb
            .missing_source_refs_hint()
            .contains("review_policy=codebound"));
        let other = SpecMeta::default();
        assert!(other
            .missing_source_refs_hint()
            .contains("未设 review_policy=codebound"));
        assert!(other.missing_source_refs_hint().contains("不支持 glob"));
    }
}
