# QueryBuilder 设计契约

QueryBuilder 定义稳定、方言中立的 `Plan`，并内置 SQLite 的具体化支持：
`Plan -> Query`。它不了解 ontology、企业实体、指标、维度或查询题面。本实验不
要求 PostgreSQL、MySQL 或其他 SQL 后端，也不要求提前抽象多后端插件协议。

```text
Plan     := 标准算子组成的、可验证的查询计划
Query    := { sql: String, bindings: Array(Val) }
transform_sqlite: Plan -> Query
```

## 标准算子与 PlanProfile

QueryBuilder 必须拥有封闭、可枚举的标准算子 vocabulary，至少覆盖：数据源、列与
绑定值表达式、标量调用、聚合、投影、过滤、等值连接、分组、排序和限制。具体类型
分解由实现选择，但同名算子的语义不得随 profile 或调用方变化。

公共 API 必须能表达 `PlanProfile`：它是标准算子能力的显式子集，并能细化重要的
算子能力，例如允许的 join kind、aggregate function 和 scalar function。profile
是能力声明，不是隐式全局状态，也不得改变算子语义。

SQLite 标准标量 vocabulary 至少包含 `Substr`，严格接受 2 或 3 个参数，分别表达
`substr(value, start)` 与 `substr(value, start, length)`；参数中的动态值必须进入
bindings。`PlanProfile.allow_distinct` 只约束聚合调用的 distinct 标志；为 false 时
验证必须拒绝 distinct aggregate。本轮不引入顶层 `SELECT DISTINCT` 算子。

必须提供纯的验证能力，判断一个 Plan 是否只使用给定 profile 接受的能力。发现
越界算子、非法结构、无效标识符或未绑定值时使用 `fail!`，不发布部分 Query。

## Plan

Plan 至少能完整保留：不透明的 String revision、基础数据源、有序投影、filter、按顺序选择的 join
及其结构化等值条件、grouping、ordering 和 limit。动态数据只能作为有类型绑定值，
不能预先 escape 或拼入 SQL。SQL 标识符与用户值必须在类型和 lowering 路径上分开。

Plan 可以参数化企业拥有的封闭类型，但公共边界不得退化为 `Any`、`Dyn` 或以
String 充当所有语义身份。不得接受预渲染的 SELECT/JOIN/GROUP BY 片段作为表达式。

## Query 与确定性转换

公共 `Query` 的概念形状为：

```telora
type Query = struct {
  sql: String,
  bindings: Array(Val),
};
```

本实验的 `Val` 只需支持 String、Int、Float 和 Bool，不含 Bytes。所有运行时值都
进入 bindings；transform 只生成目标方言的占位符，不承担字符串 escape。

SQLite transform 必须纯且确定：同一个合法 Plan 逐字节产生相同 SQL 与相同顺序的
bindings。它必须使用 SQLite 可接受的标识符、占位符和算子表达，保留投影、join、
grouping、ordering 的规范顺序。非法、profile 不支持或 SQLite 无法具体化的 Plan
通过 `fail!` 失败，不产生部分 Query。

## 公共交付

公共教程和契约必须足以让一个不了解实现的 eDSL 作者：

1. 声明其接受的 PlanProfile；
2. 使用标准算子构造 Plan；
3. 验证 `operators(plan) <= profile`；
4. 把 Plan 确定具体化为 SQLite Query；
5. 判断失败来自 profile 越界、Plan 结构还是 Query 转换。

API 名称、模块布局和内部算法不是本契约的一部分。

公共文档必须明确区分 import 边界：package 内部可以使用 `@src/...`，外部依赖者必须
使用 manifest 中的依赖名（例如 `query-builder/lib.telora`），不得让外部作者照抄
package 私有的 `@src` 路径。
