# 四角色 Ontology eDSL 实验

本目录研究一个具体问题：一套用自然语言描述的方法，能否经过多个上下文受限的 AI 角色，
逐层变成可执行、可诊断、不会绕过企业策略的 Telora 意图编译系统。

实验不是让多个 Agent 同时自由协作，而是有意把知识拆开：前一层只发布下一层真正需要的
接口和教程，后一层在看不到隐藏实现或私有事实的条件下继续工作。Host 用 Telora 的
`check`、`run`、诊断来源和语义审查判断知识是否被正确传递。

## 一句话模型

```text
Telora 语言知识
-> 可复用 ontology eDSL
-> 私有企业 ontology
-> 公共业务意图
-> typed execution plan 或带来源的拒绝
```

这里的核心不是“让 AI 生成一段查询”，而是验证：合法意图只能沿经过批准的 lowering
路径变成计划，越界意图会在正确的知识层被拒绝。

## 四个角色

已记录的实验把系统组织为 Main、A2、A3、A4 四个角色。目录中的 `a1` 表示稳定输入层，
不是本轮重新训练或实现的 Agent。

| 角色 | 责任 | 不应掌握的知识 |
|---|---|---|
| Main / Host | 分阶段准备输入、调度角色、执行独立验收、转发有界反馈、记录实验 | 不代替角色悄悄修候选代码 |
| A2 | 阅读 Telora 教程和 eDSL 设计，实现可复用 ontology eDSL，并写给 A3 的教程 | 私有企业领域、后续意图试验、隐藏验收 |
| A3 | 使用 A2 的 eDSL 建模私有企业，发布公共 intent vocabulary 和 compile 接口 | A4 请求、隐藏验收、其他企业参考实现 |
| A4 | 仅依照获准的公开接口表达业务请求，或在闭合词汇无法表达时明确拒绝 | 私有表结构、关系映射、plan builder、隐藏预期分类 |

Main 是唯一反馈中继。A2、A3、A4 不直接通信，因此每次修正都必须明确落到拥有该知识的
层，而不是靠共享聊天历史把边界抹平。

## 为什么需要 ontology eDSL

企业分析系统通常同时包含两类知识：

1. 可以跨企业复用的编译机制，例如 capability 查找、独立 lowering、关系路径分类、诊断
   汇集和原子发布门。
2. 只能由企业定义的事实，例如 Measure、Dimension、自然 grain、关系 mapping、授权策略、
   组合规则和最终 Plan 类型。

eDSL 的作用是把两类知识分开。共享库拥有编排顺序和通用规则，企业通过封闭类型、catalog
和 typed callback 注入自己的知识。共享层不得用 `Any`、`Dyn` 或 String id 抹掉类型关系，
企业层也不应复制一遍共享编译流程。

详细表面设计见 [edsl-design.md](edsl-design.md)。实验使用的私有物流题面见
[domain.md](domain.md)。

## 两个关键不变量

### Best-effort diagnostics

多个相互独立的请求应尽可能分别完成。一个 capability 缺失，不应让已经完成的其他请求
失去关系或策略诊断。这样 Host 能看到“为什么失败”的完整因果链，而不只是第一个布尔错误。

### Atomic publication

执行过部分 lowering 或构造出 candidate，不代表可以发布计划。只有 capability、组合、关系、
授权和 final builder 所需证据全部成立时，compile 才能返回 `Some(Plan)`；任何不完整证据都
必须阻止发布。

这两个不变量共同避免两种常见失败：过早停止导致诊断不足，以及拿部分成功结果拼出一个
看似可用、实际绕过策略的计划。

## 三个执行阶段

### Stage 2：实现共享 eDSL

A2 只看到 Telora 教程和 eDSL 设计。Host 使用 A2 未见过的 typed probe 或微模型检查：

- API 是否保持封闭类型；
- capability 是否逐项 lowering；
- safe、fan-out-only、missing path 是否正确分类；
- 诊断是否保留 authored subject 和规则来源；
- 不完整证据是否绝不发布计划。

### Stage 3：实现私有企业模型

A3 获得接受后的 eDSL、教程和一份私有领域题面。它负责企业 vocabulary、catalog、关系事实、
grain/aggregation policy、物理 mapping 和 typed plan builder，同时发布不含实现体的公共接口。

Stage 3 验收既包含合法 lowering，也包含缺失 capability、不同 grain、fan-out 和授权失败。

### Stage 4：公共意图试验

A4 使用闭合公共 vocabulary 构造 intent，不得输出 SQL、物理 mapping、手工计划或替代
compiler。固定 corpus 覆盖六类请求：

| 类别 | 检查目标 |
|---|---|
| direct | 教程直接暗示的合法组合能否 lowering |
| novel | 未列举但由公开概念组成的新组合能否 lowering |
| unapproved | vocabulary 中存在但没有获准 capability 的概念是否由模型拒绝 |
| mixed | 不同自然 grain 的指标是否需要显式组合策略 |
| fanout | 只能经扩张关系到达的维度是否由模型拒绝 |
| impossible | 公共闭合词汇无法表达时，A4 是否拒绝而不是发明 id |

Host 使用三种结果分类：

- `lowered`：合法公开 intent 得到 typed、只读计划；
- `model-rejected`：intent 可以表达，但企业模型依据 capability、grain、关系或授权策略拒绝；
- `agent-refused`：公共 vocabulary 根本无法忠实表达请求，A4 明确拒绝造词。

`model-rejected` 和 `agent-refused` 不是同一件事。前者证明策略存在于可执行模型中，后者证明
意图作者尊重闭合接口边界。

## 两种实验形态

本目录保留了两种互补实验。它们使用相似的四角色链，但隔离目标不同，结论不能混为一谈。

| 维度 | opencode `test-1` | Codex `test-1` |
|---|---|---|
| 主要目的 | 可观察的协作与攻击反馈闭环 | 最小分阶段输入下的知识传递 |
| 读取模型 | 前缀读：下游可见此前完整产物 | 每阶段独立 workspace，只放允许输入 |
| A4 边界 | 看得到实现，但协议要求只能走 `compile` | 只看到公共 vocabulary、教程和声明 API |
| Telora 执行 | Main 代角色执行并转发诊断 | A2/A3 可执行固定 Telora；A4 不执行 |
| 主要发现 | A4 攻击发现真实缺口；Telora 泛型推断可发现性差 | 六类固定 intent 全部得到正确 Host 分类 |
| 隔离结论 | 协作式、协议强制边界 | `soft-reproducible-v1` / `instruction-isolated` |
| 重要限制 | Main 补过 compiler，A4 可见完整实现 | A4 跨 Main 会话获授权重建，存在编排偏差 |

opencode 实验的价值在于“攻击者发现问题后，修复能否准确回到知识所属层”；Codex 实验的
价值在于“只传递公开接口和必要教程时，功能链是否仍能成立”。

## 已归档结果

### opencode

[opencode/test-1](opencode/test-1/) 记录了一次航班运营领域实验。A2 实现 eDSL 时经历多轮
泛型推断问题，Main 最终补入参考 compiler 形状；A3 使用 DSL 的收敛显著更快；A4 的攻击
试验发现空请求、重复请求、fan-out 发布门和导出面四个缺口，并推动 A2/A3 分层修复。

该结果说明协作式攻击反馈有工程价值，但不构成 clean-room 证明。详见
[opencode/test-1/FINAL-SUMMARY.md](opencode/test-1/FINAL-SUMMARY.md)。

### Codex

[codex/test-1](codex/test-1/) 归档正式 run `20260810-161853`：

- Stage 2 经一次 bounded correction 接受；
- Stage 3 在 round 0 接受；
- Stage 4 得到 2 个 `lowered`、3 个 `model-rejected`、1 个 `agent-refused`；
- 固定六类 corpus 中没有错误接受或错误拒绝。

该 run 的功能结果成立，但 A4 身份在跨 Main 会话恢复时按人工授权重建，Main 还多次把预期
collaboration 调用误路由为无副作用的 `exec true`。因此状态诚实记录为
`completed-with-protocol-deviations`，不宣称严格 registry conformance。详见
[codex/test-1/README.md](codex/test-1/README.md) 和
[codex/test-1/FINAL-SUMMARY.md](codex/test-1/FINAL-SUMMARY.md)。

## 如何阅读本目录

建议按以下顺序：

1. 本文：理解实验问题、角色和分类。
2. [edsl-design.md](edsl-design.md)：理解共享机制与企业知识的所有权边界。
3. [domain.md](domain.md)：查看私有企业题面如何实例化这些抽象角色。
4. [stage4-trials.yaml](stage4-trials.yaml)：查看固定意图 corpus。
5. [RUNBOOK.md](RUNBOOK.md) 和 [execution-profile.yaml](execution-profile.yaml)：查看正式执行协议。
6. 两个 `test-1` 的 `RUNLOG`、Host validation 和最终总结：比较探索性实验与隔离实验。

`roles/` 保存逐字 staged 的角色说明；`prepare-workspace.sh` 用于创建独立 Git workspace；
`agent-registry.template.yaml` 描述正式 run 的 Main-star 固定身份映射。

## 可以声称什么

当前证据支持以下有限结论：

- 这套 Telora 教程和 ontology eDSL 设计可以穿过分层角色，形成可执行企业模型；
- eDSL 消费侧可以保留企业封闭类型、关系 mapping 和策略所有权；
- 合法 intent 可以 lowering 为 typed plan，非法或不可表达 intent 可以在正确边界被拒绝；
- 攻击试验和 bounded feedback 能暴露并修正跨层设计缺口。

当前证据不支持以下更强结论：

- 任意模型、任意领域或任意 ontology 都能同样收敛；
- instruction isolation 等同于操作系统级或对抗性 filesystem isolation；
- AI-2 在所有运行中都独立发明或完成了 eDSL 方法；
- 生成的计划已经获得真实企业数据执行授权；
- 单次六类 corpus 可以证明生产系统的完整安全性。

实验真正测量的是“知识能否被分层表达、传递、执行和拒绝”，而不是一个笼统的 Agent 成功率。
