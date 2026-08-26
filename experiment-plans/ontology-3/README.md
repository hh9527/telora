# Ontology 3: QueryBuilder -> eDSL -> EnterpriseKnowledge

这是 Telora 主仓库内受版本控制的平台无关实验方案，包含五个隔离角色：

```text
A1 QueryBuilder: Plan -> SQLite Query
A2 ontology eDSL: type/member metadata -> prepared Request -> Plan
A3 ent-1 model: domain facts -> nominal types + typed properties
A4 intent-1: private text intent -> JSON -> fixed query command -> acceptance result
A5 query-1: numbered problem -> numbered JSON answer -> fixed query command -> production answer
```

`Plan` 使用标准算子且方言中立；本轮 QueryBuilder 只具体化 SQLite，`Query` 形状为
`{ sql: String, bindings: Array(Val) }`。A2 的 prepared EnterpriseKnowledge 声明可接受的
`PlanProfile`。A3 验证 `ontology property + Request -> Plan -> Query`。

## 角色与可见性

- A1 只实现 `query-builder/`。
- A2 只实现 `ontology/`，只看 A1 的公共教程和契约。
- A3 只实现 `ent-1/`，只看 A1/A2 的公共教程和契约；公共查询面不得泄漏物理 mapping。
- A4 只修改 `intent-1/intent.json`、`intent-1/invalid/*.json` 和验收记录，只执行
  `just a4 ...`，只看 A3 的公共查询教程/契约、JSON 接口文档和自己的私有文字意图。
- A5 只修改 `query-1/answers/<problem-id>.json`，只执行
  `just a5 make-query <problem-id>`，看不到
  Telora、私有模型和物理 mapping。
- coordinator 只启动五个长期角色，之后不解释或调度工作。

每个角色只使用两个 DAG 命令：

```text
./bin/oc-task pull <role>
./bin/oc-task submit <role> <artifact...>
```

角色只能提交以自己的角色名结尾的 artifact，例如 A3 只能提交 `.a3`。具体所有权、依赖、
检查项和 freshness 均由 DAG 引擎检查。每个角色永远循环 pull；无工作时 `pull` 最多等待
60 秒并返回 waiting，角色必须立即再次 pull。一次 pull 只按声明顺序返回第一个 runnable artifact；角色只完成并提交这个
artifact，然后再次 pull。任务不合并。

`pull` 对每个输出分别列出 `output_mtime_ns`，并为每个直接输入列出 `mtime_ns` 和
`changed`。`changed` 等价于 `input.mtime_ns > output_mtime_ns`，不需要保存历史状态。
角色必须重新读取所有 `changed: true` 的输入并据此检视、更新和验证当前输出，不能因为
checks 中的旧文件仍然存在而直接 submit。

## Artifact DAG

```text
lang + qb-req -> qb.a1
qb.a1 -> qb-feedback.a2 / qb-feedback.a3
Host 整合审查 -> qb-feedback? -> qb.a1 修订
qb.a1 + qb-feedback.a2 + qb-feedback.a3 -> Host 发布 qb

lang + edsl-req -> lang-learn.a2
qb + lang-learn.a2 -> edsl.a2
edsl-feedback? -> edsl.a2 修订
edsl.a2 -> edsl-feedback.a3
edsl.a2 + edsl-feedback.a3 -> Host 发布 edsl

lang + domain-ent-1 -> lang-learn.a3
qb + edsl + lang-learn.a3 -> ent-1-model.a3
ent-1-model.a3 -> Host 发布 ent-1-model
ent-1-model -> ent-1-query-surface.a3
ent-1-query-surface.a3 -> ent-1-query-surface-feedback.a4
Host 整合审查 -> ent-1-query-surface-feedback? -> ent-1-query-surface.a3 修订
ent-1-query-surface.a3 + ent-1-query-surface-feedback.a4 -> Host 发布 ent-1-query-surface

intent-req + ent-1-query-surface.a3 -> ent-1-query-surface-feedback.a4
intent-req + ent-1-query-surface -> intent-1.a4
intent-1.a4 -> Host 发布 intent-1

Host 发布 query-engine + query-doc + homework -> homework.a5
homework.a5 -> Host 审批 lic
Host 投递并发布 problem + lic -> answer.a5
answer.a5 -> Host 验收 answer
```

带 `?` 的输入只是普通可选 artifact：缺失时 mtime 视为 0，不阻断首版；发布后使较旧的
候选及其下游自动 stale。实际交付文件只作为存在且非空的检查项，不直接触发 DAG。
所有状态都由 `control/artifacts/*` 的 mtime 推导，不存在 claim、generation 或专用
feedback 状态。

所有跨角色正式移交都由 Host 审核后发布无角色后缀的 artifact：

```bash
./oc-ctl status t1 ontology-3/1
./oc-ctl update t1 ontology-3/1 query-builder/FEEDBACK.md=feedback/qb.md
./oc-ctl publish t1 ontology-3/1 qb-feedback
./oc-ctl publish t1 ontology-3/1 qb
./oc-ctl publish t1 ontology-3/1 edsl-feedback
./oc-ctl publish t1 ontology-3/1 edsl
./oc-ctl publish t1 ontology-3/1 ent-1-model
./oc-ctl publish t1 ontology-3/1 ent-1-query-surface
./oc-ctl update t1 ontology-3/1 ent-1/QUERY-SURFACE-FEEDBACK.md=feedback/query-surface.md
./oc-ctl publish t1 ontology-3/1 ent-1-query-surface-feedback
./oc-ctl publish t1 ontology-3/1 intent-1
./oc-ctl publish t1 ontology-3/1 query-engine query-doc
./oc-ctl publish t1 ontology-3/1 lic
./oc-ctl publish t1 ontology-3/1 problem
./oc-ctl publish t1 ontology-3/1 answer
```

A5 上岗通过后，十道真题由 Host 从 `host/a5-cases/` 逐题投递。例如在 Telora 仓根目录：

```bash
./oc-ctl update t1 ontology-3/1 \
  query-1/PROBLEM.md=experiment-plans/ontology-3/host/a5-cases/01.problem.md
./oc-ctl publish t1 ontology-3/1 problem
```

每份题面首行包含四位题号。A5 以相同题号创建 `query-1/answers/<problem-id>.json`；信息
不足、意图不合法或歧义尚未消除时不创建该题答案文件。上岗考试固定使用题号 `0000`。

`host/` 不属于 manifest 的 `workspace`，不会在启动或 A1-A4 阶段复制到 Agent 可见目录；
`A5-HARD-QUERIES.md` 中的 Host 预期也永远不投递。

发布反馈 artifact 前，Host 先把筛选后的正文写入其 checks 指定的反馈文件。语言机制问题
单独跟踪，不要求角色绕行。A4 首轮后最多追加一次当前公共接口范围内的查询面迭代。

## 运行

```bash
./oc-lab run t1 --port 4199
```

外部窗口只提供 lab。Host 从 Telora 仓根目录验证连接并选择本计划：

```bash
./oc-ctl test-connect t1
./oc-ctl start t1 ontology-3
```

要在新会话中复用旧执行已经验收的 A1-A4 产物，而不重新运行长耗时阶段：

```bash
./oc-ctl start t1 ontology-3 --from ontology-3/1
```

来源执行中兼容且仍为 current 的 artifact 会连同 checks 文件一起继承；Host promotion
可以安全吸收旧审批前已经完成的新增 review 门禁。旧会话和
`.oc-task` 不会继承。Host 随后发布新增的 `query-engine`、`query-doc`，A5 即从上岗考试
开始工作。

使用以下命令观察和干预：

```bash
./oc-ctl status t1 ontology-3/1
./oc-ctl status t1 ontology-3/1 --verbose
./oc-ctl stat t1 ontology-3/1
./oc-ctl update t1 ontology-3/1 path/in/workspace=path/in/current/directory
./oc-ctl publish t1 ontology-3/1 artifact
./oc-ctl resume t1 ontology-3/1 a5
```

准备阶段复制 manifest 声明的方案资产并构建 release Telora binary。OpenCode 配置、角色
frontmatter、权限和 coordinator 由 `oc-ctl` 确定性生成，不属于方案资产。运行时
`experiment.json` 对角色不可见。
