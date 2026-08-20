# 智能报表意图编译器思想实验

- Stage: Discussion
- Scope: executable analytics ontology, report intent, SQL/render lowering
- Parent discussion: `intent-compiler-libraries.md`
- Non-goal: implementing a database driver, SQL executor, or chart renderer

## 问题

能否用 Telora 领域库表达数据表之间的关系、业务语义、指标口径、grain、权限、
钻取本体和 rendering 规则，使 Code Agent 能在限定领域内组合任意报表意图？

成功时，Telora 将高阶报表意图校验并 lowering 为一个原子的执行计划：

```text
SQL + Parameters + ResultSchema + RenderTemplate
```

失败时，Telora 一次返回尽可能完整、靠近根因的领域 feedback，使 Code Agent
修改报表代码而不是猜测数据库错误。

```text
预先发布的 analytics Telora 库
    数据语义 + 本体 + verification + lowering
                    +
Code Agent 生成的 report.telora
                    +
Host 提供的 catalog/权限/方言 Context
                    |
                    v
              Telora 编译
                    |
          +---------+---------+
          |                   |
   ReportExecutionPlan    Diagnostics
          |                   |
   Host 查询与渲染       Code Agent 修复
```

## 为什么不是让 AI 直接生成 SQL

数据库能执行的 SQL 远大于业务允许的报表空间。直接生成 SQL 很难稳定保证：

- 指标口径正确；
- join 不产生 fan-out 和重复计数；
- 时间、货币、退款和 snapshot 语义一致；
- row/column policy 没有被绕过；
- 聚合与 grain 相容；
- SQL result 与图表字段一致；
- 失败能得到领域解释，而不只是数据库错误。

把规则放进 prompt 不能形成权威、版本化、可测试的模型。analytics Telora 库
把这些知识变成普通软件资产。Code Agent 仍有比固定模板更大的组合空间，但
组合空间由领域语义决定，而不是由数据库权限决定。

## 领域库不等于数据库 schema

数据库 schema 主要表达存储语义：

```text
organizations.kind   INT
employees.kind       INT
organizations.id     INT
employees.org_id     INT
employees.id         INT
```

领域库表达业务语义：

```text
organizations.kind   OrgKind
employees.kind       EmployeeKind
organizations.id     OrgId
employees.org_id     OrgId
employees.id         EmployeeId
```

即使底层都是 `INT`：

- `OrgKind` 不能与 `EmployeeKind` 比较；
- `OrgId` 不能与 `EmployeeId` join；
- `Money(CNY)` 不能直接与 `Money(USD)` 相加；
- `OrderCount` 不能与 `EmployeeCount` 计算增长率；
- `ConversionRate` 通常不能直接求和。

Telora 计算的是查询计划，不是真实业务行。因此这些语义身份不必全部成为 Telora
内核中的名义类型。它们可以按适合程度表达为静态类型、类型元数据或 ontology
中的规范化数据。

## 类型化 facade 与数据化本体

已知且稳定的字段可以提供静态 facade：

```telora
eq: for(A) Fn(Expr(A), Expr(A)) -> Predicate;

organization.id: Expr(OrgId);
employee.org_id: Expr(OrgId);
employee.id: Expr(EmployeeId);
```

这样错误 join 可以在局部静态拒绝。动态引用和异质 registry 则使用数据：

```telora
@struct type SemanticTypeDesc = {
    key: String,
    storage: StorageType,
    capabilities: Array(Capability),
};

@struct type FieldDesc = {
    key: String,
    entity: String,
    column: String,
    semantic_type: String,
};
```

AI 通常引用稳定 key：

```telora
measure("net_revenue")
dimension("customer_region")
field("organizations.kind")
```

compiler 从固定 ontology registry 解析 canonical descriptor，不信任意图自行伪造
storage 或 capability。最终 SQL lowering 才把 `OrgKind` 等语义擦除为 SQL
`INT`。

## 可执行领域本体

报表 ontology 可以定义：

- entity、field 和 semantic type；
- relation、join key、cardinality 与授权范围；
- measure、dimension、grain 与 aggregation semantics；
- 时间、地理、组织和产品 hierarchy；
- drill/roll-up edge；
- storage、literal、display 与 render capability；
- 每条规则的来源和稳定 ID。

它不是只有字符串边的知识图：

```text
"Employee" -- "belongsTo" --> "Organization"
```

而是带操作语义的模型。relation 可以知道连接键、基数、时间有效性和 fan-out
策略；drill edge 可以携带 lowering 函数，结合 Context 插入 join、重写聚合、
应用权限并更新结果 schema。

Telora 不需要实现完整 RDF/OWL 或开放世界推理。这里的本体是 operational /
executable ontology：它服务于编译具体意图，而不是推导任意新事实。

## 指标与 grain

相同数值类型不意味着指标可以组合：

```text
Revenue
    Money(CNY) at Grain(OrderLine)

InventoryValue
    Money(CNY) at Grain(ProductSnapshot, Day)

Budget
    Money(CNY) at Grain(Department, Month)
```

指标还具有不同聚合语义：

```text
NetRevenue       Additive
DistinctOrders   RecomputeDistinct
ConversionRate   RecomputeNumeratorAndDenominator
Inventory        SnapshotAtTime
```

因此 comparison、ratio、同比、join 和 drill 必须同时考虑：

```text
semantic value type
grain
aggregation semantics
temporal semantics
authorization scope
```

这些规则可以由普通 descriptor 与 transform 表达；局部稳定关系则可以用类型
提前排除。

## 钻取不是增加一个 GROUP BY

从国家收入钻取到省份，需要知道：

- Province 是否是 Country 的合法下一级；
- 指标到 Province 是否存在授权关系路径；
- 当前指标在新 grain 上能否直接聚合或必须重新计算；
- filter 与时间窗口怎样继承；
- join 是否引入 fan-out；
- 隐私政策是否允许更细粒度；
- render template 应怎样调整。

概念上的规则可以是：

```telora
@struct type DrillRule = {
    from: LevelRef,
    to: LevelRef,
    lower: Fn(DrillContext, ReportIr)
        -> Compilation(ReportIr),
};
```

组织 hierarchy 还可能依赖查询日期选择历史组织快照、过滤虚拟部门并应用当前
权限。可执行本体不仅声明存在一条 edge，还定义这条 edge 如何 verified lower。

## Code Agent 表达高阶报表意图

用户要求：

> 按月和地区展示过去一年已支付订单的净收入，并与去年同期比较，用折线图
> 展示。

Code Agent 可以生成靠近意图的 Telora：

```telora
import "company/analytics" as analytics;

export def report = analytics.report({
    measures: [
        analytics.measure("net_revenue"),
        analytics.previous_period("net_revenue"),
    ],
    dimensions: [
        analytics.dimension("paid_month"),
        analytics.dimension("customer_region"),
    ],
    filters: [analytics.last_months(12)],
    render: analytics.line_chart({
        x: "paid_month",
        series: "customer_region",
        values: ["net_revenue", "previous_net_revenue"],
    }),
});
```

它不需要知道物理表、退款扣除、预聚合、join path、权限 predicate、SQL 方言或
chart binding。领域 compiler 负责：

```text
resolve domain references
    -> infer required entities and grain
    -> find authorized relation paths
    -> verify/rewrite aggregation
    -> lower typed relational IR
    -> lower SQL AST and parameters
    -> derive ResultSchema
    -> verify/lower RenderTemplate
```

## 原子的执行计划

成功结果不应只是一段 SQL：

```telora
@struct type ReportExecutionPlan = {
    query: SqlQuery,
    parameters: Array(QueryParameter),
    result: ResultSchema,
    render: RenderTemplate,
    assumptions: Array(Assumption),
};
```

Plan 应保证 SQL select aliases、result schema 和 render channels 一致。Host 只
负责绑定参数、确认 assumptions/context revision、执行查询并渲染，不重新解释
指标或字段政策。

## 一轮暴露多个问题

假设意图同时包含：

```text
measure "net_revene"          拼写错误
dimension "campaign_channel" 不能组合 employee_count
chart.x "paid_month"          未出现在结果中
```

理想反馈一次给出：

```text
1. unknown measure "net_revene"
   did you mean "net_revenue"?

2. campaign_channel cannot group employee_count
   no authorized relation path connects Campaign and Employee

3. line-chart x field paid_month is absent from the result
   available dimensions: campaign_channel
```

并说明 net_revene 的 aggregation/storage lowering 被第一个问题阻塞，而不追加
一串 unknown-type 级联错误。Code Agent 可以一轮修复三个独立根因。

## 领域级根因诊断

非法比较应报告：

```text
cannot compare OrgKind with EmployeeKind

both use SQL INT storage, but represent different business domains

  analytics/organization.telora:
    organizations.kind declared as OrgKind

  analytics/employee.telora:
    employees.kind declared as EmployeeKind
```

非法 drill 应报告当前 level、目标 level、指标 aggregation/grain、直接拒绝规则和
可用目标。权限阻止时应区分“领域上不存在”与“领域上合法但当前 scope 不允许”。

诊断主位置应是 Code Agent 写下的意图，secondary location 指向 ontology 或
metric rule；低阶 SQL 失败只作为 cause，不应替代领域解释。

## Host 边界

Host 可以提供：

- catalog snapshot 与 revision；
- SQL dialect capability；
- tenant 和 authorization scope；
- query/resource budget；
- 可用数据 snapshot。

Telora 库决定这些事实对领域意图意味着什么。Telora 不连接数据库，也不执行
SQL。Host 消费成功的 `ReportExecutionPlan`，处理数据库不可用、snapshot 过期
等真实世界失败。

## 对 Telora 的检验

这个思想实验可以验证：

1. 类型与普通数据能否共同表达 semantic type、field、measure、grain 和 relation；
2. Array/Dict、递归与有界求值能否完成确定的路径搜索；
3. verification 与 lowering 能否保持普通函数风格；
4. provenance 能否跨 AI 意图、ontology、外部 catalog 和中间 IR；
5. best-effort facts 能否最大化独立诊断并抑制级联；
6. accumulation 能否改善多规则代码而不引入通用 effect system；
7. SQL、ResultSchema 与 RenderTemplate 能否原子一致；
8. LSP/CLI 能否向 Code Agent 暴露候选 measure、dimension 和 drill target；
9. Host entry 能否注入 Context 而不把数据库权限泄漏给 main；
10. 大型领域库是否仍容易阅读、测试和排障。

## 当前不应过早引入的机制

- 不因业务语义身份立即引入完整名义类型系统；
- 不因 ontology capability 立即引入 trait/assoc type；
- 不因多诊断立即引入通用 algebraic effect；
- 不因 SQL/render 输出立即引入模板或 AST 语法魔法；
- 不把 relation path search 放进 Telora 类型检查器。

先验证现有类型、类型元数据、普通数据、transform、best-effort analysis、
provenance 和窄 accumulation 是否足以构造这套领域 compiler。

## 成功标准

智能报表案例成功，不是因为 Telora 能拼出 SQL 字符串，而是因为：

- Code Agent 能用领域词汇表达超出固定模板的组合；
- 领域外组合在 SQL 生成前被拒绝；
- 一轮反馈覆盖多个独立根因；
- 反馈能指出意图、规则和修复候选；
- 成功计划无需 Host 猜测业务含义；
- 领域知识只存在于版本化 Telora 库中；
- 新指标、关系和 drill 主要增加普通库代码，而不是内核特例。

它将三个观察角度统一在一个现实案例中：

```text
可编程配置
    Code Agent 表达具体报表意图

领域规则建模
    analytics 库表达可执行本体与业务语义

意图编译器
    Telora lowering 为 SQL + ResultSchema + RenderTemplate
```
