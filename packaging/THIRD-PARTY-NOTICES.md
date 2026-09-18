# 第三方组件声明

> 本文件随 req-guard v{{VERSION}} 发布包分发。
> 许可信息由构建脚本从 `cargo metadata` 自动生成，反映**构建时**的依赖快照。

---

## 1. req-guard 自身

req-guard 的源码许可以**源码仓库的 `LICENSE` 文件为准**（本发布包不重复附带）。
源码地址与构建方式见 [`README.md`](README.md) §8。

---

## 2. 内嵌字体（仅 GUI 变体）

GUI 变体（`req-guard-gui.exe`）内嵌 **Noto Sans SC** 子集，用于中文界面渲染。

| 项 | 值 |
| --- | --- |
| 字体 | Noto Sans SC（子集化，约 1.5 MB，仅覆盖界面用字） |
| 许可 | SIL Open Font License 1.1 |
| 许可全文 | [`licenses/OFL-NOTO.txt`](licenses/OFL-NOTO.txt) |
| 生成方式 | `scripts/make_font_subset.py`（源码仓库） |

要点（非完整条款，以全文为准）：允许使用、研究、修改与再分发；
**禁止单独出售字体文件**；衍生字体须同样以 OFL 授权，且不得使用保留字体名。

---

## 3. Rust 第三方依赖

req-guard 的二进制静态链接了若干 Rust crate。按变体划分：

| 变体 | 直接依赖 | 说明 |
| --- | --- | --- |
| CLI | 仅 `req-guard-core` | **零第三方依赖**（纯 Rust 标准库） |
| TUI | + `req-guard-tui`（ratatui / crossterm 等） | 终端界面 |
| GUI | + `req-guard-gui`（eframe / egui / wgpu 等） | 桌面界面，依赖最多 |

构建时解析到的第三方 crate 总数：**{{DEP_TOTAL}}**

按许可证分布：

{{DEP_SUMMARY}}

> 完整逐包清单（名称 / 版本 / 许可证）见 **`THIRD-PARTY-LICENSES.txt`**。
> 上述均为 permissive 许可（MIT / Apache-2.0 / BSD / Zlib / ISC / Unlicense / WTFPL 及其组合），
> 无 GPL/AGPL 等强 copyleft 组件。

---

## 4. 运行时系统依赖

| 平台 | 依赖 |
| --- | --- |
| Windows | 仅操作系统自带 DLL（CRT 已**静态链接**，无需 VC++ 运行库） |
| Linux（musl 静态） | 无（不依赖 glibc，可在 scratch 容器中运行） |
| Linux（GUI 源码构建） | X11/Wayland + GTK 系统库（**本包未提供 GUI Linux 版**） |

---

## 5. 重新生成本文件

```bash
bash scripts/make-release-package.sh --no-build    # 会重新采集 cargo metadata
```

若构建环境不可用（无 cargo / 离线），发布包内保留上一版采集结果，并在本节注明采集时间：
{{DEP_GENERATED_AT}}。
