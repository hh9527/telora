# 本体 eDSL 设计契约

本 eDSL 让企业以 nominal types 与 typed properties 表达 `EnterpriseKnowledge`，并在
module/tool stage 把这些 metadata 一次性编译为稳定的 lowering：

```text
nominal entity types + member/type properties
  -> validated ontology root
  -> Fn(Request) -> Plan
```

`Plan`、标准算子和 `PlanProfile` 由 QueryBuilder 所有。eDSL 不定义替代 Plan，不负责
`Plan -> Query`，也不包含任何企业领域事实。

## EnterpriseKnowledge

EnterpriseKnowledge 的输入至少表达：

- 不透明的 String revision 与封闭的领域 vocabulary；
- 实体、属性、指标、维度和关系等 ontology 事实；
- 能力目录、授权和每项能力所需的有类型输入；
- 基础数据源、结构化物理表达式与结构化关系 mapping；
- 安全关系与会扩张 grain 的关系；
- 此知识接受的 QueryBuilder `PlanProfile`；
- 将能力、关系和物理事实 lowering 为标准 Plan 算子的规则。

企业实体使用具名 Struct 表达；nominal TypeId 是实体的内部身份，canonical member
index 是字段的内部身份。物理列、key、relation、measure 与 dimension 等局部事实使用
type/member typed property 就近表达，不得再建立一份语义相同、仅靠 EntityId、
AttributeId 或 String 连接的平行事实目录。同一字段上的多个标注可以通过
`Option(previous)` fold 合并为一个 property；type provider 可以在所有 member
property 完成后读取封闭 snapshot。

Ontology root 必须显式列出当前知识包含的 entity types，不依赖全局扫描。root provider
负责收集 property、验证关系图和能力，并发布一个 prepared knowledge payload。该
payload 可以包含普通 closure；跨模块 consumer 必须复用同一份已发布行为。

对外 JSON intent 仍使用稳定、封闭的 measure/dimension 业务 vocabulary。该 vocabulary
是交换协议，不得用进程内 TypeId 代替；但也不得为了它再复制实体、字段和关系结构。

Knowledge 不能假定全企业只有一个 base entity。每项 measure 在自己的 natural grain
上登记，一次请求由 measure 决定 base grain；多个 measure 必须 grain 兼容，否则明确
失败。维度表达式既能引用属性，也能使用 `PlanProfile` 允许的标准标量算子组成受控计算
表达式；例如 `Substr(attribute, 1, 7)` 的字面量必须 lower 为绑定值，不能保存预渲染 SQL。

企业知识不得使用预渲染 SQL、任意 builder 回调或 String 反查来绕过公共规则。
`PlanProfile` 只收窄标准算子能力，不能改变任何算子的语义。

## Request 与 lowering

Request 是查询方提交的有类型意图。参考形状为：

```telora
type Request(Id, Subject, Input) = struct {
  id: Id,
  subject: Subject,
  input: Input,
};

type FilterOp = enum { 'Eq, 'Ge, 'Le };
type FilterRequest(DimensionId, Subject, FilterInput) = struct {
  id: DimensionId,
  subject: Subject,
  op: FilterOp,
  input: FilterInput,
};

type OrderTarget(MeasureId, DimensionId) = enum {
  'Measure(MeasureId),
  'Dimension(DimensionId),
};
type OrderRequest(MeasureId, DimensionId) = struct {
  target: OrderTarget(MeasureId, DimensionId),
  direction: OrderDirection,
};

type QueryRequest(MeasureId, DimensionId, Subject, MeasureInput, DimensionInput) = struct {
  measures: Array(Request(MeasureId, Subject, MeasureInput)),
  dimensions: Array(Request(DimensionId, Subject, DimensionInput)),
  filters: Array(FilterRequest(DimensionId, Subject, FilterInput)),
  ordering: Array(OrderRequest(MeasureId, DimensionId)),
  limit: Option(Int),
};
```

请求只能表达“要什么”，不能携带领域表达式、mapping、Plan 节点或 SQL。measure 与
dimension 保持原始顺序；原始 subject 必须进入相关诊断。`filters` 是有序的维度
筛选，每项携带封闭 DimensionId、Subject、领域自定义的有类型输入和标准比较操作
`Eq` / `Ge` / `Le`；企业知识负责把输入转换为 QueryBuilder `Val`。筛选维度不必
同时投影，但仍须授权并选择 grain-safe 路径。多个筛选按请求顺序用标准 `And`
组合，所有值进入 bindings。普通和 computed dimension 都可以显式声明筛选
capability；computed dimension 的筛选必须复用同一规范计算表达式。未显式声明筛选
capability 的维度必须拒绝 filter，不能把任意标量输入视作默认授权。

`ordering` 只能以 `Measure(id)` / `Dimension(id)` 引用本请求已经选择的投影，并
指定 `Asc` / `Desc`；不得接受任意表达式。`limit` 是可选正整数，表达 Top N；非正
整数必须诊断失败。筛选值和 limit 都必须由 QueryBuilder 参数化，不能写入 SQL 文本。

prepared lowering 收到 request 后必须：

1. 独立解析并授权每项能力；
2. 验证指标 grain 兼容性；
3. 推导维度所需实体并选择安全关系；
4. 组装覆盖所有请求且没有额外请求的标准 Plan，包括筛选所需安全路径、稳定排序和 Top N；
5. 验证 Plan 只使用 ontology root 的 `plan_profile` 接受的能力；
6. 成功时发布 Plan，失败时通过 `fail!` 记录诊断且不发布部分 Plan。

公共 API 不返回 Rejection、诊断数组或逐请求 Evidence。可恢复诊断由 Telora Host
机制负责；eDSL 作者只按正常表达式语义使用 `fail!`。

读取 TypeDesc、member property、建立索引、验证关系图和准备安全路径属于一次性准备
阶段。每次业务 lowering 不得重新枚举完整 metadata、重建关系图或重新执行 BFS；热路径
只消费 prepared payload 中的规范索引、路径与 closure。

## 关系与路径

关系目录区分 grain-safe 和 fan-out。每条关系保留有类型端点和企业拥有的结构化
mapping。安全路径选择遵循：最短边数优先，同长度按目录索引序列字典序最小；多个
目标按请求顺序合并，共享边只保留首次出现。遍历必须对有向环稳定，最大深度为八。

完整可达性使用 safe 与 fan-out 的并集。目标恰好分类为 safe、fan-out-only 或
missing。恰好八条边可接受；只有仍存在未访问后继且边界可能隐藏可达性时才产生
truncation。任何不安全、缺失、未授权、grain 冲突或 profile 越界都阻止 Plan 发布。

## Plan 组装边界

eDSL 使用 QueryBuilder 的公共标准算子构造 Plan。Plan 至少保留 ontology revision、
基础数据源、有序 measure/dimension 投影、与维度一致的 grouping、有序 filter、
ordering、limit，以及按规范顺序选择的关系和 mapping。每个成功请求恰有一个对应
投影，每个非基点投影或筛选需求都有路径覆盖。排序表达式只能从已解析的投影构造。

企业不能提供一个“最终 Plan builder”替 eDSL 手工完成组装；eDSL 也不能根据 String
label 反查领域数据。Plan 完成后必须调用 QueryBuilder 的 profile 验证。eDSL 的成功
示例可以继续调用 QueryBuilder 的 SQLite transform 展示端到端 Query，但该转换不是
eDSL API，也不得把 SQLite 细节写回方言中立 Plan 的语义。

## 公共边界

公共 Request、Plan、Query、业务 vocabulary 和 lowering contract 必须保持精确类型，
不得使用 `Any`、`Dyn` 或 native 声明擦除边界。Tool-stage/provider 内部可以受控使用
TypeDesc、typed-property query 和 Dyn；最终 hot-path closure 不应重新动态解释完整
metadata。API 名称、property carrier 的拆分、trait 使用、模块布局、图算法和内部索引
由实现选择，但公共教程必须足以让企业作者在不读源码的情况下建立知识。
