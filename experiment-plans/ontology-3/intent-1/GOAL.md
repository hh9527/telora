# A4 目标：验收公共 JSON 查询接口

你是不了解数据库物理实现的查询设计者。只依据 A3 发布的公共查询面，将
`intent-1/INTENT.md` 中的文字意图忠实表达为公开形状的 JSON，通过固定 `just a4`
入口得到 Query 或诊断。

## 完整输入清单

- `intent-1/GOAL.md`
- `intent-1/INTENT.md`
- `intent-1/FEEDBACK.md`
- `query-1/QUERY-DOC.md`
- A4 自己已经产生的 `intent-1/intent.json`、`intent-1/invalid/**` 与 `intent-1/NOTES.md`
- Host 放行后：`ent-1/QUERY-DESIGNER-TUTORIAL.md` 与
  `ent-1/PUBLIC-QUERY-CONTRACT.md`

Supervisor 投递 `ent-1-query-surface-feedback.a4` 时，先依据私有文字意图检视公共查询面，
并把原始意见写入 `intent-1/FEEDBACK.md`；Host 整合并放行公共查询面后才开始最终 Request。

不得读取企业私有 DOMAIN、A3 私有模型源码、QueryBuilder/ontology 的文档或源码。
不得通过错误消息、`query` 或其他工具反向恢复表、列、alias、join mapping 或 SQL 模板。

## 交付物

- `intent-1/intent.json`：合法文字意图的完整结构化表达；
- `intent-1/invalid/grain-fan-out.json`：不安全计数单位/维度组合；
- `intent-1/invalid/non-positive-limit.json`：非正 limit；
- `intent-1/invalid/unsupported-ordering.json`：未请求或不受支持的排序目标；
- `intent-1/invalid/wrong-filter-type.json`：类型不合规的筛选输入；
- `intent-1/FEEDBACK.md`：公共查询面存在的具体歧义、缺口或不必要泄漏；
- `intent-1/NOTES.md`：JSON 选择、从真实命令输出取得的完整 `{sql, bindings}`、诊断
  结果和剩余风险，不得只转述“bindings 已验证”。

合法意图是参数化能力的初始验收，不是展示性示例。验证必须确认月份上下界、客户等级、
地区和 Top 10 均以 placeholder 对应的有序 bindings 出现，SQL 文本不得内联这些动态值；
同时验证指标降序与三项维度升序的稳定顺序。非法入口除 grain/fan-out 组合外，还应覆盖
至少一个非正 limit、一个未请求的排序目标和一个类型不合规的筛选输入，全部通过带外
`fail!` 失败且不发布部分 Query。

不得修改查询引擎、依赖配置或 `justfile`，不得定义替代 Request DSL、Plan、QueryBuilder、
SQL renderer 或诊断容器，不得直接执行 Telora。A4 不负责解析任意自然语言，只处理本轮
有界意图。

## 验证

```text
just a4 make-query
just a4 verify
just a4 expect-invalid grain-fan-out
just a4 expect-invalid non-positive-limit
just a4 expect-invalid unsupported-ordering
just a4 expect-invalid wrong-filter-type
```

`make-query` 与 `verify` 必须成功并产生逐字节相同的结果；每个 `expect-invalid` 必须观察到
带外诊断且不能发布 Query。完成唯一 Artifact 的验收和验证后结束当前 turn，Supervisor 负责校验
资产和结算。
