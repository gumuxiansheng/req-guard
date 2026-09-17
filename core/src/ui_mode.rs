//! 界面模式探测（`req-guard ui` 的"自动选界面"逻辑）。
//!
//! 判定与渲染分离：这里只回答"该用哪种界面"，不关心界面怎么实现，
//! 因此可以完整单测——而 UI 本身在 tui / gui 两个 crate 里。

use crate::error::{GateError, Result};

/// 可用的界面形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    /// 桌面图形界面（eframe）。
    Gui,
    /// 终端界面（ratatui）。
    Tui,
}

impl UiMode {
    pub fn as_str(self) -> &'static str {
        match self {
            UiMode::Gui => "gui",
            UiMode::Tui => "tui",
        }
    }

    /// 解析 `--gui` / `--tui` 或环境变量取值。
    pub fn parse(s: &str) -> Option<UiMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gui" => Some(UiMode::Gui),
            "tui" => Some(UiMode::Tui),
            _ => None,
        }
    }
}

/// 编译期可用性：本次构建是否链进了对应界面（由 cli 按 feature 填充）。
#[derive(Debug, Clone, Copy)]
pub struct UiAvailability {
    pub gui: bool,
    pub tui: bool,
}

impl UiAvailability {
    pub fn any(&self) -> bool {
        self.gui || self.tui
    }
}

/// 探测所需的环境事实。抽成结构体是为了让决策表**可单测**（无需真的去改环境变量）。
#[derive(Debug, Clone)]
pub struct EnvFacts {
    /// 环境变量覆盖：`REQ_GUARD_UI`（兼容旧名 `DEV_SCAFFOLD_UI`）。
    pub override_ui: Option<String>,
    pub is_windows: bool,
    pub is_macos: bool,
    /// 存在 `SSH_CONNECTION` / `SSH_TTY`。
    pub ssh: bool,
    /// 存在 `DISPLAY` / `WAYLAND_DISPLAY`。
    pub display: bool,
}

impl EnvFacts {
    /// 从当前进程环境采集。
    pub fn capture() -> EnvFacts {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        EnvFacts {
            override_ui: get("REQ_GUARD_UI").or_else(|| get("DEV_SCAFFOLD_UI")),
            is_windows: cfg!(windows),
            is_macos: cfg!(target_os = "macos"),
            ssh: get("SSH_CONNECTION").is_some() || get("SSH_TTY").is_some(),
            display: get("DISPLAY").is_some() || get("WAYLAND_DISPLAY").is_some(),
        }
    }
}

/// 按优先级短路决定界面形态。
///
/// | 序 | 条件 | 结论 |
/// | --- | --- | --- |
/// | 1 | 命令行显式 `--gui` / `--tui` | 采用（人工意图最高） |
/// | 2 | `REQ_GUARD_UI=gui\|tui` | 采用 |
/// | 3 | 该形态未编译进本次构建 | 退到另一个；都没编译则报错 |
/// | 4 | Windows / macOS | Gui |
/// | 5 | Linux 且有 SSH 会话 | Tui（转发图形界面不现实） |
/// | 6 | Linux 且有 DISPLAY / WAYLAND_DISPLAY | Gui |
/// | 7 | 其他（无 DISPLAY 的 tty） | Tui |
///
/// 注：文档原表把环境变量排在命令行参数之前；这里**有意调整为命令行优先**——
/// 显式参数应压过环境默认值，否则 `--gui` 会被一个陈旧的环境变量悄悄否决。
pub fn detect(forced: Option<UiMode>, avail: UiAvailability, env: &EnvFacts) -> Result<UiMode> {
    let wanted = forced.or_else(|| env.override_ui.as_deref().and_then(UiMode::parse));

    // 编译期裁剪优先于平台探测：没编进去的界面不能选。
    let want = match wanted {
        Some(UiMode::Gui) if avail.gui => UiMode::Gui,
        Some(UiMode::Tui) if avail.tui => UiMode::Tui,
        Some(UiMode::Gui) if avail.tui => UiMode::Tui,
        Some(UiMode::Tui) if avail.gui => UiMode::Gui,
        _ => {
            if !avail.any() {
                return Err(GateError::Validation(
                    "本次构建未包含任何界面，请用 --features tui（或 gui / full）重新构建".into(),
                ));
            }
            // 先按平台/会话得出"倾向形态"，再按编译期可用性裁剪。
            //
            // ★ 这里曾有一个真实缺陷：早期实现直接 `return UiMode::Gui` 而不看
            //   `avail.gui`，于是 **只编了 tui 的二进制在 Windows/macOS 上自动探测
            //   必然选中未编译的 GUI**，随后在 cli 侧报「本次构建未包含 GUI」而退出——
            //   表现就是官方发布的 req-guard-ui.exe（CLI+TUI 变体，本就不含 eframe）
            //   在 Windows 上执行 `req-guard-ui ui` 直接失败，必须手动加 `--tui`。
            //   平台倾向只是"偏好"，最终形态仍需过一遍编译期裁剪。
            let preferred = if env.is_windows || env.is_macos {
                UiMode::Gui
            } else if env.ssh {
                UiMode::Tui
            } else if env.display {
                UiMode::Gui
            } else {
                UiMode::Tui
            };
            match preferred {
                UiMode::Gui if avail.gui => UiMode::Gui,
                UiMode::Tui if avail.tui => UiMode::Tui,
                // 倾向的形态没编进来 → 退到另一个（avail.any() 已保证它可用）
                UiMode::Gui => UiMode::Tui,
                UiMode::Tui => UiMode::Gui,
            }
        }
    };

    // 说明：`want` 与用户诉求不一致时即为"回退到另一个形态"，
    // 调用方若需要提示（如"GUI 不可用，已切到 TUI"）可自行比对。
    Ok(want)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_linux() -> EnvFacts {
        EnvFacts {
            override_ui: None,
            is_windows: false,
            is_macos: false,
            ssh: false,
            display: false,
        }
    }

    fn both() -> UiAvailability {
        UiAvailability {
            gui: true,
            tui: true,
        }
    }

    #[test]
    fn 命令行优先于环境变量() {
        let mut env = env_linux();
        env.override_ui = Some("tui".into());
        assert_eq!(
            detect(Some(UiMode::Gui), both(), &env).unwrap(),
            UiMode::Gui
        );
    }

    #[test]
    fn 环境变量生效() {
        let mut env = env_linux();
        env.override_ui = Some(" TUI ".into());
        assert_eq!(detect(None, both(), &env).unwrap(), UiMode::Tui);
    }

    #[test]
    fn 未编译的形态回退另一个() {
        let env = env_linux();
        let only_tui = UiAvailability {
            gui: false,
            tui: true,
        };
        assert_eq!(
            detect(Some(UiMode::Gui), only_tui, &env).unwrap(),
            UiMode::Tui
        );
        let only_gui = UiAvailability {
            gui: true,
            tui: false,
        };
        assert_eq!(
            detect(Some(UiMode::Tui), only_gui, &env).unwrap(),
            UiMode::Gui
        );
    }

    #[test]
    fn 都没编译则报错() {
        let env = env_linux();
        let none = UiAvailability {
            gui: false,
            tui: false,
        };
        let e = detect(None, none, &env).unwrap_err();
        assert!(e.to_string().contains("--features"), "{}", e);
    }

    #[test]
    fn 平台倾向的形态没编进来时退到另一个() {
        // 真实缺陷回归：官方 req-guard-ui.exe 只编了 tui，在 Windows 上自动探测
        // 曾选中未编译的 GUI，导致 `ui`（不带 --tui）直接失败。
        let mut win = env_linux();
        win.is_windows = true;
        let only_tui = UiAvailability {
            gui: false,
            tui: true,
        };
        assert_eq!(
            detect(None, only_tui, &win).unwrap(),
            UiMode::Tui,
            "Windows 上 GUI 未编译时应回退 TUI"
        );

        let mut mac = env_linux();
        mac.is_macos = true;
        let only_gui = UiAvailability {
            gui: true,
            tui: false,
        };
        assert_eq!(
            detect(None, only_gui, &mac).unwrap(),
            UiMode::Gui,
            "macOS 上优先 GUI，且 GUI 已编译"
        );

        // Linux 有 DISPLAY 但只编了 tui → 同样回退
        let mut x11 = env_linux();
        x11.display = true;
        assert_eq!(detect(None, only_tui, &x11).unwrap(), UiMode::Tui);
    }

    #[test]
    fn 平台与ssh决策表() {
        let mut win = env_linux();
        win.is_windows = true;
        assert_eq!(detect(None, both(), &win).unwrap(), UiMode::Gui);

        let mut mac = env_linux();
        mac.is_macos = true;
        assert_eq!(detect(None, both(), &mac).unwrap(), UiMode::Gui);

        let mut ssh = env_linux();
        ssh.ssh = true;
        ssh.display = true; // 即便有 DISPLAY，SSH 会话仍优先 TUI
        assert_eq!(detect(None, both(), &ssh).unwrap(), UiMode::Tui);

        let mut x11 = env_linux();
        x11.display = true;
        assert_eq!(detect(None, both(), &x11).unwrap(), UiMode::Gui);

        let plain = env_linux();
        assert_eq!(detect(None, both(), &plain).unwrap(), UiMode::Tui);
    }

    #[test]
    fn parse_识别取值() {
        assert_eq!(UiMode::parse("GUI"), Some(UiMode::Gui));
        assert_eq!(UiMode::parse(" tui "), Some(UiMode::Tui));
        assert_eq!(UiMode::parse("web"), None);
    }
}
