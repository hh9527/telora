# 私有企业题面：物流履约分析

Customer 创建 Order，Order 从 Warehouse 发货，Warehouse 位于 Region。每个
Order 选择 Carrier 和 ServiceLevel，并产生一个或多个 Package。Package 包含
PackageItem，PackageItem 指向 Product，Product 属于 Category。

## 关系与物理映射

下列 many-to-one 方向在当前 grain 下安全：

```text
Order -> Customer
Order -> Warehouse
Warehouse -> Region
Order -> Carrier
Order -> ServiceLevel
Package -> Order
PackageItem -> Package
PackageItem -> Product
Product -> Category
```

下列方向会扩张 grain：

```text
Order -> Package
Package -> PackageItem
```

每个关系携带由企业所有的结构化物理 mapping，至少表达源表、目标表和等值 join
两侧的列引用；不能只保存预渲染的 join predicate String。

## 指标与维度

- `OrdersCreated`：Order grain，`COUNT(orders.id)`，获准。
- `DeliveredPackages`：Package grain，获准，并要求 Order。
- `UnitsShipped`：PackageItem grain，获准，并要求 Package 和 Order。
- `OrderMonth`：要求 Order，获准。
- `CustomerTier`：要求 Customer，获准；它是封闭业务枚举，成员为 `Gold`、
  `Silver`、`Bronze`，稳定外部/物理值分别为同名 String，展示标签分别为
  `Gold customer`、`Silver customer`、`Bronze customer`。
- `OriginRegion`：要求 Region，获准。
- `CarrierName`：要求 Carrier，获准。
- `ServiceName`：要求 ServiceLevel，获准。
- `ProductCategory`：要求 Category，能力获准，但从 Order grain 只能经 fan-out
  到达。
- `DeliveryException`：属于封闭 Dimension vocabulary，但没有获准 capability。

`CustomerTier` 必须用具名、无 payload 的 enum 建模，并利用 eDSL 的封闭枚举值域
能力发布上述稳定值和标签；它只支持 `Eq` 筛选。合法值转换为参数化 String
binding，未知值必须在 lowering 中原子失败。

`OrderMonth`、`OriginRegion`、`ProductCategory` 具有开放 String 值筛选能力，支持
标准 `Eq`、`Ge`、`Le`。
`CarrierName`、`ServiceName` 与 `DeliveryException` 没有筛选能力。筛选仍遵守相同的
grain-safe 路径规则；筛选维度可以不出现在分组结果中。

公共查询面必须从同一份 prepared EnterpriseKnowledge 公开 `CustomerTier` 的封闭
值目录（稳定值与展示标签），使查询设计者无需读取私有模型源码即可发现合法值；
不得在 facade 中手工维护第二份枚举目录。

不同 natural grain 的指标不能自动组合。当前没有预聚合或 allocation policy。

## 物理查询事实

基础数据源是 `orders`。领域模型必须以公共 eDSL 要求的结构化关系表达式表达以下
物理事实，使规范 Plan 自身足以被确定地转换为 SQL，而 transform 不需要反查
模型。下列 SQL 写法定义预期语义，不授权把它们作为预渲染片段直接塞入 Plan：

- `OrdersCreated`：`COUNT(orders.id)`；
- `OrderMonth`：`substr(orders.created_at, 1, 7)`；
- `CustomerTier`：`customers.tier`；
- `OriginRegion`：`regions.name`；
- `CarrierName`：`carriers.name`；
- `ServiceName`：`service_levels.name`。

每条安全关系的物理 mapping 必须足以生成对应的只读 join。领域知识必须声明自己
接受的 QueryBuilder PlanProfile；同一个规范 Plan 经公共 QueryBuilder 的 SQLite
transform 必须产生逐字节相同的 Query，其中所有动态数据位于 bindings。

## 可见验证场景

合法场景：`OrdersCreated`，筛选 2026 年 4 月到 6 月、Gold 客户与华东地区，按
`OrderMonth`、`CustomerTier`、`OriginRegion`、`CarrierName` 和 `ServiceName` 分组，
按订单数降序并以维度升序稳定打破并列，只取前 10 项。它应发布 Order grain 的只读
规范 Plan，完整保留投影、filter、grouping、ordering、limit、到这些实体的安全关系
和物理 mapping，并由公共 QueryBuilder 生成参数化 Query；筛选值和 limit 必须出现在
bindings 中且顺序稳定。

非法场景：`OrdersCreated`，按 `ProductCategory` 和 `DeliveryException` 分组。该请求
必须失败，不发布任何部分计划，也不产生 SQL。具体诊断数量和恢复求值结构不属于领域
模型的验收目标。

枚举验证场景：合法的 `CustomerTier = Gold` 必须产生 String binding `"Gold"`；
动态 JSON 输入中的未知等级 `Diamond` 以及范围操作 `CustomerTier Ge Gold` 必须失败，
不产生部分 Query，诊断应归因到对应筛选的 JSON source provenance。公共目录必须按
声明顺序稳定返回 Gold、Silver、Bronze 及其标签。

最终计划 revision 固定为 `logistics-ontology-v1`。
