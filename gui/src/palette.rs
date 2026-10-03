//! 语义色板：红=错误/失败、绿=成功/正常、琥珀=警告/提示、灰=中性/待定。
//!
//! ## 为什么不能直接用 `Color32::RED / GREEN / YELLOW / GRAY`
//!
//! 它们是**纯色**，对比度根本不达标（WCAG AA 要求正文 ≥ 4.5:1）：
//!
//! | 颜色 | 在 egui 浅色面板 `#F8F8F8` 上 | 在 egui 深色面板 `#1B1B1B` 上 |
//! |---|---|---|
//! | `Color32::YELLOW` | **1.01**（等于看不见，正是本次要修的） | 12.7（刺眼） |
//! | `Color32::GREEN`  | **1.29**（几乎隐身） | 13.9（刺眼） |
//! | `Color32::RED`    | 3.76（也不达标） | 4.6 |
//! | `Color32::GRAY`   | 3.72（不达标） | 5.4 |
//!
//! 同一个值不可能同时适配浅底与深底，所以这里**按主题取两套值**：
//! 浅底上用加深的低明度色（拉大与背景的亮度差），深底上用提亮的中高明度色。
//! 取值都在单测 `对比度在浅色与深色背景下都满足_wcag_aa` 里被守住。
//!
//! ## 色弱可辨识：颜色不是唯一线索
//!
//! 1. **图标**：每个色调配一个专属图标（✗ / ✓ / ⚠ / ○），形状本身就区分得开；
//! 2. **文字**：状态一律写明（"未解锁"/"已解锁"/"拦截"/"放行"），不靠颜色猜；
//! 3. **明度互不相同**：转成灰度后"危险"与"成功"仍能分开
//!    （浅色 0.137 vs 0.097，深色 0.366 vs 0.495）。
//!
//! 新增任何带色文字都请走 [`Tone`]，不要再写 `Color32::RED` 这类纯色。

use eframe::egui;

/// 语义色调（与界面上的"红/绿/黄"一一对应，只是取值换成了达标的版本）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// 错误 / 失败 / 拦截 / 阻塞。
    Danger,
    /// 成功 / 正常 / 已解锁 / 已通过。
    Success,
    /// 警告 / 提示（底部状态条的默认色调）。
    #[default]
    Warning,
    /// 中性 / 待定 / 无数据。
    Muted,
}

impl Tone {
    /// 浅色主题取值：底色接近白（egui 浅色面板 `#F8F8F8`，窗口内可能到 `#FFFFFF`，
    /// 嵌套面板最暗按 `#F0F0F0` 兜底），所以文字要**压暗**。
    fn on_light(self) -> egui::Color32 {
        match self {
            // #C62828：对比度 白 5.62 / #F8F8F8 5.29 / #F0F0F0 4.93
            Tone::Danger => egui::Color32::from_rgb(0xC6, 0x28, 0x28),
            // #166534：白 7.13 / #F8F8F8 6.71 / #F0F0F0 6.26
            Tone::Success => egui::Color32::from_rgb(0x16, 0x65, 0x34),
            // #92400E 深琥珀：白 7.09 / #F8F8F8 6.68 / #F0F0F0 6.22
            Tone::Warning => egui::Color32::from_rgb(0x92, 0x40, 0x0E),
            // #4B5563 深灰：白 7.56 / #F8F8F8 7.12 / #F0F0F0 6.63
            Tone::Muted => egui::Color32::from_rgb(0x4B, 0x55, 0x63),
        }
    }

    /// 深色主题取值：底色是 egui 深色面板 `#1B1B1B`（纯黑到 `#252525` 之间），
    /// 所以文字反过来要**提亮**，但不能提到纯色那么刺眼。
    fn on_dark(self) -> egui::Color32 {
        match self {
            // #FF7B72：#1B1B1B 6.83 / 纯黑 8.33 / #252525 6.08
            Tone::Danger => egui::Color32::from_rgb(0xFF, 0x7B, 0x72),
            // #56D364：#1B1B1B 8.94 / 纯黑 10.90 / #252525 7.95
            // 刻意比下面两个亮一档：色弱用户看不出红绿时，还能靠明暗分开。
            Tone::Success => egui::Color32::from_rgb(0x56, 0xD3, 0x64),
            // #D29922 琥珀金：#1B1B1B 6.82 / 纯黑 8.32 / #252525 6.07
            Tone::Warning => egui::Color32::from_rgb(0xD2, 0x99, 0x22),
            // #9CA3AF：#1B1B1B 6.78 / 纯黑 8.27 / #252525 6.04
            Tone::Muted => egui::Color32::from_rgb(0x9C, 0xA3, 0xAF),
        }
    }

    /// 按主题取色：`dark` = 当前是否为深色主题。
    pub fn color_on(self, dark: bool) -> egui::Color32 {
        if dark {
            self.on_dark()
        } else {
            self.on_light()
        }
    }

    /// 按主题取色。
    ///
    /// egui 0.36 里 `Visuals` **没有** `dark` 字段（旧版的 `visuals().dark` 已删），
    /// 当前主题要从 [`egui::Context::theme`] 问；本项目不做局部 `set_visuals` 覆盖，
    /// 所以"应用主题"就等于"这段文字所在主题"。若将来真做了局部覆盖，这里要改成
    /// 按所在 `Ui` 的 visuals 判断（比如比对 `panel_fill` 的亮度）。
    pub fn color_for(self, theme: egui::Theme) -> egui::Color32 {
        self.color_on(theme == egui::Theme::Dark)
    }

    /// 按当前 [`egui::Ui`] 的主题取色（深浅主题自动切换）。
    pub fn color(self, ui: &egui::Ui) -> egui::Color32 {
        self.color_for(ui.ctx().theme())
    }

    /// 配色之外的第二条线索：**专属图标**（色弱用户靠形状分辨）。
    ///
    /// 全部取自内嵌字体子集的符号区（`scripts/make_font_subset.py` 的
    /// `0x2600–0x26FF` / `0x2700–0x27BF`），不会出现豆腐块。
    pub fn glyph(self) -> &'static str {
        match self {
            Tone::Danger => "✗",
            Tone::Success => "✓",
            Tone::Warning => "⚠",
            Tone::Muted => "○",
        }
    }

    /// 带色 + 带图标的 [`egui::RichText`]：颜色与形状双线索，缺一也能分辨。
    pub fn rich(self, ui: &egui::Ui, text: impl Into<String>) -> egui::RichText {
        egui::RichText::new(format!("{} {}", self.glyph(), text.into())).color(self.color(ui))
    }

    /// 只上色、不加图标（用在文字本身已经把状态写清楚的场合，避免图标重复）。
    pub fn text(self, ui: &egui::Ui, text: impl Into<String>) -> egui::RichText {
        egui::RichText::new(text.into()).color(self.color(ui))
    }
}

/// 链接色（Markdown 里 `[文本](url)` 降级成的那串带下划线文本）。
///
/// 单列一个函数而不塞进 [`Tone`]：它不是**状态**语义色，但旧值 `#6EAAFF`
/// 在浅色面板上对比度只有 2.23，和黄色一样属于"浅到看不清"，一并按主题换掉。
///
/// - 浅色 `#1D4ED8`：白 6.70 / `#F8F8F8` 6.31 / `#F0F0F0` 5.88
/// - 深色 `#79C0FF`：`#1B1B1B` 8.86 / `#252525` 7.88
pub fn link_on(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(0x79, 0xC0, 0xFF)
    } else {
        egui::Color32::from_rgb(0x1D, 0x4E, 0xD8)
    }
}

/// 按当前 [`egui::Ui`] 的主题取链接色。
pub fn link(ui: &egui::Ui) -> egui::Color32 {
    link_on(ui.ctx().theme() == egui::Theme::Dark)
}

// ===================== 单测：守住对比度下限 =====================

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 相对亮度（sRGB → 线性 → 加权）。
    fn luminance(c: egui::Color32) -> f64 {
        let ch = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    /// WCAG 对比度（1.0 ~ 21.0）。
    fn contrast(fg: egui::Color32, bg: egui::Color32) -> f64 {
        let (a, b) = (luminance(fg), luminance(bg));
        let (hi, lo) = (a.max(b), a.min(b));
        (hi + 0.05) / (lo + 0.05)
    }

    /// egui 浅色主题可能出现的底色：窗口白、面板 `#F8F8F8`、嵌套面板最暗 `#F0F0F0`。
    const LIGHT_BG: [egui::Color32; 3] = [
        egui::Color32::WHITE,
        egui::Color32::from_rgb(0xF8, 0xF8, 0xF8),
        egui::Color32::from_rgb(0xF0, 0xF0, 0xF0),
    ];
    /// egui 深色主题可能出现的底色：纯黑到 `#252525`。
    const DARK_BG: [egui::Color32; 3] = [
        egui::Color32::BLACK,
        egui::Color32::from_rgb(0x1B, 0x1B, 0x1B),
        egui::Color32::from_rgb(0x25, 0x25, 0x25),
    ];

    /// 正文 AA 下限。
    const AA: f64 = 4.5;

    #[test]
    fn 对比度在浅色与深色背景下都满足_wcag_aa() {
        for tone in [Tone::Danger, Tone::Success, Tone::Warning, Tone::Muted] {
            for bg in LIGHT_BG {
                let got = contrast(tone.color_on(false), bg);
                assert!(
                    got >= AA,
                    "{tone:?} 在浅色背景 {bg:?} 上对比度仅 {got:.2}，低于 AA 的 {AA}"
                );
            }
            for bg in DARK_BG {
                let got = contrast(tone.color_on(true), bg);
                assert!(
                    got >= AA,
                    "{tone:?} 在深色背景 {bg:?} 上对比度仅 {got:.2}，低于 AA 的 {AA}"
                );
            }
        }
        // 链接色同属"要读的文字"，一并守。
        for bg in LIGHT_BG {
            assert!(contrast(link_on(false), bg) >= AA, "浅色链接色不达标");
        }
        for bg in DARK_BG {
            assert!(contrast(link_on(true), bg) >= AA, "深色链接色不达标");
        }
    }

    /// 反证：原来那几个纯色确实不达标（记在这里，免得有人改回 `Color32::GREEN`）。
    #[test]
    fn 原来用的纯色确实不达标() {
        let light = egui::Color32::from_rgb(0xF8, 0xF8, 0xF8);
        assert!(
            contrast(egui::Color32::YELLOW, light) < AA,
            "纯黄在浅色面板上应该不达标（实测 {}）",
            contrast(egui::Color32::YELLOW, light)
        );
        assert!(contrast(egui::Color32::GREEN, light) < AA);
        assert!(contrast(egui::Color32::RED, light) < AA);
        assert!(contrast(egui::Color32::GRAY, light) < AA);
        // 旧的链接蓝 #6EAAFF 同样不达标。
        assert!(contrast(egui::Color32::from_rgb(0x6E, 0xAA, 0xFF), light) < AA);
    }

    /// 色弱兜底：四个图标互不相同，且"危险"与"成功"的明度比 > 1.3（灰度下也能分开）。
    #[test]
    fn 图标互不相同且危险与成功明度错开() {
        let glyphs = [
            Tone::Danger.glyph(),
            Tone::Success.glyph(),
            Tone::Warning.glyph(),
            Tone::Muted.glyph(),
        ];
        for (i, a) in glyphs.iter().enumerate() {
            for b in glyphs.iter().skip(i + 1) {
                assert_ne!(a, b, "图标重复会让色弱用户彻底分不出状态");
            }
        }

        let ratio_light = luminance(Tone::Danger.on_light()) / luminance(Tone::Success.on_light());
        let ratio_dark = luminance(Tone::Success.on_dark()) / luminance(Tone::Danger.on_dark());
        assert!(
            ratio_light > 1.3,
            "浅色下危险/成功明度比仅 {ratio_light:.2}"
        );
        assert!(ratio_dark > 1.3, "深色下成功/危险明度比仅 {ratio_dark:.2}");
    }
}
