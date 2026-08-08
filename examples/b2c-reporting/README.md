# B2C 报表复用实验

这个十二表模型用于检验 `ontology-method` 是否能被第二个企业模型直接复用，
并验证 `analytics-ontology` 能否作为 Telora 内嵌的行业 DSL 统一编排 capability、
关系路径与完整性证明。

企业层仍然定义自己的实体、指标、维度、关系事实、物理映射、grain 组合策略和
最终执行计划；共享 DSL 负责通用 lowering 顺序。当前允许少量 typed selector 和
`CombinedMeasure` adapter，以保持企业类型封闭而不退化为 `Any` 或 `Dyn`。
它没有导入 B2B 模型，也没有共享表名、指标公式或 SQL payload。

共享内容包括：

- TypeMetadata capability 构造器；
- capability lookup、独立 lowering 与 completeness；
- bounded relation closure 与 connecting-edge selection；
- 缺失 capability 的统一诊断规则。

B2C 自己定义十二个 Entity、两个 Measure、五个 Dimension、物理关系和映射。
合法意图从 Order grain 取得月份、消费者地区和获客 Campaign。非法意图同时
触发 ProductCategory fan-out 与缺失 LoyaltyTier capability，证明模型规则与共享
规则可以在一次 best-effort 求值中共同反馈。

```sh
cargo run -p telora -- run examples/b2c-reporting/valid.telora
cargo run -p telora -- run examples/b2c-reporting/invalid.telora
```
