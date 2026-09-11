//! 中文字体内嵌。
//!
//! **必须做**：egui 默认字体（`default_fonts`）只含拉丁字形，不含 CJK。
//! 不内嵌中文字体的话，界面上所有中文都会变成"豆腐块"（□），等于不可用。
//!
//! 字体来源：Noto Sans SC（SIL OFL 许可，可商用、可内嵌）。
//! 生成方式见 `scripts/make_font_subset.py`（下载完整字体 → 子集化 → 输出到本目录）。

use eframe::egui;

/// Noto Sans SC 子集：ASCII + GB2312 汉字 + 常用标点与界面符号。
const NOTO_SC: &[u8] = include_bytes!("../assets/NotoSansSC-Regular.otf");

/// 装载字体：把中文放到 Proportional 首位（拉丁字形仍由其后字体兜底）。
pub fn setup(cc: &eframe::CreationContext<'_>) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto_sc".into(),
        egui::FontData::from_static(NOTO_SC).into(),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "noto_sc".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("noto_sc".into());

    cc.egui_ctx.set_fonts(fonts);
}
