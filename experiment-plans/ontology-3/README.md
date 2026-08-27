# Ontology 3: QueryBuilder -> eDSL -> EnterpriseKnowledge

这是 Telora 主仓库内受版本控制、由 Labflow 执行的知识工厂方案。四个隔离角色依次完成：

```text
A1 QueryBuilder: Plan -> SQLite Query
A2 ontology eDSL: type/member metadata -> prepared Request -> Plan
A3 ent-1 model: domain facts -> nominal types + typed properties
A4 intent-1: private text intent -> JSON -> fixed query command -> acceptance result
```

`Plan` 使用方言中立的标准算子；本轮 QueryBuilder 具体化 SQLite，`Query` 为
`{ sql: String, bindings: Array(Val) }`。A2 声明参数化过滤、TOP N 和来源诊断能力，A3 用
真实领域模型验证这些能力，A4 只通过 JSON 与固定 `just` 入口验收。

## 角色隔离

- A1 只修改 `query-builder/`。
- A2 只修改 `ontology/`；批准后的 `qb` 只暴露 QueryBuilder 公共教程和契约。
- A3 只修改 `ent-1/`；批准后的 `edsl` 只暴露 eDSL 公共教程和契约。
- A4 只修改 `intent-1/` 验收资产，只读取 A3 的公共查询教程/契约。
- coordinator 只作为 DAG 根 Session；Supervisor 创建和维护四个角色 Session，并主动投递任务。

Artifact 的 `assets` 同时定义提交检查、保留等级和角色文件权限。角色拥有的 Artifact Asset
可读写；直接输入 Artifact 的 Asset 只读；Host 批准节点只引用允许下游看到的公共资产。

每个角色只响应 Supervisor 当前投递的任务：

```text
阅读 prompt 中的唯一目标、要求和输入变化
充分处理并验证本次任务
labflow agent submit <role> <artifact>
结束当前 turn
```

角色不主动轮询。Supervisor 按声明顺序选择第一个可执行 Artifact，在锁内生成或复用 active task，
并将如下结构转化为文本 prompt：

```json
{
  "target": {"name": "qb.a1", "instruction": "完成 QueryBuilder"},
  "inputs": [{"name": "qb-req", "fresh": true}],
  "assets": [{"path": "query-builder/GOAL.md", "updated": true}]
}
```

文本中的“本轮已更新”表示 `fresh: true`，“需要重新读取”表示 `updated: true`；可选输入尚未发布时
会明确说明尚未提供。已有旧文件不代表当前任务已经完成。prompt 投递失败或 Supervisor 重启时，
同一有效 active task 会被重新投递，而不会产生重复任务。

`submit` 只表示本次任务已完成并交给 Host 检视，不表示任务要求已经达到。角色充分尝试后仍无法
达到要求时，也要整理现有产出和原因并提交；是否批准、反馈或结束流程由 Host 决定。

## Artifact DAG

```text
lang + qb-req + qb-feedback? -> qb.a1
qb.a1 -> qb-feedback.a2 / qb-feedback.a3
qb.a1 + 两份检视 -> Host submit qb

lang + edsl-req -> lang-learn.a2
qb + lang-learn.a2 + edsl-feedback? -> edsl.a2
edsl.a2 -> edsl-feedback.a3 -> Host submit edsl

lang + domain-ent-1 -> lang-learn.a3
qb + edsl + lang-learn.a3 + ent-1-model-feedback? -> ent-1-model.a3
ent-1-model.a3 -> Host submit ent-1-model
ent-1-model + ent-1-query-surface-feedback? -> ent-1-query-surface.a3
ent-1-query-surface.a3 -> A4 检视 -> Host submit ent-1-query-surface

intent-req + ent-1-query-surface -> intent-1.a4 -> Host submit intent-1
```

带 `?` 的输入是普通可选 Artifact：缺失不阻断首版；刷新后使较旧输出及下游失效。所有
Artifact 都独立产生工作压力，没有 `start_artifacts` 或 `finish_artifact`。

## 运行

外部操作员建立实验室：

```bash
./labflow lab run lab-1 --port 4201
```

停止 server 后使用 `./labflow lab remove lab-1` 显式回收实验室目录。

Host 在仓库根目录执行：

```bash
./labflow host test-connect lab-1
./labflow host start lab-1 ontology-3
# 使用 start 返回的 title；首次通常是 ontology-3@1。
./labflow host submit lab-1 ontology-3@1 \
  lang qb-req edsl-req domain-ent-1 intent-req
```

初始输入和后续门禁都由 Host 显式 submit。观察与调度使用：

```bash
./labflow host pull lab-1 ontology-3@1
./labflow host status lab-1 ontology-3@1
./labflow host stat lab-1 ontology-3@1
./labflow host submit lab-1 ontology-3@1 qb
./labflow host resume lab-1 ontology-3@1 a4
```

反馈先更新 Asset，再刷新 Artifact：

```bash
./labflow host update lab-1 ontology-3@1 \
  query-builder/FEEDBACK.md=feedback/qb.md
./labflow host submit lab-1 ontology-3@1 qb-feedback
```

Host 强制替换角色输出必须显式使用 `update --force` 或 `submit --force`。知识工厂完成后，Host
把验收资产整理为 bundle，交给独立的 `benchmark-mode` 计划；题面不进入本 DAG。

需要复用旧执行的已验收资产时使用 `start --from <title>`。方案自身不包含
OpenCode 配置或角色 frontmatter；Labflow 在实验 workspace 中确定性生成运行时 adapter。
