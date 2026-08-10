# EDSL_TUTORIAL.md — 用 ontology eDSL 定义你的企业本体

面向：企业模型作者（A3）。本文教你如何用 `a2/ontology-edsl/` 下的库，把你自己知道的
企业知识表达为可编译的本体，并把行业级验证与发布编排交给库。

本文不包含任何企业事实——示例是领域中立的抽象玩具域（颜色节点），请照葫芦画瓢换成你的模型。

## 0. 前置要求：导入方式（重要）

Telora 的推断对"选择性导入"敏感：**必须用命名空间导入**，否则库里的泛型函数调用会退化为
`Any` 导致 check 失败。

```telora
import "../../a2/ontology-edsl/compiler.telora" as compiler;
import "../../a2/ontology-edsl/types.telora"    as types;
import "std/array" as array;
```

（你可以在自己的 `telora-deps.json` 里声明 `ontology-edsl` 依赖路径，再 `import "ontology-edsl/compiler.telora" as compiler;`。）

另一个重要前提：**身份类型（请求 id）和实体类型必须支持内建 `==`**。库内部用 `idOf(cap) == request`
查找能力、用 `==` 计算可达集合。枚举/原子类型天然支持 `==`。

## 1. 总览：你定义什么，库保证什么

你（企业模型）只提供：封闭类型、能力目录、关系事实、组合策略（`requirementsOf`）、
允许列表、以及最终计划构建器。库承担：独立降级、部分结果收集、编译完整性、需求派生、
有界路径闭包与分类、fan-out/缺失路径/未授权诊断、原子发布门。

入口只有一个：`compiler.compile_with(...)`。

## 2. 语义角色（可选但推荐）

`types.telora` 提供三个 TypeMetadata 构造器，给你的记录贴上"语义角色"。字段语义由库定义，
具体类型参数由你提供；`compile_with` 不直接读这些字段，它通过你的 typed 选择器工作，
所以你既可以用这些构造器做注解，也可以用普通 `@struct` + 选择器（见下例）。

- `MeasureDefinition(Id, Value, Granularity, Aggregation, Input, Output)` → 字段 `id / valueType / granularity / aggregation / lower: Fn(Id, Input) -> Option(Output)`
- `DimensionDefinition(Id, Input, Payload)` → 字段 `id / lower: Fn(Id, Input) -> Option(Payload)`
- `RelationDefinition(From, To, Cardinality, Mapping)` → 字段 `from / to / cardinality / mapping`

## 3. 你需要定义的封闭类型

```telora
@enum type Entity = { Red: 'None, Blue: 'None, Green: 'None };   # 实体必须支持内建 ==
@enum type ReqId  = { CountRed: 'None, CountBlue: 'None };       # 身份必须支持内建 ==
@enum type Unit   = { U: 'None };                                # 空输入类型

@struct type Capability = {
    id: ReqId,
    lower: Fn(ReqId, Unit) -> Option(Int),     # 你的能力函数
};

@enum type Requirement = { At(Entity): 'None };                  # 关系需求（带目标实体）

@struct type Relation = { from: Entity, to: Entity, mapping: String };   # mapping = 你的物理映射载荷

@struct type Plan = { text: String };                            # 你的惰性执行计划
```

要点：
- `ReqId` 和 `Entity` 必须能用 `==` 比较（枚举即可）。
- `Capability` 的 `lower` 接受**请求的身份**（意图），不是目录定义本身——产物的来源要指向意图。
- `Relation.mapping` 是你的物理载荷（表/列/连接提示），库原样携带并交还给 `buildPlan`。

## 4. 能力目录与 typed 选择器

```telora
def countRedCap: Capability = {
    id: 'CountRed,
    lower: fn(request, _input) {
        match request { 'CountRed => 'Some(10), 'CountBlue => 'None }
    },
};
def countBlueCap: Capability = {
    id: 'CountBlue,
    lower: fn(request, _input) {
        match request { 'CountBlue => 'Some(20), 'CountRed => 'None }
    },
};

def capabilities: Array(Capability) = [countRedCap, countBlueCap];

def capId: Fn(Capability) -> ReqId = fn(cap) { cap.id };
def capLower: Fn(Capability, ReqId, Unit) -> Option(Int) = fn(cap, request, input) { cap.lower(request, input) };
```

## 5. 需求派生与组合（`requirementsOf`）

`requirementsOf: Fn(Array(Output)) -> Array(Requirement)` 接收**全部已完成的能力输出**，
返回关系需求列表。这是你的"组合 + 派生"点：把度量片段、维度载荷、以及你企业自己的组合逻辑
都放进这里。

```telora
def requirementsOf: Fn(Array(Int)) -> Array(Requirement) = fn(_values) {
    ['At('Blue)]
};
```

注意：旧设计里的"独立组合步骤"已并入 `requirementsOf`（对整批完成值工作）与 `buildPlan`。

## 6. 目标 / 诊断主体 / 相等选择器

```telora
def targetOf: Fn(Requirement) -> Entity = fn(req) {
    match req { 'At(entity) => entity }
};
def subjectOf: Fn(Requirement) -> Entity = targetOf;   # 诊断主体（当前实现未使用，见 STAGE2_NOTES）
def entityEq: Fn(Entity, Entity) -> Bool = fn(a, b) { a == b };   # 供诊断成员判断用
```

## 7. 关系事实：安全目录与 fan-out 目录

把关系分成两个目录，**同一条关系只出现在一个目录**：
- `safeEdges`：不改变粒度的连接（同粒度连通）。
- `fanOutEdges`：扩张粒度（一对多方向）。

```telora
def safeRelations: Array(Relation) = [
    { from: 'Red, to: 'Blue, mapping: "safe-hint" },
];
def fanOutRelations: Array(Relation) = [
    { from: 'Blue, to: 'Green, mapping: "fanout-hint" },
];
def relationFrom: Fn(Relation) -> Entity = fn(r) { r.from };
def relationTo: Fn(Relation) -> Entity = fn(r) { r.to };
```

## 8. 允许列表

```telora
def allowedEntities: Array(Entity) = ['Red, 'Blue];   # 授权实体；目标不在其中会被诊断并阻塞发布
```

## 9. 最终计划构建器（`buildPlan`）

`buildPlan: Fn(Array(Output), Array(Requirement), Array(Relation)) -> Option(Plan)`
- 第 1 参：完成的能力值；第 2 参：全部需求；第 3 参：**joins**（安全目录，含你的物理映射）。
- 返回 `'Some(plan)` 表示成功；`'None` 时请用 `emit_error!` 说明原因（会被收集，并阻塞发布）。
- 计划是**惰性产物**（如 SQL 描述、结果模式、渲染绑定），不是执行授权。

```telora
def buildPlan: Fn(Array(Int), Array(Requirement), Array(Relation)) -> Option(Plan) = fn(values, requirements, joins) {
    'Some({ text: "compiled plan" })
};
```

## 10. 调用管线（唯一入口）

```telora
def result: Option(Plan) = compiler.compile_with(
    ['CountRed, 'CountBlue],       # requests
    capabilities,                  # capabilities
    capId, capLower, 'U,           # idOf, lower, input
    requirementsOf, targetOf, 'Red, subjectOf,   # requirementsOf, targetOf, base, subjectOf
    allowedEntities,               # allowed
    safeRelations, fanOutRelations,               # safeEdges, fanOutEdges
    relationFrom, relationTo, entityEq,           # fromOf, toOf, eq
    buildPlan
);
```

## 11. 语义与诊断（库的行为）

- **独立降级**：每个请求独立执行；缺失能力 → `emit_error!("no capability {request}", request)`，不短路其他请求。
- **完整性**：`compileRequested` 只有当所有请求都产生值时 `complete` 才是 `'Some`。
- **路径分类**：`classifyPathsNoEq` 从 `base` 出发做有界闭包（6 轮，见 STAGE2_NOTES），
  一次派生三结果：joins（安全边）、fanOutOnly（仅 fan-out 可达）、unreachable（都不可达）。
- **诊断**：`diagnoseTargets` 对每个目标落在 fanOutOnly/unreachable 的需求，以
  **需求值 + 目标实体**为主体报诊断；`validateAllowlist` 对未授权目标报诊断。
- **发布门**：`compilationComplete(complete) && array.length(unauthorized) == 0` 时调用
  `buildPlan`，否则返回 `'None`。诊断与候选构建都发生在门判定**之前**，不会被门掩盖。

## 12. 扩展点清单（你要提供的）

1. 封闭类型：`ReqId`/`Entity`（支持 `==`）、`Cap`、`Input`、`Output`、`Requirement`、`Relation`、`Plan`；
2. 能力目录 + `idOf` + `lower`（lower 收到请求身份）；
3. `requirementsOf`（组合 + 派生需求）；
4. `targetOf`（+ `subjectOf`）；
5. `base` 基准实体、`allowed` 允许列表；
6. 安全/fan-out 关系目录 + `fromOf`/`toOf`；
7. `eq` 实体相等选择器；
8. `buildPlan` 最终构建器。

**不要**重写：能力查找、独立降级、完整性、需求聚合、图闭包、路径分类、维度路径诊断、允许列表校验、原子发布门。
