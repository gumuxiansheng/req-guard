#!/bin/sh
# verify_sdd_routing.sh —— SDD 产物路由校验（REQ-021）
#
# 只读体检：不写文件、不改 .gitignore、不改清单。违规时只列出精确路径。
#
# 三组检查（语义见 docs/规范/SDD产物接入规范.md §7 与 REQ-021 设计 2）：
#   A 来源声明在位：每个被 git 跟踪的 specs/** 文件，前 15 行内必须含
#                   <!-- SDD-SOURCE: REQ-<3位数字> -->
#   B 指向的清单存在：声明里的 REQ-<id> 必须能解析到清单文件（归档区也算）
#   C codebound 契约：活跃清单 frontmatter 是否齐备 —— 只报告，不改变退出码
#
# 为什么用 POSIX sh 而不是 Python（REQ-021 设计 6）：CNB 的 rust:1-slim-bookworm
# 镜像没有 python3，沿用「探测+跳过」会让该侧完全不校验。这里把依赖降到 git + sh。
#
# 退出码：0 合规 / 1 违规
# 用法：  sh scripts/verify_sdd_routing.sh

set -u

if ! ROOT=$(git rev-parse --show-toplevel 2>/dev/null); then
    echo "✗ 不在 git 仓库内，无法判定 SDD 路由" >&2
    exit 1
fi
cd "$ROOT" || exit 1

REQ_DIR=".gates/requirements"
MARKER_MAX_LINE=15
EXIT=0

echo "SDD 产物路由校验（REQ-021）"
echo "────────────────────────────────────────"

# ── A / B 组：被 git 跟踪的 specs/** 文件 ────────────────────────────────
tracked=$(git ls-files -- specs/ 2>/dev/null)

total=0
no_decl=""
bad_pos=""
bad_target=""

if [ -z "$tracked" ]; then
    echo "A 组  版本库内无被跟踪的 specs/** 文件 → 合规"
else
    printf '%s\n' "$tracked" | while IFS= read -r f; do
        [ -n "$f" ] || continue
        [ -f "$f" ] || continue

        # 取前 MARKER_MAX_LINE 行内的声明（限定位置：防止把声明藏到文件末尾）
        head_line=$(head -n "$MARKER_MAX_LINE" "$f" 2>/dev/null |
            sed -n 's/.*SDD-SOURCE:[[:space:]]*\(REQ-[0-9][0-9][0-9]\).*/\1/p' | head -n 1)

        if [ -z "$head_line" ]; then
            # 区分「完全没有」与「写了但不在前 N 行」
            if grep -q 'SDD-SOURCE' "$f" 2>/dev/null; then
                printf 'POS %s\n' "$f"
            else
                printf 'NONE %s\n' "$f"
            fi
        else
            # B 组：该 REQ 是否解析得到清单（活跃 + 归档区）
            hit=$(ls "$REQ_DIR"/"$head_line"*.md 2>/dev/null | head -n 1)
            if [ -z "$hit" ]; then
                hit=$(find "$REQ_DIR/archive" -name "$head_line*.md" -type f 2>/dev/null | head -n 1)
            fi
            if [ -z "$hit" ]; then
                printf 'TARGET %s %s\n' "$f" "$head_line"
            fi
        fi
    done > /tmp/.sdd_routing_findings.$$
fi

if [ -f /tmp/.sdd_routing_findings.$$ ]; then
    no_decl=$(sed -n 's/^NONE //p' /tmp/.sdd_routing_findings.$$)
    bad_pos=$(sed -n 's/^POS //p' /tmp/.sdd_routing_findings.$$)
    bad_target=$(sed -n 's/^TARGET //p' /tmp/.sdd_routing_findings.$$)
    total=$(git ls-files -- specs/ 2>/dev/null | grep -c . 2>/dev/null)
    rm -f /tmp/.sdd_routing_findings.$$
fi

if [ "$total" -eq 0 ]; then
    echo "A 组  版本库内无被跟踪的 specs/** 文件 → 合规"
else
    echo "A 组  受检 specs/** 文件：$total 个"
fi

if [ -n "$no_decl" ]; then
    EXIT=1
    echo "  ✗ 缺来源声明（须在前 ${MARKER_MAX_LINE} 行内含 <!-- SDD-SOURCE: REQ-<id> -->）："
    printf '%s\n' "$no_decl" | sed 's/^/      /'
fi

if [ -n "$bad_pos" ]; then
    EXIT=1
    echo "  ✗ 有声明但不在前 ${MARKER_MAX_LINE} 行内（声明必须打开可见）："
    printf '%s\n' "$bad_pos" | sed 's/^/      /'
fi

if [ -n "$bad_target" ]; then
    EXIT=1
    echo "  ✗ 声明指向的清单不存在："
    printf '%s\n' "$bad_target" | sed 's/^/      /'
fi

if [ "$EXIT" -eq 0 ]; then
    echo "A/B 组  声明在位且指向的清单均存在 → 合规"
fi

# ── C 组：活跃清单的 codebound 契约（只报告，不改退出码）────────────────
echo "────────────────────────────────────────"
missing=""

for m in "$REQ_DIR"/*.md; do
    [ -f "$m" ] || continue
    # 归档区是只读历史，不参与统计（B-05）
    case "$m" in
        */archive/*) continue ;;
    esac
    # 评论文件不是清单，没有 frontmatter（它本就该被排除，否则会刷出一屏假缺失）
    case "$m" in
        *.comments.md) continue ;;
    esac

    # 抽取 frontmatter：首个 --- 与下一个 --- 之间
    fm=$(awk 'NR==1 && $0=="---" {inb=1; next} inb && $0=="---" {exit} inb {print}' "$m" 2>/dev/null)

    has_policy=0
    has_refs=0
    if [ -n "$fm" ]; then
        printf '%s\n' "$fm" | grep -q '^review_policy:[[:space:]]*[^[:space:]]' && has_policy=1
        # source_refs 存在 且 不是空数组 []
        if printf '%s\n' "$fm" | grep -q '^source_refs:[[:space:]]*[^[:space:]]'; then
            printf '%s\n' "$fm" | grep -q '^source_refs:[[:space:]]*\[[[:space:]]*\]' || has_refs=1
        fi
    fi

    if [ "$has_policy" -eq 0 ] || [ "$has_refs" -eq 0 ]; then
        why="缺 review_policy"
        [ "$has_refs" -eq 0 ] && why="$why、缺 source_refs"
        missing="${missing}      ${m}  （${why}）
"
    fi
done

if [ -n "$missing" ]; then
    echo "C 组  契约不齐备的活跃清单（**只报告，不改变退出码**）："
    printf '%s' "$missing"
    echo "      → doc-guard 装上后，此项即为 FRS004（源已改而规格未同步）的前置条件"
else
    echo "C 组  全部活跃清单的 review_policy / source_refs 齐备"
fi

# ── 结论 ────────────────────────────────────────────────────────────────
echo "────────────────────────────────────────"
if [ "$EXIT" -eq 0 ]; then
    echo "✅ SDD 路由合规"
else
    echo "⛔ SDD 路由违规：上述文件已入库但来源不可追溯"
    echo "   处理：补上声明，或把它们移出 specs/；本脚本不替你删文件"
fi

exit "$EXIT"
