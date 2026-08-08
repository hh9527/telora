# 实现说明

## 企业事实

`model.telora` 定义了 SaaS 客服企业自己的封闭 `Entity`、`Measure` 和
`Dimension` 集合，以及语义值、聚合、基数等封闭分类。指标公式、维度表达式、
十一张表之间的物理 join mapping、同 natural grain 的组合策略和最终
`ExecutionPlan` 都属于企业知识。

`CustomerSegment` 是企业可以谈论、但尚未批准 lowering 的概念，因此只出现在
`Dimension` enum 中，不出现在 capability catalog。`TicketTag` capability 已获批准，
但从 Ticket grain 到 Tag 必须先经过 one-to-many 的 `TicketTag`，因此仍不能发布。

## 共享 Ontology DSL 承担的规则

模型调用 `analytics.compile_with`，由共享 DSL 负责 capability 查找和 lowering、
结果完整性判断、所需 entity 收集、安全路径闭包、fan-out 与 missing path 分类、
路径诊断，以及只有完整候选才可发布的阶段顺序。

企业模块没有重新实现 capability 搜索、图遍历或最终完整性门禁。

## Typed adapter 样板

调用共享编译器时保留了下列纯 adapter：

- 从 measure/dimension capability 取 `id`；
- 调用 capability 自带的 `lower`；
- 从组合指标取 base entity 和 required entities；
- 从维度计划取 required entity 和诊断 subject；
- 从关系取 `from` 和 `to`。

这些函数不包含企业决策，只用于让共享高阶函数在不擦除具体类型的情况下访问
企业 record 字段。

## 当前别扭之处

`compile_with` 是较长的位置参数接口，多个 selector closure 是机械 forwarding。
Telora 目前不能在泛型签名里精确表达由 metadata family 生成类型的字段约束，因而
共享库不能直接写 `capability.id` 或 `relation.from`，必须由企业层传 adapter。

本实现没有使用 `Any`、`Dyn` 或字符串 id 来掩盖这些限制。

## 实验复盘

第一次编译在裸 `True` 上停止。Telora 的 Bool 值实际使用 `'True` tag；教程给出的
语法范围没有明确说明这一点。第二次编译发现 `array.flat_map` 无法推导匿名函数的
泛型结果，于是增加了带完整签名的 `measure_required_entities` adapter。此后 valid
计划正确产生，invalid 在 Host recovery 中同时报告缺失的 `CustomerSegment`
capability 和 `TicketTag` fan-out。

语义审查还发现初稿在 `fanout_relations` 中重复放入了安全的
`TicketTag -> Tag`。共享算法会合并 safe 与 fan-out catalog 计算完整可达闭包，因而
这里只需把真正改变 grain 的 `Ticket -> TicketTag` 放入 fan-out catalog；后续安全边
保留在 `safe_relations` 即可。修正后两个 catalog 与企业 cardinality 知识一致。

教程足以确定封闭类型、metadata family、capability lowerer、typed adapter、
`compile_with` 的阶段和参数顺序，也足以独立完成企业知识与共享方法论的分层。
需要猜测的部分主要是 Bool 的具体值语法、局部泛型推导何时必须显式签名，以及
safe/fan-out 两组边在共享路径闭包中的精确合并方式。后两项都没有迫使模型退化到
`Any` 或复制共享 lowering 流程，但适合补入后续教程。
