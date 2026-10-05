#!/usr/bin/env bash
# REQ-009 判决性实验：确认关键判据**真的被测试守着**。
#
# 纪律出处：《AI 工具合规保证规范》§8.2 ——「测试通过」与「测试有效」是两件
# 无法区分的事。本脚本把那条纪律变成命令：对每条关键判据注入一个「注入它就坏」
# 的变异，跑对应测试，**测试仍全绿就说明这条判据没人守**。
#
# 用法：
#   bash scripts/verify_tests_have_teeth.sh              # 全量清单
#   bash scripts/verify_tests_have_teeth.sh --only 3     # 只跑第 3 条
#   bash scripts/verify_tests_have_teeth.sh --only 围栏   # 只跑理由含「围栏」的条目
#
# 清单格式（scripts/mutation-manifest.txt，每行一条，5 段以单个 TAB 分隔）：
#   <文件>\t<锚点原文>\t<追加文本>\t<测试过滤串>\t<理由>
#
# 三态判定（任一非 killed 都让退出码非 0）：
#   killed        目标测试跑起来了且有失败 → 守住了
#   survived      目标测试跑起来了且全绿   → 这条判据没人守
#   build_failed  编译失败                 → **不算守住**（见下方「为什么」）
#
# 为什么编译失败不算守住：`survived` 的教训是「没人守」，而编译失败连这个结论都
# 得不到 —— 它只说明「代码和这个文件有耦合」。若算成守住，一份让 crate 编译不过的
# 清单会**全绿通过**，检查等于不存在。
#
# 本脚本会**临时改工作区文件**。三道保证：注入前检查目标文件是否被 git 跟踪且干净、
# trap 还原、还原后校验内容与注入前一致。任何一道失效都以非 0 退出码报出。
set -u

# 不开 `set -e`：本脚本要捕获 cargo / git / python 的退出码做判定，
# 而 `set -e` 会在第一次非 0 时直接退出，把「哪一条变异坏了」这个信息丢掉。

MANIFEST_REL="scripts/mutation-manifest.txt"
ONLY=""

die() {
    # 单一出口：任何问题都以退出码 1 结束，且**不打印**含 `survived` 的汇总
    # （AC-003 要求「锚点未命中」的输出里不含 `survived` —— 清单腐化与
    # 判据没人守是两种失效，措辞混同会让整个检查被习惯性忽略）。
    printf '%s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
用法: verify_tests_have_teeth.sh [--only <行号|理由关键字>]

  --only <行号|理由关键字>   只跑匹配的条目（行号从 1 起，或理由子串）
  -h, --help                显示本帮助

清单默认在 scripts/mutation-manifest.txt。
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --only)
            [ $# -ge 2 ] || die "--only 需要一个参数（行号或理由关键字）"
            ONLY="$2"
            shift 2
            ;;
        --only=*)
            ONLY="${1#--only=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "未知参数：$1"
            ;;
    esac
done

# ── 前置：必须在 git 仓库内 ──
# 还原手段是 `git checkout --`，不在仓库里就无处还原（T4 的前提）。
git rev-parse --show-toplevel >/dev/null 2>&1 \
    || die "不在 git 仓库内，拒绝运行：本脚本靠 \`git checkout --\` 还原工作区，无仓库则无处还原。"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT" || die "无法进入仓库根目录：$ROOT"

[ -f "$MANIFEST_REL" ] || die "找不到变异清单：$MANIFEST_REL"
command -v python3 >/dev/null 2>&1 || die "未找到 python3 —— 注入器依赖它（见 REQ-009 §2 关键设计 3：不用 sed 是因为 & 反向引用会把源码改成别的东西）"

TMPDIR_RUN="$(mktemp -d "${TMPDIR:-/tmp}/verify-teeth.XXXXXX")" \
    || die "无法创建临时目录"

# ── 注入器：从 heredoc 落一份到临时目录 ──
# 为什么折进来而不是留一个 .py 文件：变更范围契约（GATE:TOUCH）里声明的是本
# `.sh` —— 让实现去开第二个文件，就得把那份文件也声明进去，而声明扩张要走
# 人工审批。一份文件 = 一处真相，且少一个「忘了同步」的机会。
# 落临时目录而不是 eval/管道：注入器要能带非 0 退出码被逐条判定（锚点未命中 3 /
# 歧义 4 / 自证落地失败 5），走管道会把退出码吃掉。
INJECTOR="$TMPDIR_RUN/injector.py"
cat >"$INJECTOR" <<'PYEOF_INJECTOR'
#!/usr/bin/env python3
"""REQ-009 判决性实验的注入器（G3 自证落地 + 三态判定的证据采集）。

**为什么单独一个 .py**：注入要改 Rust 源码，而 sed 的替换文本里 `&` 是「整个
匹配」的反向引用 —— `&&` 在 Rust 里满地都是，一条含 `&&` 的替换会被 sed 展开成
匹配内容，**注入出语法不同的代码并静默跑测试**（REQ-009 §2 关键设计 3）。
python3 逐行 `str.replace` 是字面量，无此问题。

**它不判定测试是否通过**（REQ-009 N4）：那是 `cargo test` 的职责。本模块只做
「注入 → 自证落地 → 还原」三件事，观测由调用方拿退出码完成。

用法（由 verify_tests_have_teeth.sh 调用，不供人直接跑）：

    inject --file <路径> --anchor <锚点原文> --replacement <替换文本>
        注入并自证落地。成功时向 stdout 打一行机器可读的结果：
            injected<TAB><行号>
        失败时退出码非 0，stderr 给可执行的中文原因。

    extract --file <路径> --anchor <锚点原文>
        只做锚点定位（不修改文件），stdout 打 `<行号>`；未命中退出码 3，
        歧义退出码 4。供调用方在**注入前**先判歧义，避免改了一半才发现。

    readfile --file <路径>
        原样输出文件内容（调用方据此比对还原后是否逐字一致）。

`--replacement` 是**追加到锚点所在行行尾**的文本（REQ-009 §2 关键设计 2），
不是整行替换：行数不变，`mask_fenced` 的调用方依赖行号。
"""
from __future__ import annotations

import argparse
import sys

# 退出码。0 = 成功；其余供调用方区分失败形态（不同形态要报不同文案，
# 「清单腐化」与「判据没人守」混成一句话会让整个检查被习惯性忽略）。
EXIT_OK = 0
EXIT_USAGE = 2
EXIT_ANCHOR_MISS = 3
EXIT_ANCHOR_AMBIGUOUS = 4
EXIT_SELF_VERIFY = 5
EXIT_IO = 6


def die(code: int, msg: str) -> "None":
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def read_lines(path: str) -> list[str]:
    """按 `splitlines` 读入，保留行结构但不吞末尾换行。

    还原一致性比对要求**逐字相同**，所以这里不做任何换行规范化；
    写入时统一以 `\\n` 连接并补一个末尾换行（Rust 源文件都是这个形态，
    且 `str.replace` 只在整行文本上操作，不依赖原始行尾形态）。
    """
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.read().splitlines()
    except OSError as e:
        die(EXIT_IO, f"读不到目标文件 {path}：{e}")


def write_lines(path: str, lines: list[str]) -> None:
    try:
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
    except OSError as e:
        die(EXIT_IO, f"写目标文件 {path} 失败：{e}")


def locate(lines: list[str], anchor: str) -> int:
    """锚点定位。命中 0 处 → 退出码 3；≥2 处 → 退出码 4。

    **不猜要改哪一处**：多命中时任选一处就是在改一个没人审过的位置，
    而本脚本的全部价值建立在「改的地方是审过的那一处」上。
    """
    hits = [i for i, line in enumerate(lines) if anchor in line]
    if not hits:
        die(
            EXIT_ANCHOR_MISS,
            f"锚点未命中：目标文件里找不到 {anchor!r}\n"
            f"          这通常是**清单腐化**（代码演进了、清单没跟上），"
            f"不是「判据没人守」。请更新 scripts/mutation-manifest.txt 的锚点。",
        )
    if len(hits) > 1:
        where = "、".join(f"第 {i + 1} 行" for i in hits[:5])
        die(
            EXIT_ANCHOR_AMBIGUOUS,
            f"锚点有歧义：{anchor!r} 在目标文件里出现 {len(hits)} 次（{where}）\n"
            f"          不猜要改哪一处。请把锚点写得更精确（多带几个字符）。",
        )
    return hits[0]


def cmd_extract(args: argparse.Namespace) -> None:
    lines = read_lines(args.file)
    print(locate(lines, args.anchor) + 1)


def cmd_inject(args: argparse.Namespace) -> None:
    lines = read_lines(args.file)
    idx = locate(lines, args.anchor)
    # 注入前的快照：自证落地要比对的是「磁盘 vs 注入前」，
    # 不是「磁盘 vs 我打算写成什么」—— 后者恒等于真，比对不出任何东西。
    original = list(lines)

    before = lines[idx]
    # 追加而非替换整行：行数不变，`mask_fenced` 的调用方依赖行号
    # （见 docs/规范/AI工具合规保证规范.md 的「掩码而非删除」）。
    after = f"{before} {args.replacement}"
    lines[idx] = after
    write_lines(args.file, lines)

    # ── 自证落地（G3）──
    # 「恰好一行被改，且那行等于预期值」：写完立刻读回比对。
    # 未生效的变异 → 测试当然全绿 → 被判成「判据没人守」→ **误报**，
    # 而误报会让整个检查被习惯性忽略。
    check = read_lines(args.file)
    if len(check) != len(original):
        die(
            EXIT_SELF_VERIFY,
            f"自证落地失败：注入后行数从 {len(original)} 变成 {len(check)}，"
            f"预期不变（{args.file}）",
        )
    diff = [i for i, (a, b) in enumerate(zip(check, original)) if a != b]
    if diff != [idx]:
        die(
            EXIT_SELF_VERIFY,
            f"自证落地失败：预期只有第 {idx + 1} 行被改，"
            f"实际差异行 = {[i + 1 for i in diff] or '无'}（{args.file}）",
        )
    if check[idx] != after:
        die(
            EXIT_SELF_VERIFY,
            f"自证落地失败：第 {idx + 1} 行读回为 {check[idx]!r}，"
            f"预期 {after!r}（{args.file}）",
        )

    print(f"injected\t{idx + 1}")


def cmd_readfile(args: argparse.Namespace) -> None:
    """原样输出，供调用方比对还原后是否与注入前逐字一致。"""
    try:
        with open(args.file, "r", encoding="utf-8") as f:
            sys.stdout.write(f.read())
    except OSError as e:
        die(EXIT_IO, f"读不到目标文件 {args.file}：{e}")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(add_help=True)
    sub = ap.add_subparsers(dest="cmd", required=True)

    for name in ("inject", "extract"):
        p = sub.add_parser(name)
        p.add_argument("--file", required=True)
        p.add_argument("--anchor", required=True)
        if name == "inject":
            p.add_argument("--replacement", required=True)

    p = sub.add_parser("readfile")
    p.add_argument("--file", required=True)

    args = ap.parse_args(argv)
    {"inject": cmd_inject, "extract": cmd_extract, "readfile": cmd_readfile}[args.cmd](args)
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
PYEOF_INJECTOR
[ -s "$INJECTOR" ] || die "注入器落盘失败（heredoc 为空）：$INJECTOR"

# 正在被变异、尚未还原的文件（空 = 当前没有未还原的改动）。
# 中断时 trap 靠它还原 —— 「卡住不退出会把工作区留在坏状态」是本脚本最坏的失效。
MUTATED=""

# ── 还原 + 清理（任何退出路径都跑）──
cleanup() {
    local rc=$?
    trap - EXIT INT TERM
    if [ -n "$MUTATED" ]; then
        restore_one "$MUTATED" || rc=1
    fi
    [ -d "$TMPDIR_RUN" ] && rm -rf "$TMPDIR_RUN"
    exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# 还原一个文件并**校验**它回到了注入前的样子。
# 校验失败不是「尽力而为」的问题：它意味着工作区已经被留在坏状态，
# 那比不做这个检查更糟 —— 必须以非 0 退出码明确告知。
restore_one() {
    local f="$1"
    if ! git checkout -- "$f" 2>/dev/null; then
        printf '⛔ 还原失败：git checkout -- %s 失败。请手工检查该文件，并考虑 `git stash list` / `git fsck`。\n' \
            "$f" >&2
        MUTATED=""
        return 1
    fi
    if [ -n "$(git status --porcelain -- "$f")" ]; then
        printf '⛔ 还原校验失败：%s 还原后仍与 HEAD 不同。工作区已被留在坏状态。\n' "$f" >&2
        printf '   请立即执行：git diff -- %s   查看残留改动，必要时 git checkout -- %s\n' "$f" "$f" >&2
        MUTATED=""
        return 1
    fi
    MUTATED=""
    return 0
}

# ── 1. 读清单 ──
# 空清单必须明确报「没有变异条目」，不能退化成静默通过（AC-009）：
# 一个从不报错的检查和一个坏掉的检查，在使用者的感知里没有区别。
ENTRIES="$TMPDIR_RUN/entries.tsv"
: >"$ENTRIES"
ENTRY_LNO=0
VALID=0

while IFS= read -r line || [ -n "$line" ]; do
    ENTRY_LNO=$((ENTRY_LNO + 1))
    case "$line" in
        \#*) continue ;;                 # 整行注释
    esac
    if [ -z "$(printf '%s' "$line" | tr -d '[:space:]')" ]; then
        continue                        # 空行
    fi
    # 5 段 = 恰好 4 个 TAB。多一个 TAB 说明某段里混进了 TAB 字符
    # （本格式无转义，段内出现分隔符就是格式非法）—— 报「清单格式非法」
    # 而不是把它当第 6 段静默忽略。
    tabs="$(printf '%s' "$line" | tr -cd '\t' | wc -c | tr -d ' ')"
    if [ "$tabs" != "4" ]; then
        die "清单格式非法：第 $ENTRY_LNO 行有 $tabs 个 TAB（分隔符），预期 4 个（5 段）。
     本清单以单个 TAB 分隔且**无转义**：某段里出现 TAB 即视为格式错误。
     若锚点或替换文本本身含 TAB，请换用不含 TAB 的锚点。"
    fi
    f="$(printf '%s' "$line" | cut -f1)"
    anchor="$(printf '%s' "$line" | cut -f2)"
    repl="$(printf '%s' "$line" | cut -f3)"
    filt="$(printf '%s' "$line" | cut -f4)"
    reason="$(printf '%s' "$line" | cut -f5-)"

    if [ -z "$f" ] || [ -z "$anchor" ] || [ -z "$repl" ] || [ -z "$filt" ]; then
        die "清单格式非法：第 $ENTRY_LNO 行有空段（文件/锚点/追加文本/测试过滤串均必填）。
     该行内容：$line"
    fi
    # 理由必填（AC-006）：没有理由的变异条目会被后来人当成噪音删掉，
    # 而它可能是整份清单里最重要的一条。
    if [ -z "$reason" ]; then
        die "理由必填：第 $ENTRY_LNO 行（${f}）的理由字段是空的。
     理由是这份清单里最容易随时间丢失的东西 —— 丢了就没人知道「为什么验这个」。"
    fi
    # 变异无效（AC-008）：替换文本与锚点原文相同 → 注入后文件没实质变化，
    # 测试必然全绿，会被误判成「判据没人守」。
    if [ "$repl" = "$anchor" ]; then
        die "变异无效：第 $ENTRY_LNO 行的追加文本与锚点原文完全相同，注入后行为不变。
     这样的条目必然让测试全绿，会被误判成「判据没人守」。"
    fi
    if [ "$ONLY" != "" ] && [ "$ONLY" != "$ENTRY_LNO" ] && [ "${reason#*$ONLY}" = "$reason" ]; then
        continue                        # --only 不匹配
    fi
    VALID=$((VALID + 1))
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$ENTRY_LNO" "$f" "$anchor" "$repl" "$filt" "$reason" >>"$ENTRIES"
done <"$MANIFEST_REL"

if [ "$VALID" -eq 0 ]; then
    die "没有变异条目：$MANIFEST_REL 里没有任何可执行的条目（文件存在但全被过滤或只有注释）。
     一个从不报错的检查和一个坏掉的检查，在使用者的感知里没有区别 —— 故这里明确失败。"
fi

# ── 2. 注入前检查：跟踪状态 / 干净状态 / 锚点定位 ──
# 全在跑 cargo **之前**做完：这些失败都不该浪费一次编译，
# 且 AC-003 / AC-007 要求它们发生时目标文件未被修改、cargo 未被调用。
while IFS=$'\t' read -r ln f anchor repl filt reason; do
    [ -n "$ln" ] || continue
    if [ ! -f "$f" ]; then
        die "第 $ln 条的目标文件不存在：$f"
    fi
    # 未跟踪文件（AC-012）：`git checkout --` 对它无效，还原会静默失败，
    # 把工作区留在坏状态 —— 那比拒绝运行糟得多。
    if ! git ls-files --error-unmatch -- "$f" >/dev/null 2>&1; then
        die "目标文件未跟踪：第 $ln 条的 $f 不在 git 索引里。
     还原手段是 \`git checkout -- <file>\`，它对未跟踪文件无效 —— 注入后无法还原。
     请先把文件提交，或把它移出变异清单。"
    fi
    # 脏检查**只针对本次要注入的文件**（AC-015）：开发者随时在工作区里改别的东西，
    # 若要求整个仓库干净，这个脚本在日常开发里几乎跑不了 —— 那等于只在 CI 存在。
    if [ -n "$(git status --porcelain -- "$f")" ]; then
        die "工作区不干净：第 $ln 条的目标文件 $f 有未提交修改。
     此时 \`git checkout -- $f\` 会**丢掉你的改动**，所以拒绝运行。
     请先提交或 stash 该文件的改动。（本检查只针对目标文件，别处的改动不影响运行。）"
    fi
    # 锚点定位（extract 不修改文件）：未命中 = 清单腐化，≥2 处 = 歧义。
    # 两种都用**不同措辞**报出，因为修法不同。
    if ! python3 "$INJECTOR" extract --file "$f" --anchor "$anchor" >/dev/null; then
        die "第 $ln 条（${f}）的锚点有问题，详见上方说明。
     理由：$reason"
    fi
done <"$ENTRIES"

TOTAL="$VALID"

# ── 3. 基线：先确认每条变异的过滤串在**未注入**时就是绿的 ──
# 不绿就注入 = 拿一个已知红的基线去证明「变异杀死了它」，那种「杀死」毫无意义。
printf '基线检查（%d 条）…\n' "$TOTAL"
BASELINE_FAIL=0
while IFS=$'\t' read -r ln f anchor repl filt reason; do
    [ -n "$ln" ] || continue
    printf '  [基线] 第 %s 条  过滤串=%s\n' "$ln" "$filt"
    # **过滤串必须真的匹配到测试**。`cargo test <串>` 在 0 匹配时退出码仍是 0 ——
    # 于是「过滤串写错/测试被改名」会同时骗过基线与变异判定，报出 `survived`
    # （「这条判据没人守」）。那是**误报**，而误报会让整个检查被习惯性忽略。
    # 用 `--list` 数出匹配数：0 即拒绝，不进入注入流程。
    NMATCH="$(cargo test --quiet -- --list -- "$filt" 2>/dev/null | grep -c ': test$' || true)"
    if [ "${NMATCH:-0}" -eq 0 ]; then
        BASELINE_FAIL=1
        printf '⛔ 过滤串未匹配到任何测试：第 %s 条的 %s 在本仓库里数不出测试（cargo test -- --list 匹配 0 个）。\n' \
            "$ln" "$filt" >&2
        printf '   锚点：%s\n' "$anchor" >&2
        printf '   理由：%s\n' "$reason" >&2
        printf '   典型原因：过滤串随测试改名而失效（**清单腐化**，不是「判据没人守」）。\n' >&2
        printf '   请用 `cargo test -- --list -- <串>` 核对后更新清单。\n' >&2
        continue
    fi
    printf '         匹配到 %s 个测试\n' "$NMATCH"
    if ! cargo test --quiet -- "$filt" >"$TMPDIR_RUN/baseline.$ln.log" 2>&1; then
        BASELINE_FAIL=1
        printf '⛔ 基线不绿：第 %s 条的测试过滤串 %s 在**未注入**状态下就失败。\n' "$ln" "$filt" >&2
        printf '   锚点：%s\n' "$anchor" >&2
        printf '   理由：%s\n' "$reason" >&2
        printf '   这条基线本身就是红的，注入后的红证明不了任何事 —— 请先修好它。\n' >&2
        printf '   日志：%s\n' "$TMPDIR_RUN/baseline.$ln.log" >&2
    fi
done <"$ENTRIES"
if [ "$BASELINE_FAIL" -ne 0 ]; then
    die "基线检查未通过，中止：未进入注入流程，目标文件未被修改。"
fi

# ── 4. 逐条注入 → 跑测试 → 三态判定 → 还原 ──
KILLED=0
SURVIVED=0
BUILD_FAILED=0
IDX=0

while IFS=$'\t' read -r ln f anchor repl filt reason; do
    [ -n "$ln" ] || continue
    IDX=$((IDX + 1))
    printf '\n[%d/%d] 第 %s 条  %s\n' "$IDX" "$TOTAL" "$ln" "$f"
    printf '      理由：%s\n' "$reason"

    # 注入前把原始内容存一份：还原后逐字比对（AC-004 / T4）。
    ORIG="$TMPDIR_RUN/orig.$ln"
    python3 "$INJECTOR" readfile --file "$f" >"$ORIG" || die "第 $ln 条：读原始内容失败"

    # 注入 + 自证落地。未生效就报错退出，绝不继续跑测试
    # （未生效的变异 → 测试当然全绿 → 被判成「没人守」→ 误报）。
    if ! python3 "$INJECTOR" inject --file "$f" --anchor "$anchor" --replacement "$repl"; then
        MUTATED="$f"
        restore_one "$f" || true
        printf '⛔ 自证落地失败：第 %s 条的变异没能确认写进 %s。\n' "$ln" "$f" >&2
        printf '   已还原目标文件；未执行 cargo test（跑它只会得到一个假的「没人守」）。\n' >&2
        exit 1
    fi
    MUTATED="$f"

    # 三态判定。分开记录两次退出码：编译没通过 ≠ 测试通过了。
    if ! cargo test --quiet --no-run -- "$filt" >"$TMPDIR_RUN/build.$ln.log" 2>&1; then
        BUILD_FAILED=$((BUILD_FAILED + 1))
        printf '      结果：build_failed（编译失败 —— **不计为守住**）\n'
        printf '      日志：%s\n' "$TMPDIR_RUN/build.$ln.log" >&2
        restore_one "$f" || die "第 $ln 条还原失败，工作区已被留在坏状态"
        continue
    fi

    if cargo test --quiet -- "$filt" >"$TMPDIR_RUN/run.$ln.log" 2>&1; then
        SURVIVED=$((SURVIVED + 1))
        printf '      结果：survived ⚠️  **这条判据没有任何测试守着**\n'
        printf '      过滤串=%s\n' "$filt"
        restore_one "$f" || die "第 $ln 条还原失败，工作区已被留在坏状态"
        continue
    fi

    KILLED=$((KILLED + 1))
    # `--quiet` 下每个变红的测试打一行 `<模块路径>::<测试名> --- FAILED`。
    # 计数用同一形态逐行统计；`|| true` 是必要的：grep -c 匹配不到时已输出 0
    # 且退出码为 1，不吞掉退出码就会在 `set -u` 下被当成命令失败。
    # 汇总那行 `test result: FAILED. …` 不计入（它不是某个测试的名字）。
    FAILED_LINE=' --- FAILED$'
    NFAIL="$(grep -cE "$FAILED_LINE" "$TMPDIR_RUN/run.$ln.log" || true)"
    printf '      结果：killed（守住了，%s 个测试变红）\n' "$NFAIL"
    if [ "$NFAIL" -gt 0 ]; then
        printf '      变红的测试：\n'
        grep -E "$FAILED_LINE" "$TMPDIR_RUN/run.$ln.log" | sed 's/^/        /'
    fi

    # 还原 + 逐字校验（T4 的硬约束）。
    restore_one "$f" || die "第 $ln 条还原失败，工作区已被留在坏状态"
    python3 "$INJECTOR" readfile --file "$f" >"$TMPDIR_RUN/after.$ln" || true
    if ! cmp -s "$ORIG" "$TMPDIR_RUN/after.$ln"; then
        die "还原校验失败：第 $ln 条还原后 %s 与注入前**不逐字相同**。
     工作区已被留在坏状态，请立即手工核对。（比对：%s）" "$f" "$f"
    fi
done <"$ENTRIES"

# ── 5. 汇总 ──
printf '\n────────────────────────────────────────\n'
printf '变异条目：%d   killed=%d   survived=%d   build_failed=%d\n' \
    "$TOTAL" "$KILLED" "$SURVIVED" "$BUILD_FAILED"

if [ "$SURVIVED" -gt 0 ]; then
    printf '\n⛔ 有 %d 条判据没有任何测试守着（survived）。\n' "$SURVIVED"
    printf '   失效方向最坏的那类正是「门禁看着在拦、实际没拦」。\n'
    printf '   处置：补测试，或确认该判据已不适用后把条目移出清单。\n'
fi
if [ "$BUILD_FAILED" -gt 0 ]; then
    printf '\n⛔ 有 %d 条变异让 crate 编译不过（build_failed）—— **编译失败不计为守住**。\n' "$BUILD_FAILED"
    printf '   这类变异只证明「代码和该文件有耦合」，没验到判据本身。\n'
    printf '   请改用能编译的变异（如短路成 return），或确认该判据已不适用。\n'
fi

if [ "$SURVIVED" -gt 0 ] || [ "$BUILD_FAILED" -gt 0 ]; then
    exit 1
fi
printf '\n✅ 全部 %d 条判据都有测试守着。\n' "$TOTAL"
exit 0