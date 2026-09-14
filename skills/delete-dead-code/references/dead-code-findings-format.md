# Dead Code Findings Format

每个模块 agent 产出一份该模块的 findings artifact。`dead_code_scope.py verify` 按本文的 finding
行锚点做范围校验：**只有 finding 行参与校验**，正文散文提到路径不算 finding。

产物路径：`temp/delete-dead-code/<branch-slug>/<module-slug>.md`，其中 `<module-slug>` 是模块名把
`/` 换成 `-`；`<repository-root>` 写作 `repository-root`。

## 结构

````markdown
# Dead Code Findings

- Module: `<模块名，与 scope 输出逐字一致>`
- Base: `<不可变 base commit SHA；显式目录模式写扫描时的 HEAD>`
- Outcome: `CLEAN|ACTED|REPORTED`

## Findings

- `<repo 相对路径>:<行号>` — DUPLICATE|DEAD-BRANCH|DEAD-CODE — `<符号名>` — <一句话描述> — CONFIRMED|LIKELY — <用于确认的 grep>

## Disposition

- `<repo 相对路径>:<行号>` — DELETED|CONSOLIDATED|KEPT|HANDOFF — <为什么；KEPT 给出保留机制，HANDOFF 必须写明阻塞在哪个模块、那边要改什么>

## Verification

- `<命令>` — <真实结果摘要>
````

## 规则

- 三种 Outcome 互斥，且由 `verify` 机械判定，不是自述：
  - `CLEAN` —— 一条 finding 都没有。`## Findings` 与 `## Disposition` 都必须是 `- 无`。
  - `ACTED` —— 至少有一条 `DELETED` 或 `CONSOLIDATED`。
  - `REPORTED` —— 有 finding，但一条都没动（跨模块、LIKELY、故意重复）。
  **`REPORTED` 存在的理由**：没有它，「查到十条真死代码但一条都动不了」只能记成 `CLEAN`，
  汇总就会把有问题的模块报成干净的。dogfood 第一轮真实发生过。
- `HANDOFF` 是删除动作跨出了本模块：符号在我这儿，唯一的调用方或测试在别人那儿。写明目标模块，
  主 agent 会在 fan-out 结束后配对处置。**不要为了让 Outcome 好看而把 HANDOFF 写成 KEPT。**
- finding 行必须以 `` `path:line` `` 开头（允许前导空白与 `-`/`*` 列表符）。这是 verify 的唯一锚点；
  写成别的形状等于这条 finding 不参与范围校验，属于格式错误。
- 路径一律仓库相对、POSIX 分隔符，不写绝对路径。
- `CONFIRMED` 表示已按仓库全量 grep 确认无引用，证据列出实际执行的 grep；`LIKELY` 表示查不到引用
  但可能被字符串、动态、跨语言或平台条件到达。
- **只有 `CONFIRMED` 可以 `DELETED` 或 `CONSOLIDATED`。`LIKELY` 一律 `KEPT`**，由主 agent 汇总上报。
- `KEPT` 的理由必须具体到机制（真注入缝 / 平台门控 / 边界测试钉死的故意重复 / 生成契约），
  不接受「保守起见」。
- 同一 `path:line` 在 `## Findings` 与 `## Disposition` 中各出现一次，两边必须一一对应；`verify` 对单边出现的 anchor 报错。
- 置信度字段后可以补一句限定语，但 `CONFIRMED` / `LIKELY` 本身必须紧跟在一个 `— ` 之后。

## 正例（CLEAN）

````markdown
# Dead Code Findings

- Module: `src/client`
- Base: `844205cb5df4303378daafa917c74f1770749df4`
- Outcome: `CLEAN`

## Findings

- 无

## Disposition

- 无

## Verification

- `just lint` — passed
````

## 反例

- `Outcome: CLEAN` 却列了 finding：状态与内容自相矛盾，`verify` 拒绝并提示应记为 `REPORTED`。
- `Outcome: ACTED` 却一条 `DELETED`/`CONSOLIDATED` 都没有，或 `REPORTED` 却动了东西：同样拒绝。
- finding 有 anchor 而 Disposition 没有（或反之）：拒绝——处置漏写等于这条 finding 没有结论。
- finding 行写成 `- src/client/render.rs:12 — DEAD-CODE …`（缺反引号）：不会被 verify 锚点匹配，
  这条 finding 事实上逃过了范围校验。
- `LIKELY` 配 `DELETED`：未经确认的删除，拒绝。
