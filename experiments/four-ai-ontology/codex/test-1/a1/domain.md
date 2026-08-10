# 私有企业题面：物流履约分析

本文件是 clean-room 实验的稳定企业输入。它只能在 AI-3 阶段出现，AI-2 不得
读取，AI-4 只能读取 AI-3 从中整理出的公开意图接口。

## 企业背景

这是一家为多个商家履约的物流企业。Customer 创建 Order，Order 从 Warehouse
发货。Warehouse 位于 Region。每个 Order 选择 Carrier 和 ServiceLevel，并产生
一个或多个 Package。Package 包含 PackageItem，PackageItem 指向 Product，Product
属于 Category。

企业希望把业务报表意图 lowering 为经过验证的 typed execution plan。计划记录
指标、维度和必要关系，但本实验不连接真实数据库。

## 物理模型

共有十一张表：

| Entity | Table | Key fields |
|---|---|---|
| Customer | `customers` | `id`, `tier` |
| Order | `orders` | `id`, `customer_id`, `warehouse_id`, `carrier_id`, `service_level_id`, `created_at` |
| Warehouse | `warehouses` | `id`, `region_id` |
| Region | `regions` | `id`, `name` |
| Carrier | `carriers` | `id`, `name` |
| ServiceLevel | `service_levels` | `id`, `name` |
| Package | `packages` | `id`, `order_id`, `delivered_at` |
| PackageItem | `package_items` | `id`, `package_id`, `product_id`, `quantity` |
| Product | `products` | `id`, `category_id`, `sku` |
| Category | `categories` | `id`, `name` |
| DeliveryEvent | `delivery_events` | `id`, `package_id`, `event_kind`, `occurred_at` |

每条关系需要保留企业自己的物理 mapping，至少包含目标 table 和 join predicate。

## 关系事实

以下方向是 many-to-one，在当前 grain 下安全：

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
DeliveryEvent -> Package
```

以下方向扩张 grain：

```text
Order -> Package
Package -> PackageItem
Package -> DeliveryEvent
```

不存在从 Order 到 Product/Category 的安全路径。除非指标本身位于 PackageItem
grain，否则 Product 或 Category 维度需要显式预聚合/allocation policy；本题不提供
这类策略。

## 指标

### OrdersCreated

- 含义：创建的订单数；
- natural grain：Order；
- semantic type：Count；
- aggregation：Additive；
- expression：`COUNT(orders.id)`；
- 无额外 required entity。

### DeliveredPackages

- 含义：已经送达的包裹数；
- natural grain：Package；
- semantic type：Count；
- aggregation：Additive；
- expression：`COUNT(packages.id) FILTER (WHERE packages.delivered_at IS NOT NULL)`；
- required entity：Order。

### UnitsShipped

- 含义：发出的商品件数；
- natural grain：PackageItem；
- semantic type：Count；
- aggregation：Additive；
- expression：`SUM(package_items.quantity)`；
- required entities：Package、Order。

不同 natural grain 的指标不能自动组合。企业当前没有批准预聚合策略；组合失败
必须产生诊断并阻止发布。

## 维度

| Dimension | Required entity | Expression | Capability status |
|---|---|---|---|
| OrderMonth | Order | `substr(orders.created_at, 1, 7)` | approved |
| CustomerTier | Customer | `customers.tier` | approved |
| OriginRegion | Region | `regions.name` | approved |
| CarrierName | Carrier | `carriers.name` | approved |
| ServiceName | ServiceLevel | `service_levels.name` | approved |
| ProductCategory | Category | `categories.name` | approved, but unsafe from Order grain |
| ProductSku | Product | `products.sku` | approved, but unsafe from Order grain |
| DeliveryException | DeliveryEvent | `delivery_events.event_kind` | intentionally not approved |

`DeliveryException` 必须存在于封闭的 Dimension vocabulary 中，但不能出现在当前
capability catalog。企业还没有定义哪些 event kind 属于业务异常，也没有批准它的
grain policy。

## 企业组合与发布规则

1. 至少选择一个指标；
2. 所有指标和维度必须有获准 capability；
3. 多指标只有 natural grain 相同才能组合；
4. 维度必须能从组合后的 base grain 经安全关系到达；
5. 只能经 fan-out 到达的维度必须被拒绝；
6. 所有独立错误应尽可能在一次 Host recovery 中报告；
7. 不完整或非法候选不能发布执行计划。

最终 typed plan 至少包含：

```text
revision
base entity
measure plans
dimension plans
selected safe relations
read-only declaration
```

revision 固定为 `logistics-ontology-v1`。

## AI-3 可见验收场景

合法场景：

```text
measure: OrdersCreated
dimensions: OrderMonth, CustomerTier, OriginRegion, CarrierName, ServiceName
```

应产生 Order grain 的只读计划，并包含到 Customer、Warehouse/Region、Carrier 和
ServiceLevel 的安全关系。

非法场景：

```text
measure: OrdersCreated
dimensions: ProductCategory, DeliveryException
```

一次 recovery 至少应报告：

- DeliveryException 没有获准 capability；
- ProductCategory 从 Order grain 只能经 fan-out 到达。

## 公开边界要求

AI-3 必须另外发布 `PUBLIC_INTENT.md`，供 AI-4 学习。它可以公开：

- Measure、Dimension 的业务名称和含义；
- compile 入口类型和意图写法；
- 一般性的失败类别；
- 哪些请求需要显式策略而当前不可用。

它不得公开：

- table 或 column 名；
- join predicate；
- 指标 SQL/expression；
- relation catalog 源码；
- eDSL 或企业 plan builder 的实现；
- Host 隐藏验收请求。
