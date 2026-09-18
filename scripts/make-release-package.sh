#!/usr/bin/env bash
# req-guard 发布包组装脚本
#
# 把「已编译二进制 + 文档 + 脚本 + 模板 + 校验信息」组装成一个可直接分发的发布包：
#
#   dist/req-guard-v<version>/
#   ├── README.md / VERSION / CHANGELOG.md / SHA256SUMS.txt / THIRD-PARTY-*
#   ├── bin/<平台-架构>/req-guard[.exe]、req-guard-ui[.exe]…
#   ├── docs/      安装指南 / 用户手册 / 校验说明 / 常见问题
#   ├── scripts/   install|uninstall|verify-checksums|selfcheck（.sh 与 .ps1 双份）
#   └── templates/ CI 接入样例
#
# 并额外产出按平台裁剪的归档（zip / tar.gz），每个归档内的 SHA256SUMS.txt 只覆盖自身内容。
#
# 用法：
#   bash scripts/make-release-package.sh              # 先构建再组装（默认）
#   bash scripts/make-release-package.sh --no-build   # 用已有 dist/raw/ 直接组装
#   bash scripts/make-release-package.sh --no-archive # 只出目录，不打包
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$(pwd)"

export PATH="$HOME/.cargo/bin:$PATH"

# ---- Python：优先隔离环境的受管版本 ----
PY=""
for cand in "${HOME}/.workbuddy/binaries/python/versions/3.13.12/python.exe" \
            "$(command -v python3 || true)" "$(command -v python || true)"; do
  [ -n "${cand}" ] && [ -x "${cand}" ] && PY="${cand}" && break
done
[ -n "${PY}" ] || { echo "找不到可用的 Python（替换占位符与打包需要）" >&2; exit 2; }
HELPER="${REPO_ROOT}/scripts/make_release_package.py"

# Git Bash 的 POSIX 路径（/c/...）Windows Python 不认，会拼成 C:\c\...；统一转成 C:\... 再传。
W() { if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else echo "$1"; fi; }

# ---- 参数 ----
DO_BUILD=1
DO_ARCHIVE=1
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build)   DO_BUILD=0;   shift ;;
    --no-archive) DO_ARCHIVE=0; shift ;;
    -h|--help)    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

# ---- 版本 ----
VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
[ -n "${VERSION}" ] || { echo "无法从 Cargo.toml 解析 version" >&2; exit 2; }
PKG_NAME="req-guard-v${VERSION}"
PKG_DIR="${REPO_ROOT}/dist/${PKG_NAME}"
BUILD_DATE="$(date '+%Y-%m-%d %H:%M:%S %z')"
GIT_COMMIT="$(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"
RUSTC_VER="$(rustc --version 2>/dev/null || echo 'unknown')"
CARGO_VER="$(cargo --version 2>/dev/null || echo 'unknown')"

echo "==> 组装 req-guard v${VERSION} 发布包"
echo "    提交      : ${GIT_COMMIT}"
echo "    构建时间  : ${BUILD_DATE}"
echo "    工具链    : ${RUSTC_VER}"

# ---- 1) 构建二进制 ----
if [ "${DO_BUILD}" -eq 1 ]; then
  echo
  echo "==> 1/7 构建各平台二进制（scripts/build-release-local.sh）"
  bash scripts/build-release-local.sh | sed 's/^/    /'
else
  echo
  echo "==> 1/7 跳过构建（--no-build），复用 dist/raw/"
fi

# ---- 2) 组装目录骨架 ----
echo
echo "==> 2/7 组装目录骨架"
# 上一版发布包目录若已存在，**改名归档**到 dist/.trash/ 而不是删除。
# 理由：发布包是构建产物，重复组装时"删了重建"与"留个旧快照"功能等价，但后者不碰删除操作——
# 既能在无人值守（CI）下稳定运行，也不会触发工作环境的批量删除保护。
# 旧快照无用途时可自行清理 dist/.trash/。
if [ -d "${PKG_DIR}" ]; then
  TRASH_DIR="${REPO_ROOT}/dist/.trash"
  mkdir -p "${TRASH_DIR}"
  mv "${PKG_DIR}" "${TRASH_DIR}/${PKG_NAME}-$(date '+%Y%m%d-%H%M%S')"
  echo "    （上一版已归档到 dist/.trash/）"
fi
mkdir -p "${PKG_DIR}"
cp -f packaging/README.md packaging/CHANGELOG.md packaging/THIRD-PARTY-NOTICES.md "${PKG_DIR}/"
cp -r packaging/docs      "${PKG_DIR}/docs"
cp -r packaging/scripts   "${PKG_DIR}/scripts"
cp -r packaging/templates "${PKG_DIR}/templates"
# GitHub Actions 样例以仓库 templates/ci 为唯一出处（避免 packaging 副本与仓库漂移）
cp -f templates/ci/req-guard-ci.yml "${PKG_DIR}/templates/ci/req-guard-ci.yml"

# 字体许可（GUI 变体内嵌 Noto Sans SC）
mkdir -p "${PKG_DIR}/licenses"
cp -f gui/assets/OFL-NOTO.txt "${PKG_DIR}/licenses/OFL-NOTO.txt"
echo "    文档/脚本/模板/许可已就位"

# ---- 3) 放入二进制（target 三元组 → 平台友好命名 + 执行位）----
echo
echo "==> 3/7 放入预编译二进制"
put() {  # put <源> <目标>；缺失即报错（发布包不允许静默缺件）
  local src="$1" dst="$2"
  if [ ! -f "${src}" ]; then
    echo "    ❌ 缺失产物：${src}" >&2
    MISSING=1
    return 0
  fi
  mkdir -p "$(dirname "${dst}")"
  cp -f "${src}" "${dst}"
  case "${dst}" in
    *.exe) : ;;                 # Windows 不需要执行位
    *)     chmod 0755 "${dst}" ;;   # Unix 必须给执行位，否则 git/直接调用都会静默失败
  esac
  printf '    %-42s %8s\n' "$(echo "${dst}" | sed "s|^${PKG_DIR}/||")" "$(du -h "${dst}" | cut -f1)"
}

MISSING=0
RAW="dist/raw"
put "${RAW}/cli/req-guard-cli-x86_64-pc-windows-msvc.exe"   "${PKG_DIR}/bin/windows-x86_64/req-guard.exe"
put "${RAW}/ui/req-guard-ui-x86_64-pc-windows-msvc.exe"     "${PKG_DIR}/bin/windows-x86_64/req-guard-ui.exe"
put "${RAW}/gui/req-guard-gui-x86_64-pc-windows-msvc.exe"   "${PKG_DIR}/bin/windows-x86_64/req-guard-gui.exe"
put "${RAW}/cli/req-guard-cli-x86_64-unknown-linux-musl"    "${PKG_DIR}/bin/linux-x86_64/req-guard"
put "${RAW}/ui/req-guard-ui-x86_64-unknown-linux-musl"      "${PKG_DIR}/bin/linux-x86_64/req-guard-ui"
put "${RAW}/cli/req-guard-cli-aarch64-unknown-linux-musl"   "${PKG_DIR}/bin/linux-aarch64/req-guard"
put "${RAW}/ui/req-guard-ui-aarch64-unknown-linux-musl"     "${PKG_DIR}/bin/linux-aarch64/req-guard-ui"
[ "${MISSING}" -eq 0 ] || { echo "产物不完整，请先跑 scripts/build-release-local.sh" >&2; exit 1; }

# 脚本执行位（发布包可能被打包/解压，不能指望继承）
chmod 0755 "${PKG_DIR}"/scripts/*.sh

# ---- 4) 生成 VERSION 与第三方声明 ----
echo
echo "==> 4/7 生成版本信息与第三方声明"
cat > "${PKG_DIR}/VERSION" <<EOF
# req-guard 发布包版本信息（机器可读：key = value）
version = "${VERSION}"
package = "${PKG_NAME}"
build_date = "${BUILD_DATE}"
git_commit = "${GIT_COMMIT}"
rustc = "${RUSTC_VER}"
cargo = "${CARGO_VER}"

# 产物清单：包内路径 → 构建 target / 特性
#   变体 cli = 默认特性（零 UI 依赖）；ui = --features tui；gui = --features gui
bin/windows-x86_64/req-guard.exe      target=x86_64-pc-windows-msvc     variant=cli  crt=static
bin/windows-x86_64/req-guard-ui.exe   target=x86_64-pc-windows-msvc     variant=ui   crt=static
bin/windows-x86_64/req-guard-gui.exe  target=x86_64-pc-windows-msvc     variant=gui  crt=static
bin/linux-x86_64/req-guard            target=x86_64-unknown-linux-musl  variant=cli  libc=musl-static
bin/linux-x86_64/req-guard-ui         target=x86_64-unknown-linux-musl  variant=ui   libc=musl-static
bin/linux-aarch64/req-guard           target=aarch64-unknown-linux-musl variant=cli  libc=musl-static
bin/linux-aarch64/req-guard-ui        target=aarch64-unknown-linux-musl variant=ui   libc=musl-static

# 校验
checksums = SHA256SUMS.txt
checksum_algorithm = sha256
signature = none
EOF
echo "    VERSION 已生成"

if cargo metadata --format-version 1 --offline > dist/_meta.json 2>/dev/null; then
  "${PY}" "$(W "${HELPER}")" deps \
    --meta "$(W "${REPO_ROOT}/dist/_meta.json")" \
    --out-md "$(W "${PKG_DIR}/THIRD-PARTY-NOTICES.md")" \
    --out-txt "$(W "${PKG_DIR}/THIRD-PARTY-LICENSES.txt")" | sed 's/^/    /'
else
  echo "    ⚠️  cargo metadata 不可用，保留包内既有依赖声明（请人工确认时效性）"
fi

# ---- 5) 占位符替换 + ps1 BOM ----
echo
echo "==> 5/7 替换文档占位符（版本/日期/提交/工具链）"
"${PY}" "$(W "${HELPER}")" subst --root "$(W "${PKG_DIR}")" \
  --define "{{VERSION}}=${VERSION}" \
           "{{BUILD_DATE}}=${BUILD_DATE}" \
           "{{GIT_COMMIT}}=${GIT_COMMIT}" \
           "{{RUSTC_VERSION}}=${RUSTC_VER}" \
  | sed 's/^/    /'

# ---- 6) 校验和 ----
echo
echo "==> 6/7 生成 SHA256SUMS.txt"
"${PY}" "$(W "${HELPER}")" sums --root "$(W "${PKG_DIR}")" --out "$(W "${PKG_DIR}/SHA256SUMS.txt")" | sed 's/^/    /'

# ---- 7) 打包 ----
echo
if [ "${DO_ARCHIVE}" -eq 1 ]; then
  echo "==> 7/7 生成分平台归档"
  "${PY}" "$(W "${HELPER}")" archive --root "$(W "${PKG_DIR}")" --out "$(W "${REPO_ROOT}/dist/${PKG_NAME}-windows-x86_64.zip")" \
      --format zip --keep-bin windows-x86_64 --regenerate-sums --trash-root "$(W "${REPO_ROOT}/dist/.trash")" | sed 's/^/    /'
  "${PY}" "$(W "${HELPER}")" archive --root "$(W "${PKG_DIR}")" --out "$(W "${REPO_ROOT}/dist/${PKG_NAME}-linux-x86_64-musl.tar.gz")" \
      --format targz --keep-bin linux-x86_64 --regenerate-sums --trash-root "$(W "${REPO_ROOT}/dist/.trash")" | sed 's/^/    /'
  "${PY}" "$(W "${HELPER}")" archive --root "$(W "${PKG_DIR}")" --out "$(W "${REPO_ROOT}/dist/${PKG_NAME}-linux-aarch64-musl.tar.gz")" \
      --format targz --keep-bin linux-aarch64 --regenerate-sums --trash-root "$(W "${REPO_ROOT}/dist/.trash")" | sed 's/^/    /'
  "${PY}" "$(W "${HELPER}")" archive --root "$(W "${PKG_DIR}")" --out "$(W "${REPO_ROOT}/dist/${PKG_NAME}-all.zip")" --trash-root "$(W "${REPO_ROOT}/dist/.trash")" \
      --format zip | sed 's/^/    /'
else
  echo "==> 7/7 跳过打包（--no-archive）"
fi

# ---- 收尾校验 ----
echo
echo "==> 收尾自检：包内校验和"
(cd "${PKG_DIR}" && bash scripts/verify-checksums.sh | tail -4) | sed 's/^/    /'

echo
echo "================ 发布包就绪 ================"
echo "目录   : dist/${PKG_NAME}"
echo "归档   :"
ls -1 dist/*.zip dist/*.tar.gz 2>/dev/null | sed 's/^/          /'
echo
echo "分发前建议：把 dist/${PKG_NAME} 与各归档一起发布；"
echo "用户侧校验：bash scripts/verify-checksums.sh （包内）"
