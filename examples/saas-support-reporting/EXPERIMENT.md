# 第三企业 Code Agent 建模实验记录

## 目的

检验一个没有阅读既有 B2B/B2C 实现的 Code Agent，能否只依靠：

- 新企业的数据模型和文本化业务知识；
- 一份最小 Telora 教程；
- `ontology-method` 与 `analytics-ontology` 的公开接口；

独立写出企业 ontology、验证与 lowering 代码，并根据真实诊断快速收敛。

Agent 被明确禁止执行 Telora、Cargo、测试和 formatter。所有执行均由实验主持方
完成，Agent 只能根据返回的诊断修改代码。

## 时间线

| 时间（CST） | 事件 |
|---|---|
| 00:05 | 下发题面、领域知识和最小教程 |
| 00:09:10 | Agent 交付 256 行第一版模型及 valid/invalid 用例 |
| 00:09:58 | 主持方反馈第一条诊断：裸 `True` 是未知 binding |
| 00:11:07 | Agent 修正两处 Bool tag 写法 |
| 00:11:39 | 主持方反馈第二条诊断：`flat_map` 结果类型无法推导 |
| 00:13:08 | Agent 增加 typed adapter；第三次 check 通过 |
| 00:15 | valid 执行和 invalid Host recovery 验收通过 |
| 00:16:44 | 主持方反馈重复 relation 所造成的 catalog 语义不纯 |
| 00:17:06 | Agent 清理 catalog，并完成实验复盘 |

从下发题面到静态检查收敛约 8 分钟，到行为验收约 10 分钟，到模型语义审查
完成约 12 分钟。

## 诊断闭环

第一轮：

```text
model.telora:189:40: unknown binding "True"
```

Agent 不仅修复这一处，还自行找到 `read_only: True` 的同类问题。

第二轮：

```text
model.telora:188:75:
field required_entities: cannot infer generic result type Array<?36>
```

Agent 增加 `Fn(MeasurePlan) -> Array(Entity)` adapter 固定 `flat_map` 的结果类型，
没有使用 `Any`、`Dyn`，也没有修改共享库。

第三次静态检查通过。有效场景第一次执行即得到包含 Ticket grain、四个维度和
完整安全 join 链的 `Some(ExecutionPlan)`。

普通 CLI 对非法场景只显示首条生产诊断，因此主持方增加 Host recovery 回归，
证明一次恢复同时包含：

```text
no ontology capability is defined for CustomerSegment
reaching Tag expands the measure grain
```

## 语义审查

第一版同时把安全边 `TicketTag -> Tag` 放入 safe 和 fan-out catalog。行为测试仍
能通过，但这种重复让 catalog 名称不再准确。共享算法本来会合并两组边计算完整
可达性，因此 Agent 根据审查反馈删除重复，只把真正改变 grain 的
`Ticket -> TicketTag` 留在 fan-out catalog。修正后的 recovery 仍通过。

这说明只依靠执行测试不足以审查 ontology 质量；企业事实是否被放进正确的语义
分类仍需要模型审查或额外 invariant。

## 结果

Agent 第一次建模就正确完成了：

- 十一个封闭 Entity、两个 Measure、六个 Dimension；
- capability 与“enum 中存在但尚未获准 lowering”的区别；
- safe 和 fan-out relationship 事实；
- 两种 natural grain 及显式同 grain 组合策略；
- typed CombinedMeasure 与 ExecutionPlan；
- `analytics.compile_with` 的完整实例化；
- 有效计划与双错误非法意图。

企业代码只表达企业知道的枚举、公式、物理 mapping、组合策略和最终 builder。
capability 搜索、独立 lowering、路径闭包、fan-out/missing 分类、诊断与原子发布
均由共享 DSL 承担。

## 暴露的缺口

1. 最小教程应明确 Bool 是 `'True` / `'False` tag；
2. 泛型数组组合子何时需要 typed adapter 仍不够可预测；
3. safe 与 fan-out catalog 在完整路径闭包中的合并语义需要更直接的公开说明；
4. Host recovery 才能观察完整诊断集，CLI 默认首错输出不适合作为该能力的验收；
5. catalog 分类正确性目前依靠阅读和领域审查，没有静态 invariant 保证。

这些缺口影响学习和排障速度，但本实验没有发现阻止第三企业表达合理业务意图的
语言机制缺口，也没有迫使企业复制行业 lowering 流程。

## 结论

这次结果支持“Telora 是宿主语言，ontology 方法论是 embedded DSL”的定位。
共享库不只是帮助函数集合：它规定企业模型的 extension points、组合顺序、失败
语义和发布条件。第三企业可以在不了解前两个企业实现的情况下使用这门 DSL，并
在两条编译诊断和一次语义审查后快速收敛。
