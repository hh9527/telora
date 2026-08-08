# 第三企业 Ontology 建模挑战

## 任务

基于 [DOMAIN.md](DOMAIN.md) 中给出的 SaaS 客服企业知识，使用 Telora 和现有
`analytics-ontology` embedded DSL，实现一个封闭、类型化的 ontology 模型。

只能依据以下材料设计：

- 本文件；
- `DOMAIN.md`；
- `TELORA-TUTORIAL.md`；
- 教程明确列出的共享库源文件。

不要阅读或复制 `examples/b2c-reporting`、`examples/intelligent-reporting` 中的
实现。这是一次独立建模实验，不是移植练习。

## 交付文件

在本目录创建：

```text
telora-deps.json
model.telora
valid.telora
invalid.telora
IMPLEMENTATION.md
```

`IMPLEMENTATION.md` 用中文简要说明：

1. 哪些定义是企业事实；
2. 哪些规则由共享 ontology DSL 承担；
3. 为调用共享 DSL 写了哪些纯 adapter 样板；
4. 哪些地方因为 Telora 机制限制而变得别扭。

## 必须满足的行为

`valid.telora` 请求：

```text
measure    ResolvedTickets
dimensions OpenedMonth, WorkspacePlan, AssignedTeam, SlaPolicy
```

成功时应得到 `Some(ExecutionPlan)`，且计划中能看见：

- revision `saas-support-v1`；
- base entity Ticket；
- resolved ticket 的表达式；
- plans、agents、teams、sla_policies 等关系映射；
- `read_only: True`。

`invalid.telora` 请求：

```text
measure    ResolvedTickets
dimensions TicketTag, CustomerSegment
```

在 Host best-effort recovery 下，应至少同时产生：

1. CustomerSegment 没有 capability；
2. 从 Ticket 到 Tag 只能经过 one-to-many fan-out，必须拒绝。

最终结果必须是 `None`，不能靠任意 fallback 发布不完整计划。

## 静态约束

- Entity、Measure、Dimension 必须是封闭的 enum；
- capability、relation 和 plan 必须保留企业具体类型；
- 不得用 `Any`、`Dyn` 或 String id 模拟泛型复用；
- 必须调用 `analytics-ontology/compiler.telora` 的 `compile_with`；
- 企业模块只表达领域事实、企业策略和 typed adapters；
- 不修改 Telora 编译器、标准库、`ontology-method` 或
  `analytics-ontology`；
- 不引入 trait、effect、Host 特例或新的语法。

## 实验规则

编写期间不要执行 `cargo`、`telora`、测试、formatter 或任何用于验证代码的
命令。完成文件后停止，并报告你预计最可能出现的三个错误。执行和诊断反馈由
主实验者负责。
