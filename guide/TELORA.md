# Telora 语言教程

本文面向第一次使用 Telora 的程序作者和库作者，说明当前已经实现并经过验证的
公开语言表面。完整语义以 [`../docs/design/LANGUAGE.md`](../docs/design/LANGUAGE.md)
为准；本文没有说明的行为不能据此推断为存在。

Telora 是一门确定、纯、面向表达式的语言，用于把意图编译为不可变计划。
值不可变，模块显式声明，TypeMetadata 是普通数据，并由执行程序代码的同一个
VM 求值。

最小 crate 由 `telora-deps.json` 和入口模块组成：

```text
hello/
  telora-deps.json
  src/bin/main.telora
```

```json
{"dependencies":{}}
```

```telora
# src/bin/main.telora；# 引入行注释
export def output: String = "hello, telora";
```

在 crate 目录运行：

```bash
telora check @bin/main.telora
telora run main
telora query exports @bin/main.telora
```

`check` 检查并求值模块导出，`run` 通过 Entry 调度入口，`query` 以 JSONL 查询
模块事实。后文会进一步解释三者的边界。

除非示例明确展示完整模块，后文单独出现的 `let` 代码均表示函数体或 `do` block
中的局部片段，不能直接放在模块顶层。

## 值与绑定

```telora
42                         # Int
3.5                        # Float
1e6                        # 带十进制指数的 Float
"text"                     # String
b"bytes"                   # Bytes
'Ready                     # Atom
'True                      # Bool 是封闭的 Atom 类型
'Some(1)                   # 带标签的值
(1, "one")                # Tuple 值
[1, 2, 3]                  # Array
{name: "Ada", active: 'True} # record/Dict 值
```

Bool 值是 `'True` 和 `'False`；Telora 不进行 truthiness 转换。Float 是有限的
IEEE 754 binary64。Float 字面量接受小数点形式（`3.5`）和指数形式（`1e6`、
`1.25e-3`）。NaN、正无穷和负无穷都不是 Telora 值。

```telora
def answer = 40 + 2;
def increment: Fn(Int) -> Int = fn(value) { value + 1 };
```

`let`、`def`、`type`、`for`、`fn`、`match`、`native`、`decl`、`import` 和
`export` 是语言保留字。`_` 用作模式或显式类型实参占位符；闭包参数使用具名
标识符。

## 运算符与控制流

比较运算符为：

```telora
left == right
left != right
left < right
left > right
left <= right
left >= right
```

相等和不等要求两侧具有同一静态语义类型；已知类型或形状不兼容时会在前端报错，
而不是返回 False。普通复合值保持结构相等语义，两个具名 struct/enum 值还要求
相同的名义类型。dict、Atom 或 Tagged 字面量可以从另一侧获得 exact nominal
context，例如 `wrapper == 'Box("x")` 和 `'Box("x") == wrapper`；不需要先单独
标注字面量。显式 Any 或 Union 边界内的不同运行时 variant 可以比较并返回 False。
有序比较只接受类型相同的
`Int`、`Float` 或 `String` 操作数；不存在混合数值强制转换。String 按其内部
UTF-8 字节序列精确地进行字典序比较，不做规范化，不使用 locale 规则、大小写
折叠或自然数排序。

`%` 接受类型相同的 Int 或 Float 操作数，与 `*` 和 `/` 具有相同优先级和左
结合性，并使用截断余数语义。非零结果与左操作数同号：`-7 % 3 == -1`，且
`7 % -3 == 1`。Int `% 0` 以 `DivisionByZero` 失败。

Float 的 `+`、`-`、`*`、`/` 和 `%` 必须产生有限 Float。产生 NaN 或无穷时，
抛出等价于 `fail!("NonFiniteFloat", left, right)` 的带来源 blame。这包括 Float
除以正零或负零、Float 对任一零求余，以及算术溢出。操作数按源码顺序各求值
一次。有限 Float 比较遵循普通数值语义，并且 `-0.0 == 0.0`。

前缀 `!` 对 Bool 返回相反的规范 Bool，对 Int 返回按位补。二元 `&`、`^` 和
`|` 只接受 Int。从紧到松的优先级依次为：一元运算、`*`/`/`/`%`、`+`/`-`、
`&`、`^`、`|`、比较、`&&`、`||`。

六种比较共享一个不可结合的优先级层。需要比较某个比较结果时必须写括号；
`a < b <= c` 不是链式比较。`&&` 和 `||` 接受 Bool，并执行短路求值。
`if` 表达式始终具有 `else` 分支。`ctrl_block` 可以是普通 block、`if`、
`if let`、`match` 或 `return expression;`。`if` 和 `if let` 的 `else` 接受
`ctrl_block`；非 block 形式会被规范化为包含该控制流表达式的 block。因此可以
连续写 `else if`：

```telora
if score >= 90 { 'Excellent }
else if score >= 60 { 'Pass }
else { 'Fail }
```

```telora
if ready { value }
else if let 'Some(cached) = candidate { cached }
else match fallback { 'Some(value) => value, 'None => default }
```

也可以直接把提前返回用作分支：

```telora
if ready { value } else return fallback;
```

## 函数与契约

```telora
fn(value) { value + 1 }

def identity: for(A) Fn(A) -> A = fn(value) { value };
def map_pair: Fn(Int, String) -> Tuple([String, Int]) =
    fn(number, text) { (text, number) };
```

函数类型写作 `Fn(P1, ..., Pn) -> R`。普通 TypeMetadata 构造器调用写作
`Func([P1, ..., Pn], R)`：

```telora
type Unary = Func([Int], String);
```

`Fn(Int) -> String` 与 `Func([Int], String)` 产生相同的规范函数元数据。

泛型调用默认推断类型实参，也可以使用显式的 `@[...]` 应用：

```telora
identity@[Int](1)
pair@[Int, _](1, "text")
```

`_` 表示由完整调用上下文推断该类型实参。没有标记的 `value[index]` 只表示
Array 索引。

推断会综合完整泛型调用中的证据。当另一个实参能够确定外围 enum 时，单独一个
封闭 Atom 实参不会过早地把共享参数固定为其 singleton 类型。例如，`'Base`
实参和 `Array(NodeId)` 实参可以共同推断出 `NodeId`。只有完整调用仍确实存在
歧义或约束不足时，才使用显式 `@[...]`。

匿名 Struct 实参同样参与完整调用上下文。泛型回调结果可以拓宽较早 seed 中的
singleton Atom 字段和空集合字段。因此，下列 fold 会直接推断出 `flag: Bool`
和 `items: Array(Int)`：

```telora
array.fold([1, 2, 3], {flag: 'False, items: []}, fn(state, item) {
    {flag: item > 1 || state.flag, items: array.push(state.items, item)}
})
```

当回调分支返回多个结构对应的 Struct variant 时，它们的 union 会保留字段间
关系。seed 会根据唯一兼容的 variant 完成推断；无关 Atom 或存在歧义的完成方式
仍然报错。

由闭包初始化且没有标注的局部绑定，可以从后续泛型调用中获得预期函数类型。
闭包分支产生的 variant union 在每个 variant 和 payload 均兼容时，会细化为
预期的封闭 enum。例如，`'None | 'Some(String)` 会细化为 `Option(String)`。
未知 variant 和不兼容 payload 仍然报错。

Telora 在拓宽结果之前合并分支证据：泛型代码中的 `if` 若为同一个预期 enum
结果贡献不同的窄 variant，会先 join 它们，再拓宽为该 enum。例如，有类型的
`Array(Option(Output))` fold 可以在一个分支 push `'None`，在另一个分支 push
`'Some(output)`。当回调仍然约束不足时，带有完整契约的具名辅助函数依然有用。

## Struct、enum 与模式

```telora
type Entity = enum {
    'Ticket,
    'Agent,
};

type Requirement = struct {
    target: Entity,
    reason: String,
};
```

Struct 和 enum 都是封闭的具名声明。不同声明即使结构相同也不是同一个类型；alias、
import 和 reexport 保留原声明身份。字段使用 `.field`；enum 值使用 Atom 或 Tagged
语法。`struct` 和 `enum` 只用于 `type` 的直接初始化，不能作为普通函数调用；
`@struct`、`@enum` 不是可用的兼容语法。

声明上下文中的记录或 tag 字面量会取得预期类型的声明身份。外部 JSON/TOML/YAML
数据可以在 `codec.decode` 或 `validate(Type, raw)` 这类有精确 witness 的边界取得
身份。已经产生的匿名记录或另一个声明类型的值，不能只因结构相同而在后续标注、
参数或返回值边界被重新标记；应在字面量的产生点给出声明契约。

静态约束、checked cast 和动态投影是三种不同能力：

```telora
let empty = [].ty!(Array(Int));       # 只协助静态推断，无运行时调用
let truth = 'True.ty!(Bool);

let user_result = raw.cast!(User);   # Result(User, String)

import "std/dyn" as dyn;
let projected = dyn.project@[User](package); # Option(User)
```

`ty!` 必须能在编译期证明目标类型，不能从 `Any` 或 `Dyn` 恢复类型。普通赋值只允许
`T -> Any`，不允许未经检查的 `Any -> T`。`cast!` 只验证表示并保留原数据图：raw
Dict/Atom 可以在完整匹配时取得目标 witness，但两个不同具名类型不能按结构互转；
String parse、Int/Float 转换、`Value -> model`、rename/default/flatten 都属于 codec，
不属于 cast。Dyn 投影只在打包时的 canonical 类型与目标完全相同时成功，不做结构猜测。

```telora
match result {
    'Some(value) => value,
    'None => fallback,
}

match pair {
    (left, right) => left,
}
```

对已知封闭 variant 的 match 必须穷尽，或者包含 catch-all。`_` 是通配模式。

## Array

显式导入标准 Array 模块：

```telora
import "std/array" as array;
```

常用操作包括：

```telora
array.length(values)                 # Int
values[index]                        # A；以 OutOfRange blame 失败
array.get(values, index)             # Option(A)
array.enumerate(values)              # Array(Tuple([Int, A]))
array.find(values, predicate)        # Option(A)
array.any(values, predicate)         # Bool
array.all(values, predicate)         # Bool
array.map(values, mapper)            # Array(B)
array.flat_map(values, mapper)       # Array(B)
array.filter(values, predicate)      # Array(A)
array.fold(values, initial, folder)  # State
array.concat([left, right])          # Array(A)
array.push(values, item)             # 新的 Array(A)
```

Array 保留顺序。`find` 返回第一个匹配项，`filter` 保留输入顺序，fold 从左到右
处理各项。这些顺序属性可以成为确定性程序契约的一部分。

在结构对应的 `if`、`if let` 或 `match` 结果中，含有具体 Array 或 Dict 元素的
分支会向空分支提供元素类型证据，该行为与分支顺序无关。如果每个可达分支都为
空，且不存在预期元素类型，应添加显式标注，例如
`let none: Array(Item) = [];`。

两种索引形式都使用从零开始的 Int 索引。`values[index]` 直接返回元素；索引为
负数或越界时，以 `fail!("OutOfRange", values, index)` 失败。对于同样的缺失
位置，`array.get` 返回 `'None`。`enumerate` 在保留来源顺序和重复项的同时，
把每一项与其从零开始的 Int 索引配对：

```telora
array.get(["a", "b"], 1)       # 'Some("b")
["a", "b"][1]                 # "b"
array.enumerate(["a", "b"])   # [(0, "a"), (1, "b")]
```

缺失属于预期控制流时使用 `array.get`；缺失违反不变量时使用直接索引。算法需要
保留位置时使用 `array.enumerate(values)`。

图或集合等任何必要的有界结构，都使用有类型的不可变值和普通库函数构建。

## Tuple 元数据

Tuple 值和 Tuple TypeMetadata 是普通值的两种不同用途：

```telora
let pair: Tuple([Int, String]) = (1, "one");
let number: Int = pair.0;
type Pair = Tuple([Int, String]);
```

`Tuple` 恰好接收一个实参，即 TypeMetadata 的 Array：`Tuple([A, B])`。Tuple
值使用非负整数字面量投影，例如 `pair.0`。投影是可组合的后缀操作：
`value.1.0` 表示 `(value.1).0`，并且可以与字段选择、索引和调用组合。已知的
越界位置属于分析错误。`Fn(A) -> Array(Tuple([B, C]))` 等嵌套形式合法。

## TypeMetadata family

类型是一等元数据值。`Type` 是任意有效 TypeMetadata 的类型；`TypeOf(A)` 是
描述 `A` 的元数据的精确证据。

参数化声明定义可复用的 TypeMetadata family：

```telora
type Capability(Id, Input, Output) = struct {
    id: Id,
    lower: Fn(Id, Input) -> Option(Output),
};

type TicketCapability = Capability(TicketId, Request, TicketPlan);
```

family 在值位置也是普通的有类型元数据能力。family 必须接收全部参数，其阶数
为 rank-1，并且不能是 higher-kinded。无环的 family 可以引用同一模块中的具体
类型或另一个 family，且不受声明顺序影响：

```telora
type Build(Value) = struct {state: BuildState, value: Option(Value)};
type BuildState = enum {'Ready, 'Pending};
```

名义 Struct/Enum family 可以用当前全部参数原序直接自递归：

```telora
type Expr(Leaf) = enum {
    'Leaf(Leaf),
    'Call(Array(Expr(Leaf))),
};
```

它形成有限 symbolic graph；同一 concrete application 复用 canonical identity。参数
变换或换序、mutual family cycle、mixed cycle、无生产 alias，以及对普通局部 helper
的依赖仍然非法。family 也可以引用已经封闭的 concrete recursive type。不得用
`Any`、`Dyn` 或 String 标识替代本可由类型表达的关系。

## Value、格式、codec 与 schema

JSON、YAML 和 TOML 统一归一化为 `std/value.Value`。它是普通的 nominal recursive
enum，不是 `Any`、VM raw graph 或 lossless AST：

```telora
type Value = enum {
    'None, 'True, 'False,
    'Int(Int), 'Float(Float), 'String(String), 'Bytes(Bytes),
    'Array(Array(Value)), 'Object(Dict(Value)),
    'LocalDate(String), 'LocalTime(String),
    'LocalDateTime(String), 'OffsetDateTime(String),
};
```

`std/json` 负责 JSON 文本和 schema，`std/codec` 在 Value 与有类型值之间转换：

```telora
import "std/codec" as codec;
import "std/json" as json;
import "std/result" as result;
import "std/value" { Value };

type Query = struct {
    subject: String,
    limit: Int,
};

let raw = json.parse("{\"subject\":\"orders\",\"limit\":20}")
    |> result.unwrap;
let query: Query = codec.decode(Query, raw) |> result.unwrap;
let encoded: Value = codec.encode(Value, query) |> result.unwrap;
let compact: String = json.stringify(encoded);
let pretty: String = encoded |> json.stringify_pretty(2);
let query_schema = json.schema(Query);
```

也可以用 `json.decode(Query, text)` 直接把 JSON 文本解码成 `Query`。两条路径的
区别是边界位置：`json.parse` 只解析文本并返回 Value；`codec.decode` 对已经存在的
Value 施加类型契约。`codec.encode` 的首个参数固定为 canonical `Value` witness，
返回 Value；只有需要 JSON 文本边界时才调用 `json.stringify` 或
`json.stringify_pretty`。`yaml.parse` 和 `toml.parse` 同样返回
`Result(Value, BlameError)`。

Value 的每个递归 Array/Object 子节点都具有同一个 canonical TypeId，可以穷尽
match。`cast!` 只做表示不变的 checked refinement，不能解开 Value variant；
Value 与领域 model 的 rename/default/flatten 转换只能由 codec 完成。

上述 parse、decode 和 encode 都返回带 native opaque error 的 `Result`。普通源码
不命名该错误类型。调用者确实需要根据失败恢复或选择其他路径时，使用 `match` 保留
这个 `Result`；当前函数承诺返回解码后的值、失败后无法履行该契约时，使用
`unwrap!`，或匹配 Err 后调用 `fail!(error.message, error, input)`。Codec 失败不会
发布部分解码值。

Struct 和 enum 默认从同一份 TypeMetadata 派生 codec 与 JSON schema。常用的
`std/json` decorator 可以显式调整其 JSON 表示：

```telora
@json.rename_all('CamelCase)
type Details = struct {
    order_id: String,
    @json.default('None)
    note: Option(String),
};

type Envelope = struct {
    kind: String,
    @json.flatten details: Details,
};

@json.untagged
type Scalar = enum {
    'Text(String),
    'Count(Int),
};
```

此外可以用 `@json.rename("name")` 改写单个字段名，用
`@json.skip_serializing_if('None)`、`@json.skip_serializing_if('Empty)` 或一个返回
Bool 的函数省略满足条件的字段。Decorator 是产生 attribute 的普通元数据函数；
codec 和 schema 读取相同 attribute，因此二者不会形成两套独立模型。

JSON/TOML/YAML 文件也可以作为静态数据模块 import。它们在封闭模块图建立时由
Host 加载，不是运行时文件 IO，并且只导出 `data: Value`：

```telora
import "./request.json" { data as request };
import "./policy.yaml" { data as policy };
import "./config.toml" { data as config };
```

JSON/TOML 拒绝越界 Int 和非有限 Float。YAML 只接受 String mapping key，拒绝
custom tag，限制 alias 深度和展开量，并确定性展开 mapping merge；`!!binary`
经过 canonical base64 校验后成为 `'Bytes(...)`。格式归一化保留 array index/object
key 的来源路径，内部 Value wrapper 不增加路径层级。
不要为了打印中间值而手写 `*_desc` 函数：公开结果需要稳定 JSON 形状时使用
codec，需要临时观察任意局部值时使用 `dbg!`：

```telora
let plan = dbg!(make_plan(model, request));
let checked = plan.dbg!("before lowering");
```

`dbg!` 返回原值并保留其精确类型，因此可以直接插入表达式或管道。后置写法是通用
contextual intrinsic 糖：`value.dbg!("message")` 等价于
`dbg!(value, "message")`。message 必须是 String literal。

`telora run` 把观察写到 stderr，每行是一个 JSON object：

```json
{"name":"plan","repr":"{...}","module":"@src/query.telora","line":42,"message":"before lowering"}
```

`name` 是被观察表达式的源码文本，`repr` 是有界、确定且能处理 cycle 的临时表示。
它不是 JSON 编码契约。稳定结构化边界使用 codec，长期面向人的领域摘要应显式建模。
`dbg!` 不捕获其他局部变量，也不应观察敏感值或作为生产日志接口。

## 当前实现限制与缓解方法

本节描述当前实现中已经通过测试和实验确认的边界。遇到这些边界时，应使用给出的
有类型写法，不要用 `Any`、`Dyn` 或 String 标识绕过。

### 多元素能力目录的类型推断

显式 `Array(ConcreteFamily)` 契约会向每个元素下传完整的 expected item type。多个
匿名能力记录中的 singleton Atom、不同闭包、`'Some`/`'None` 窄 variant 和空集合
因此可以直接按同一个 concrete family 检查：

```telora
type EventId = enum {'Created, 'Updated};
type Event = struct {id: Int};
type Decision = enum {'Accept, 'Reject};
type HandlerDefinition(Id, Input, Output) = struct {
    id: Id,
    handle: Fn(Input) -> Output,
};
type Handler = HandlerDefinition(EventId, Event, Decision);

let handlers: Array(Handler) = [
    {
        id: 'Created,
        handle: fn(event) { if event.id > 0 { 'Accept } else { 'Reject } },
    },
    {
        id: 'Updated,
        handle: fn(event) { if event.id == 0 { 'Reject } else { 'Accept } },
    },
];
```

元素顺序不影响检查结果；真正不兼容的字段会在对应元素处报告类型冲突。同样的原则
适用于其他高阶 family 的记录字面量。只有缺少共同的 Array expected
type，或记录需要先在数组之外分别构造时，才给完整记录或具名构建函数添加 concrete
family 契约。巨大 union 错误应首先检查是否缺少这个公共期望类型。

### 声明 enum 的直接 expected context

已经确定的声明 enum 契约会直接下传到 Array 元素、record 字段、函数参数和返回值、
`if`/`match` 分支以及带函数类型标注的 closure。Expr、Operator、Val 等递归或非递归
enum 应在构造边界提供一次完整契约：

```telora
def make_expr: Fn() -> Expr = fn() { 'Column({alias: "orders", column: "id"}) };

def plan: Plan = do {
    let expr: Expr = if use_all { 'All } else { 'Column({alias: "orders", column: "id"}) };
    let operators: Array(Operator) = ['Filter(expr), 'Project([expr])];
    {expr, operators}
};
```

expected type 不穿过未标注 binding 反向解释它的定义。不要先写
`def raw = 'Column(...);`，再依靠后续 `def expr: Expr = raw;` 为 `raw` 补身份；应在
字面量、分支、closure 或集合的直接构造点标注 `Expr`。这也避免根据 tag 名全局猜测
一个 nominal enum owner。未知 variant 和不兼容 payload 仍然是确定的类型错误。

### enum payload 不能是匿名 Struct 类型

Enum variant payload 是 TypeMetadata 表达式。`struct { ... }` 只允许作为直接
`type` 初始化器，因此匿名 Struct 不能嵌入 payload；应先声明具名 Struct：

```telora
# 不支持：struct 初始化器不能嵌入 enum payload
# type Expr = enum {'Column(struct {alias: String, column: String})};

type ColumnRef = struct {alias: String, column: String};
type Expr = enum {'Column(ColumnRef)};

# 值位置的匿名记录仍然合法
let expr: Expr = 'Column({alias: "o", column: "id"});
```

### Family 与递归具体类型

递归 enum/struct 在函数契约、参数化 family 契约和模块接口中保持精确类型，不会把
递归位置擦除为 `Any`。Family 可以引用已经封闭的非参数化递归具体类型：

```telora
type Expr = enum {'Literal(Value), 'Call(CallExpr)};
type CallExpr = struct {name: String, args: Array(Expr)};

type Dialect(Context) = struct {
    render: Fn(Context, Expr) -> String,
};
```

递归类型可以经完整、选择性、alias 或 open import 进入其他模块的函数与 family
契约。同一模块也可以同时声明递归类型、引用它的 Plan/projection family、递归
renderer 和多个 transform 契约；`check`/`query` 与严格运行使用相同的递归 component
封闭规则，不需要为了 checker 人工拆分这些定义。Family 可以在直接 Struct/Enum
initializer 中用原参数自递归，但不能变换参数、形成 mutual/mixed cycle 或调用同模块
普通 helper。需要超出这一边界的共享递归骨架时，把递归部分封闭为 concrete type，
只在递归结构之外参数化使用它的 capability、renderer 或 dialect：

```telora
type Expr = enum {'Literal(Value), 'Call(CallExpr)};
type CallExpr = struct {name: String, args: Array(Expr)};

type Renderer(Context) = struct {
    render: Fn(Context, Expr) -> String,
};
```

同一递归代数只需要替换叶节点类型时，优先使用上述同参递归 family。若递归过程中
必须改变参数，分别声明封闭递归类型或先把允许叶节点建模为闭合 enum；不要用
`Any`/`Dyn` 模拟开放递归。

### 复杂 family 值的 codec witness

`codec.encode(Value, value)` 的首个参数固定为公共 Value witness；codec 从有类型值
已经携带的 canonical witness 读取 source schema。对于参数很多的 concrete family，
规范做法仍是在定义模块中建立一次 concrete type alias，并导出 alias 或有类型的
边界函数：

```telora
import "std/codec" as codec;
import "std/value" { Value };

type Snapshot = PipelineSnapshot(Stage, Input, Expr, Plan, Output);

def encode_snapshot = fn(value: Snapshot) {
    codec.encode(Value, value)
};

export { Snapshot, encode_snapshot };
```

下游调用 `encode_snapshot(value)`，不重建完整 TypeMetadata。该方式同样覆盖跨模块
调用和包含封闭递归类型参数的 family。Alias 和函数契约仍由静态检查，不从运行时值
反射类型；不要把值打包为 `Any`/`Dyn` 后猜测 witness。

### Bytes 没有默认 JSON 表示

公共 Value 可以显式携带 `'Bytes(Bytes)`，YAML `!!binary` 也映射到该 variant；但
JSON 没有原生 Bytes 类别，`json.stringify` 和 schema 不为 Bytes 选择隐式文本编码。
包含裸 `Bytes` 的类型不能作为完整 JSON text/schema 边界。设计需要稳定 JSON
codec/schema 的数据模型时，当前应从公共 `Val`、Model、Plan 和输出类型中排除 Bytes：

```telora
type Val = enum {
    'String(String),
    'Int(Int),
    'Float(Float),
    'Bool(Bool),
};
```

若应用要求本身必须携带二进制数据，把它记录为当前模型无法覆盖的边界，不自行
选择 Base64、tagged object 或其他协议。不要用 String 假装 Bytes，也不要通过手写
JSON、`Any` 或 `Dyn` 绕过该限制。Array 元素、enum payload 和根 Bytes 同样没有
隐式表示。

### 泛型函数和外围类型参数

多态函数不能作为“尚未实例化的普通值”依赖后续任意使用来决定全部类型参数。
优先在调用点推断，必要时用 `@[...]` 显式应用，或给具名辅助函数声明完整契约。
定义契约中 `for(...)` 引入的类型参数在对应实现体的类型位置内可见，包括局部
`let` 标注、嵌套类型应用和内层闭包注解；它们不会泄漏到相邻定义或模块结果。

```telora
def collect: for(N) Fn(Array(Item(N))) -> Array(Item(N)) = fn(items) {
    let result: Array(Item(N)) = items;
    result
};
```

## String 与诊断

只需要判断整个 String 是否满足词法规则时，使用 `std/regex` 做整串匹配，不要先把
String 拆成字符数组。应在模块级编译一次规则并复用，例如 SQL identifier：

```telora
import "std/regex" as regex;

def sql_identifier = regex.compile(r"^[A-Za-z_][A-Za-z0-9_]*$");
def is_sql_identifier: Fn(String) -> Bool = fn(text) {
    regex.is_match(sql_identifier, text)
};
```

不要用 `string.split(text, "")` 模拟字符遍历；它会物化中间数组，并包含首尾空串。

普通字符串不进行插值。插值使用反引号：

```telora
let message = `missing capability \{name}`;
let progress = `ratio=\{3.0}, offset=\{-0.0}`; # "ratio=3, offset=-0"
```

普通字符串支持 `\0`、`\n`、`\r`、`\t`、`\"`、`\\`、两位 ASCII `\xNN`、
Unicode scalar `\u{...}` 和反斜杠换行后的显式续行。反引号字符串使用 `` \` ``
代替 `\"`，并额外用 `\{...}` 表达插值。raw String 不处理 escape 或插值；正则、
SQL 模板等包含大量反斜杠的文本优先使用 raw String，并按需增加 `#` delimiter。

插值按值的运行时 primitive `meta` 支持 String、Int、Float 和 Atom，不读取名义
`ty`，也不会寻找 `fmt.display` provider。静态检查只尽早排除已知不可能落入这四种
表示的表达式。例如 `Bool` 值可以插值，是因为其表示为 Atom，而不是因为 Bool
实现了 Display。Float 使用有限 binary64 的稳定文本表示：最短、可往返、不受
locale 影响；`3.0` 显示为 `3`，`-0.0` 显示为 `-0`，原始小数或指数拼写不会保留。

插值不渲染任意 Tagged、Struct、Array、Dict、Tuple、Dyn 或用户值。声明 enum 和
Tagged payload 应先通过 `match` 得到明确文本；Array/Dict 的人类展示应显式
`array.map` 后使用 `string.join`。不要为了展示而把 Query binding 插入 SQL；仍应
保持 `{ sql, bindings }` 的参数绑定边界。

`std/fmt` 是独立的显式 TypeMetadata-driven 能力：

```telora
import "std/fmt" as fmt;

@fmt.display_by("{host}:{port}")
type Endpoint = struct { host: String, port: Int };

def endpoint_text: Fn(Endpoint) -> String = fn(endpoint) {
    fmt.display(Endpoint, endpoint)
};
```

`display_by` 是受控模板 eDSL：它构造带 `std/fmt.display` attribute 的 TypeMetadata，
而 `fmt.display` 显式接收这份 TypeMetadata。它不是 trait、`ToString` 或把模板转换成
Telora 表达式的代码生成函数，也不参与 `\{...}` 插值。

`dbg!` 的 `repr` 是 Host-only、有界且 cycle-safe 的观察文本，不进入 Telora String；
codec/JSON 是数据交换协议，也不是展示 API。Float 的 debug repr 会保留 `3.0` 和
`-0.0`，有意不同于插值及 `fmt.display` 的 `3` 和 `-0`。

```telora
def check_capability: Fn(Subject) -> Result(Capability, String) = fn(subject) {
    match find_capability(subject) {
        'Some(capability) => 'Ok(capability),
        'None => 'Err("missing capability"),
    }
};

let optional = check_capability.should_ok!(authored_subject);
let required = check_capability.must_ok!(authored_subject);
let optional_existing = existing_result.try_unwrap!();
let required_existing = existing_result.unwrap!();
fail!("missing capability", authored_subject)
```

Contextual intrinsic 支持 `receiver.ident!(arguments...)` 后置糖，严格等价于把 receiver
放到前置调用的第一个参数。它不是 method lookup，也不允许调用未由语言定义的
intrinsic。

对于 `checker: Fn(A1, ..., An) -> Result(R, String)`：

```text
checker.should_ok!(a1, ..., an) : Option(R)
checker.must_ok!(a1, ..., an)   : R
```

checker 可以接收零到多个参数，但不能省略 checker。checker 与各参数都只求值一次，
顺序从左到右；发生 Warning 或 failure 时，参数按同一顺序成为诊断证据。

- `should_ok!` 把 checker 的 `Ok(R)` 变成 `Some(R)`；Err 产生 Warning 和 `None`。
- `must_ok!` 返回 checker 的 Ok payload；Err 产生失败和 `Never`。
- `try_unwrap!` 和 `unwrap!` 对已有 `Result(R, String)` 应用相同两种策略。
- `?` 只传播原容器的失败分支，不产生诊断或转换容器。
- `fail!(message, subjects...)` 产生失败，并保留 subjects 的来源。
- `panic!(message)` 只用于实现错误或不变量破坏。

`for` 契约引入的类型参数在对应实现体的局部标注、嵌套类型应用和内层闭包注解中
可见。仅为产生诊断且输出类型难以从上下文推断时，模块级同类型辅助 checker 仍然
有助于保持精确类型：

```telora
def reject_same: for(A) Fn(A, String) -> Result(A, String) =
    fn(evidence, message) { 'Err(message) };

let ignored = reject_same.should_ok!(subject, "missing capability");
```

### 面向契约的失败模式

函数的公共契约承诺返回 `T` 时，普通写法是直接返回 `T`；当当前输入无法产生一个
合法的 `T` 时，使用 `fail!(message, subjects...)`：

```telora
def make_plan: Fn(Model, Request) -> Plan = fn(model, request) {
    let checked = validate(model, request);
    if checked.valid {
        assemble_plan(model, checked)
    } else {
        fail!("request cannot produce a complete plan", request, checked)
    }
};
```

普通 Telora 调用者不接收或处理诊断对象。它只表达值依赖、成功结果和失败位置；
求值器与 Host 依据这些依赖保留来源、跳过失败值的依赖计算，并尽力继续彼此独立的
工作。最终结果仍然原子发布：不能产生完整 `T` 时，不发布部分 `T`。

best-effort 求值在复合值内部也按数据依赖推进。`array.map` 会保留失败槽位、跳过它
继续后续逐项变换，并按索引顺序处理健康槽位；`array.length` 只依赖已知形状；选择
失败槽位会传播原诊断。`filter` 可以继续检查其他独立 predicate，但任一 predicate
失败都会令最终成员关系不可发布；`fold` 的 accumulator 失败后不再调用后续 reducer。
`flat_map`、`concat` 和 spread 的输出形状依赖失败成员，因此最终传播原 Fail，但不会
再产生“expected Array/Func”一类级联类型错误。普通函数的 callee 或直接实参为 Fail
时不执行函数体；结构相等、codec 和 JSON 读取完整数据图，遇到可达 Fail 也传播原根因。
Array/Tuple/Dict/tagged 构造、`map`、`enumerate`、`push` 和 `zip` 等保形操作可以保留
失败子节点，以便继续健康成员。`any` 的健康 True 和 `all` 的健康 False 可以确定性短路；
`find` 若在候选成员之前已有失败 predicate，则成员身份不确定并传播 Fail。
这些失败槽位不是语言值或额外 variant，源码不能匹配或恢复。可达性只决定还可继续
哪些诊断计算；只要出现任何 error，本轮命令的最终 Host 结果就整体失去意义，即使干净的
最终根仍可算出，codec、最终返回值和 SystemEffect 也不会越过 Host 边界。Module 在
WorkWorld/MainWorld 间的内部固化不是 Host 发布，可以保留 Fail。需要业务恢复时仍显式使用
`Option`、`Result` 或领域 enum。

依赖库失败时仍是普通 Module。Module 的 `Available/Unavailable` 只表达源码是否存在；
定义和表达式分别携带 `Known/Unknown/Incomputable` 等事实状态。内部 Module 可以同时保留
健康 export 和含 Fail 的 export：下游读取健康项可继续工作，读取失败项则传播同一个 Fail。
没有 `PartialModule`/`UntrustedModule` 语言实体，也不会把原始 error 降级。

不要仅仅为了向 Host 报告诊断，就把 `Fn(Input) -> Output` 改成公开的
`Fn(Input) -> Outcome(Output, Rejection)`，也不要在 eDSL 中复制一套
`BlameError`、诊断数组或发布状态机。只有 Telora 调用者本身确实需要恢复、分支或
组合失败时，才把失败建模为 `Option`、`Result` 或领域 enum。`panic!` 仍只表示实现
错误或不变量破坏，不用于输入不满足动态契约。

## 模块

```telora
import "std/array" as array;
import "@src/local.telora" { compile };
import "plan-lib/types.telora" as types;

def defaults: Defaults = do {
    let base = load_defaults();
    normalize(base)
};
export def compile: Fn(Input) -> Output = fn(input) { ... };
export { Entity, Requirement, compile };
```

import 是静态的。模块顶层是声明空间，只接受 option、import、type、decl、def、
native 和 export；普通模块值也使用 `def`。`let` 只用于函数或 `do` 等普通 block
中的顺序计算和局部 shadow。顶层 `let`、`export let`、裸表达式和 final expression
都不合法。模块只暴露显式 export。库必须导出向调用者承诺的每个类型和函数。

默认 prelude 相当于可遮蔽的隐式 open import，只为本模块尚未声明的名字提供
fallback。`validate` 等 prelude 名不是保留字，本地 binding 可以正常使用同名；
仍需访问内建项时使用显式别名，例如
`import "core/prelude" { validate as builtin_validate };`。

`src/bin/` 下的入口由 Host 以 `@bin/...` 选择；
`tests/` 下的入口以 `@test/...` 选择。从这些入口导入可复用代码时，应使用显式源码根路径：

```telora
# src/bin/main.telora
import "@src/compiler.telora" { compile };
```

稳定逻辑模块 ID 与 crate 布局一一对应：

```text
@src/model.telora       -> <crate>/src/model.telora
@bin/main.telora        -> <crate>/src/bin/main.telora
@test/compiler.telora   -> <crate>/tests/compiler.telora
plan-lib/types.telora   -> <plan-lib>/src/types.telora
```

Host 从当前目录向上查找最近的 `telora-deps.json`，因此可以在 crate 根目录或其任意
子目录运行命令。`run main` 选择 `@bin/main.telora`；binary name 是不含路径分隔符和
`.telora` 后缀的单个 stem。`@main` 不是模块 ID。`run main` 完全等价于
`run-with std/entry/default main`；默认 Entry 把 Main 的显式 String `output` export
转换为 `Output(String)` effect。自定义 Entry 使用 `.entry.telora` 后缀，并通过
`run-with` 显式选择；普通模块不能导入它。环境与输入由 Entry 显式传给 Main，不形成
ambient binding。完整示例：

```text
telora run main -C examples/my-crate
telora run-with @src/tool.entry.telora main -C examples/my-crate -- argument
telora check @test/compiler.telora -C examples/my-crate
```

`check` 用统一 Module 管线的 best-effort 策略求值所选模块；任何 error 都会非零退出，
但内部图仍可保留以查询健康事实。它不进行 Entry 调度，也不会调用已经
导出的函数，因此不等价于应用行为验收。成功路径必须由普通 `run` 严格执行；遇到
失败时可以用 `run --best-effort` 扩大诊断覆盖，并检查非零退出、Host 诊断和无
output。不能仅以 `check` 成功作为行为证据。

在 binary/test 入口中，`./compiler.telora` 以及其他 `./` 或 `../` import 非法。
在 `src/` 下的模块中，相对 import 仍然合法，并从导入模块的逻辑目录解析。
`@src/` 始终从导入模块所属 crate 的源码根解析。`plan-lib/types.telora`
等 package 路径选择由 crate manifest 固定的依赖；`std/...` 选择由 Host 注册的
内置模块。依赖只公开自身的 `src/`，不公开 `src/bin/` 或 `tests/`。

单文件探索使用 `telora run -S path/to/file.telora`。该模式不查找
`telora-deps.json`，即使文件祖先目录中存在 manifest；它只接受根文件中声明的
`crate.dependency` 和 `crate.format` resolver options，并相对根文件所在目录解析。
普通 crate 模块不能声明这些 resolver options，被 standalone 根导入的模块也不能
再次声明它们。

## 递归与有界工作

Telora 支持带显式契约的递归函数。调用和 back-edge 消耗 fuel；分配也受配额
限制。算法必须暴露其契约要求的任何语义深度界限，而不能依赖 Host fuel
最终耗尽来终止。

## 编程建议

- 通过 selector 和回调保留精确的泛型类型。
- 嵌套回调约束不足时，优先使用带显式契约的小型具名辅助函数。
- 应用事实和物理映射留在可复用方法库之外。
- 不得添加 Host 函数、native 声明、`Any` 或 `Dyn` 来绕过困难的泛型关系。
- 优先让类型表达静态约束；动态失败使用 `fail!` 并携带原始证据。
- 以严格 `run` 作为最终验收；失败排查时再使用 `--best-effort` 扩大诊断覆盖。
