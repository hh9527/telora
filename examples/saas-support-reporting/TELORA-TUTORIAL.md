# 完成本挑战所需的最小 Telora 教程

这不是完整语言手册，只覆盖本题需要的表面语法和共享 DSL 契约。

## 文件与 import

依赖文件使用 JSON：

```json
{"dependencies":{"analytics-ontology":{"path":"../analytics-ontology"},"ontology-method":{"path":"../ontology-method"}}}
```

模块导入与导出：

```telora
import "std/array" as array;
import "ontology-method/types.telora" as types;
import "analytics-ontology/compiler.telora" as analytics;

export def compile: Fn(Input) -> Output = fn(input) { ... };
export { Entity, Measure, Dimension, ExecutionPlan };
```

相对模块使用 `import "./model.telora" { compile };`。

## 封闭类型

enum variant 使用单引号值：

```telora
type Entity = enum {
    'Ticket,
    'Agent,
};

let ticket: Entity = 'Ticket;
```

record 类型与值：

```telora
type Item = struct { name: String, nodes: Array(Entity) };
let item: Item = {name: "x", nodes: ['Ticket]};
```

函数签名写作 `Fn(A, B) -> C`，泛型写作：

```telora
def identity: for(A) Fn(A) -> A = fn(value) { value };
```

## Option、match 与诊断

```telora
match value {
    'Some(found) => found,
    'None => fallback,
}
```

领域规则拒绝一个候选时，可通过 typed checker 产生 Warning 并返回 None：

```telora
def reject: Fn(Value) -> Result(Output, String) =
    fn(authored_value) { 'Err("message") };
let output = reject.should_ok!(authored_value);
```

Host recovery 会收集独立执行路径上的诊断。不要为了继续执行而构造假的计划。

## 数组

本题主要使用：

```telora
array.find(values, predicate)
array.all(values, predicate)
array.map(values, mapper)
array.flat_map(values, mapper)
array.concat([left, right])
```

当泛型结果难以推导时，先定义一个带完整签名的辅助函数，或给局部 binding 写
具体类型。

## TypeMetadata definition families

阅读以下共享文件是允许且必要的：

```text
examples/ontology-method/src/types.telora
examples/analytics-ontology/src/compiler.telora
```

关系、指标 capability 和维度 capability 应通过共享 metadata family 产生：

```telora
type Relation = types.RelationDefinition(Entity, Cardinality, RelationMapping);

type MeasureCapability = types.MeasureDefinition(
    Measure,
    Entity,
    SemanticValueType,
    Aggregation,
    Alignment,
    MeasurePlan,
);

type DimensionCapability = types.DimensionDefinition(
    Dimension,
    Array(Measure),
    DimensionPlan,
);
```

`MeasureDefinition` 要求字段：`id`、`value_type`、`natural_grain`、
`aggregation`、`lower`。`DimensionDefinition` 要求 `id` 和 `lower`。
`RelationDefinition` 要求 `from`、`to`、`cardinality` 和 `mapping`。

## Capability lowerer

一个 measure capability 的典型形状：

```telora
def example: MeasureCapability = {
    id: 'SomeMeasure,
    value_type: 'Count,
    natural_grain: 'Ticket,
    aggregation: 'Additive,
    lower: fn(requested, alignment) {
        'Some({
            source: requested,
            base_entity: 'Ticket,
            required_entities: [],
            expression: "COUNT(tickets.id)",
        })
    },
};
```

维度 capability 的 lowerer 接受请求 id 和当前选中的 Measure 数组，并返回
`Option(DimensionPlan)`。

## `analytics.compile_with`

完整签名以共享源文件为准。调用参数按以下顺序分组：

```text
requested measures
measure capabilities / id selector / lower adapter / lower input

requested dimensions
dimension capabilities / id selector / lower adapter / lower input

combine_measures
combined base selector
combined required-node selector
dimension required-node selector
dimension diagnostic-subject selector
extra required nodes

safe relations / fan-out relations / from selector / to selector
final plan builder
```

`combine_measures` 应返回一个企业自定义的 typed `CombinedMeasure`。最终 builder
接收：

```text
CombinedMeasure, Array(DimensionPlan), Array(Relation)
```

并返回 `Option(ExecutionPlan)`。

最常见的 adapter 是：

```telora
fn(capability) { capability.id }
fn(capability, requested, input) { capability.lower(requested, input) }
fn(dimension) { dimension.required_entity }
fn(dimension) { dimension.source }
fn(relation) { relation.from }
fn(relation) { relation.to }
```

这些 adapter 可以保留。不要用类型擦除来减少它们。

## 顶层验证文件

```telora
import "./model.telora" { compile };

export def output = compile(['SomeMeasure], ['SomeDimension]);
```

本实验由主实验者运行。你只负责写好代码并报告预期风险。
