# AI 需求门禁（req-guard）

> **规则**：AI 在本项目实现新需求前，必须先按步骤完成
> **需求分解 → 技术方案 → 测试计划** 三段清单，且每段由审核人显式批准，
> 否则**硬拦截**：AI 工具无法写入/修改任何文件。

## 工作原理

| 层 | 拦截点 | 说明 |
| --- | --- | --- |
| L1 | AI 工具 `PreToolUse`（Write/Edit） | 最硬：AI 根本写不了文件 |
| L2 | `git pre-commit` | 兜底：未过审不得提交 |
| L3 | CI 调用 `req-guard check` | 兜底：未过审不得合并 |

拦截脚本：`.gates/hooks/req-guard-check.sh`（POSIX）与 `.ps1`（Windows）。
它只解析清单里的 `GATE` 标记行——**正文随便改，GATE 行只能由 `req-guard` 命令改写**。

## 标准流程

```bash
# 1. 创建需求清单（AI 或人执行）
req-guard create -t "用户登录改造"

# 2. AI 填写三段正文（编辑生成的 .gates/requirements/REQ-001-*.md）

# 3. 审核人逐段批准（注意顺序：分解 → 方案 → 测试计划）
req-guard approve REQ-001 --step decomposition --reviewer 张三
req-guard approve REQ-001 --step solution      --reviewer 张三
req-guard approve REQ-001 --step testplan      --reviewer 张三

# 4. 查看状态；三步全 approved 才"已解锁"
req-guard status REQ-001

# 5. 解锁后 AI 才可编写代码

# 6. 需求完成（或中止）后归档——门禁随之跳过该清单
#    done 满 archive.after_days 天（默认 30）后，下次 done 时自动物理归档；
#    手动补扫：req-guard archive --author <姓名>（--dry-run 先预览）
req-guard done REQ-001 --author 张三

# （可选）查看归档历史
req-guard status --archived
```

打回：`req-guard reject REQ-001 --step solution --reviewer 张三 --reason "缺少回滚方案"`

## 到期归档（done 满 N 天后自动搬入 archive/）

需求文档会越积越多，全部平铺在 `.gates/requirements/` 会让 `status`/`list` 越来越长。
`req-guard done` 成功后，系统自动扫描 done 满 `archive.after_days` 天（默认 30，
可在 `.gates/req-guard.yaml` 的 `archive.after_days` 调整）的清单，**成对搬移**
（清单 + 评论）到 `.gates/requirements/archive/<创建年份>/`（无日期段 → `misc/`）：

- 归档区是**只读历史**：拦截脚本、`list`、自动编号都不再看它，但 `status`/`comments`
  仍可按 id 查到（`req-guard status REQ-001`、`req-guard status --archived`）；
- 归档**不复号**：`create` 自动编号会跳过归档区已用过的 `REQ-NNN`；
- AI 不得修改归档区文件（原样保留历史证据）。

## 审批锁：approve / resolve / done / bypass 须人类执行

`--reviewer` / `--author` 只是名字，不构成身份保证。因此 req-guard 给支持会话环境
注入的 AI 工具（如 Claude Code）写入 `"REQ_GUARD_AI_CTX": "1"`，`approve / reject /
resolve / done / bypass` 检测到该标记即**拒绝执行**——AI 经 Shell 自批会被堵在命令层。

- 审核人请在**自己的终端**（AI 会话之外）执行审批命令；
- 人类误中拦截时：在不带该变量的终端重试，或先 `unset REQ_GUARD_AI_CTX`；
- 该标记可被 `env -u` 剥离，属提高门槛而非强保证；生产环境请升级
  reviewer token / 带外审批（见《AI工具合规保证规范.md》§4.4）。

## 应急绕过（有痕、有时效）

```bash
req-guard bypass --reason "线上故障热修，事后补审" --ttl 60
```

绕过窗口内放行，但**每次都写审计日志** `.gates/audit/gate-audit.log`。
`.gates/.bypass` 已被 `.gitignore` 忽略，不会入库。

**人肉开发场景**：typo 修正、文档笔误、线上热修、临时试改等不值得建 REQ 的改动，
可直接 `bypass --reason <原因> --ttl <分钟>`（默认 60 分钟，到期自动失效）。
但 bypass 是**应急阀不是常规通道**——功能与架构改动必须先建 REQ 走三段审核；
事后请补建需求并 `req-guard done <REQ-ID> --author <姓名>` 归档，把账还上。

## L3 CI 强制门禁（部署规范）

`.gates/req-guard.yaml` 默认 `enforce.ci: true`——CI 必须**独立重跑**门禁，
本机任何绕过（含 `git commit --no-verify`）都会在服务端被抵消：

1. 流水线中执行 `req-guard check`（退出码非 0 即失败）；CI 镜像内置 req-guard
   二进制，版本与 Release tag 一致（`req-guard -V` 可核对）；
2. 将该检查设为**必需（required）状态检查**：不通过禁止合并；
3. 分支保护：禁止直推 `main` 等受保护分支；
4. 并行开发团队建议追加一步 `req-guard ids --check`：合并后扫描需求编号冲突，
   同 id 多文件 / 前缀歧义即红（规范详见 `docs/规范/需求编号防冲突命名规范.md`）；
5. 建议追加一步 `req-guard install --verify`：任一在用 AI 工具缺 hook 即红，
   消除 L1 静默缺口。

**开始接入**：`req-guard install` 已在本项目生成可直接部署的样例
`.gates/ci/req-guard-ci.yml`（GitHub Actions）——把它复制到 `.github/workflows/`
并设为必需状态检查即可；该样例同时跑 `req-guard check`、`ids --check`
与 `install --verify`。

## 审计台账与摘要（随仓库提交）

- `.gates/audit/ledger.md`：approve / reject / resolve / bypass / 阻塞性评论等
  关键事件的**入库台账**——PR diff 可直接复核"谁在何时批了什么"；
- `.gates/audit/DIGEST`：`req-guard audit-digest` 生成本机 `gate-audit.log` 的
  SHA-256 摘要；PR 中与本地日志比对即可发现事后篡改；
- `gate-audit.log`（全量流水）与 `.bypass` 是本机运行态，已由 .gitignore 忽略。

## 与 gates-toolkit 的关系

- **gates-toolkit**：代码质量门禁（提交时查 SQL/Java 规范）——管"写得对不对"。
- **AI 需求门禁**：流程门禁（写代码前查审核）——管"该不该写"。
- 两者互补；req-guard 生成时把 AI 门禁**追加**在 gates-toolkit 的 pre-commit 之后，不覆盖。
