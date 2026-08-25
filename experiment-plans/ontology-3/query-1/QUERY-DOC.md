# Ent-1 查询能力

你只需把业务问题写成题号对应的答案 JSON，再执行
`just a5 make-query <problem-id>`。成功时命令输出
规范化后的 `intent` 以及参数化的 `query.sql` 和 `query.bindings`；失败时输出带来源的
Telora 诊断。不要删除必要条件来换取成功。

A4 使用同一形状的 `intent-1/intent.json` 和 `intent-1/invalid/<name>.json`，但通过
`just a4 ...` 验收接口；A5 不应使用 A4 的命令或文件。

## 完整 JSON 形状

```json
{
  "measures": ["DeliveredPackages"],
  "dimensions": ["CarrierName"],
  "filters": [
    {
      "dimension": "OrderMonth",
      "op": "Eq",
      "value": {"String": "2026-07"}
    },
    {
      "dimension": "OriginRegion",
      "op": "Eq",
      "value": {"String": "East China"}
    }
  ],
  "ordering": [
    {"target": {"Measure": "DeliveredPackages"}, "direction": "Desc"},
    {"target": {"Dimension": "CarrierName"}, "direction": "Asc"}
  ],
  "limit": 5
}
```

请求必须至少包含一个指标。指标和分组维度直接写公共业务名称；筛选值写作
`{"String":"具体值"}`。没有筛选或排序时写空数组；没有条数限制时 `limit` 写
`null`，否则必须写正整数。查询发起者不属于当前 Request 契约，不要把题面给出的
发起者写入 JSON，也不要把它臆造为业务筛选。

## 可查询指标

- `OrdersCreated`：已创建订单数；每个订单计一次。
- `DeliveredPackages`：已送达包裹数；每个包裹计一次。
- `UnitsShipped`：已发货商品件数；每个包裹货品行计一次。

一个请求中的指标必须使用同一种计数单位。

## 可分组维度

- `OrderMonth`：订单创建的日历月份。
- `CustomerTier`：下单客户的等级。
- `OriginRegion`：订单发货仓库所在的地区。
- `CarrierName`：承运商名称。
- `ServiceName`：服务水平名称。
- `ProductCategory`：商品类别。

`OrderMonth`、`CustomerTier`、`OriginRegion`、`CarrierName` 和 `ServiceName` 可与上述
三种指标安全组合。`ProductCategory` 可与 `UnitsShipped` 安全组合；订单或包裹可能
包含多个商品类别，因此它不能与 `OrdersCreated` 或 `DeliveredPackages` 安全组合。

`DeliveryException` 是可理解的业务概念，但当前没有对应的查询或筛选能力。

## 筛选

只支持以下筛选维度，值均为字符串：

- `OrderMonth`：例如 `{"String":"2026-07"}`。
- `CustomerTier`：例如 `{"String":"Gold"}`。
- `OriginRegion`：地区值使用查询能力定义的规范文本。目前题面使用的规范值包括
  `{"String":"East China"}`（华东）、`{"String":"South China"}`（华南）和
  `{"String":"North China"}`（华北）。如果用户给出的地区没有对应规范值，先询问，
  不要自行翻译或类推。
- `ProductCategory`：例如 `{"String":"电子产品"}`。

操作符为 `Eq`、`Ge`、`Le`。多个筛选按数组顺序以 AND 组合；一个维度可同时用
`Ge` 和 `Le` 表达闭区间。筛选维度不必同时出现在 `dimensions` 中。
`CarrierName` 和 `ServiceName` 目前只能分组，不能筛选。

## 排序与 Top N

排序目标必须是本次请求中已经出现的指标或分组维度：

- 指标：`{"Measure":"DeliveredPackages"}`。
- 维度：`{"Dimension":"CarrierName"}`。

方向为 `Asc` 或 `Desc`。`ordering` 数组顺序就是优先级：第一项是主排序键，后续项
依次用于打破并列。需要稳定的 Top N 时，先按指标降序，再按题面要求的名称或其他
维度升序，并把 N 写入 `limit`。筛选值和 limit 都会出现在 bindings 中。

## 决策边界

题面缺少月份、地区、截止时间等必要值时，先向用户询问，不要猜。题面中的“履约量”、
“表现最好”、“最重要”等词可能对应不同计数单位时，先用业务语言列出候选含义并请用户
确认，不要先运行某个猜测。

如果题面允许明确的替代口径，可以先验证首选口径，并在诊断表明它不安全时使用题面允许的
替代口径。若题面禁止替代，或所需筛选能力不存在，应保留全部业务条件，并根据最终 Telora
诊断说明为什么当前不能得到查询计划。
