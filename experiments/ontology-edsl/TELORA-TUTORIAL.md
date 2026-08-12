# 面向本体 eDSL 作者的 Telora 教程

这是本体 eDSL 实验使用的有界语言输入，不是完整的语言规范。某项行为未在
本文档中说明时，不要发明 Host 能力，也不要绕过有类型的 Telora 模型。

Telora 是一门确定、纯、面向表达式的语言，用于把意图编译为不可变计划。
值不可变，模块显式声明，TypeMetadata 是普通数据，并由执行程序代码的同一个
VM 求值。

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
let answer = 40 + 2;
def increment = fn(value) { value + 1 };
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

对于普通复合值，相等和不等运算保持结构相等语义。有序比较只接受类型相同的
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

泛型函数体内的局部标注不能引用外围模块级 `for` 契约引入的类型参数。应当把
完整泛型契约放在模块级辅助函数上，不得使用 `Any` 擦除类型来绕过这一边界。

Telora 在拓宽结果之前合并分支证据：泛型代码中的 `if` 若为同一个预期 enum
结果贡献不同的窄 variant，会先 join 它们，再拓宽为该 enum。例如，有类型的
`Array(Option(Output))` fold 可以在一个分支 push `'None`，在另一个分支 push
`'Some(output)`。当回调仍然约束不足时，带有完整契约的具名辅助函数依然有用。

## Struct、enum 与模式

```telora
@enum type Entity = {
    Ticket: 'None,
    Agent: 'None,
};

@struct type Requirement = {
    target: Entity,
    reason: String,
};
```

Struct 和 enum 都是封闭的。字段使用 `.field`；enum 值使用 Atom 或 Tagged
语法。

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

本实验主要使用以下操作：

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
处理各项。这些顺序属性可以成为确定性 eDSL 契约的一部分。

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
@struct
type Capability(Id, Input, Output) = {
    id: Id,
    lower: Fn(Id, Input) -> Option(Output),
};

type TicketCapability = Capability(TicketId, Request, TicketPlan);
```

family 在值位置也是普通的有类型元数据能力。family 必须接收全部参数，其阶数
为 rank-1，并且不能是 higher-kinded。无环的 family 可以引用同一模块中的具体
类型或另一个 family，且不受声明顺序影响：

```telora
@struct type Result(Value) = {outcome: Outcome, value: Option(Value)};
@enum type Outcome = {Published: 'None, Rejected: 'None};
```

包含 family 的环、递归的具体依赖，以及对普通局部辅助项的依赖仍然非法。
标识、payload、映射和计划使用模型提供的具体类型。不得用 `Any`、`Dyn` 或
String 标识替代未知关系。

## String 与诊断

普通字符串不进行插值。插值使用反引号：

```telora
let message = `missing capability \{name}`;
```

插值支持 String、Int 和 Atom 等具有稳定表示的标量。它不渲染任意 Tagged、
Struct、Array、Dict、Tuple、Dyn 或用户值。主体是结构化值时，消息应保持静态。

```telora
let error = blame!("missing capability", authored_subject);
let reported = report('Error, error);
let warning = emit_warn!("fallback policy used", authored_subject);
let ignored = emit_error!("missing capability", authored_subject);
raise!(error)
```

- `blame!` 构造带来源的 `BlameError` 值，但不报告它。
- `report` 发布诊断，并返回同一个错误。
- `emit_info!` 和 `emit_warn!` 报告非阻塞诊断。
- `emit_error!` 是报告便利形式；Error 诊断会使严格模块执行在其发布边界失败，
  即使局部求值会继续到足以收集彼此独立诊断的位置。
- `raise!` 立即以 `Never` 退出最近的函数。

预期的领域拒绝必须能够与意外的运行时失败区分。如果 eDSL 契约要求值级拒绝
结果，应返回 `Option`、`Result`、显式 enum 或诊断值；不得用 `emit_error!`
替代该结果通道。

## 模块

```telora
import "std/array" as array;
import "@src/local.telora" { compile };
import "ontology-lib/types.telora" as types;

export def compile: Fn(Input) -> Output = fn(input) { ... };
export { Entity, Requirement, compile };
```

import 是静态的。模块只暴露显式 export。eDSL 必须导出向企业作者承诺的每个
类型和函数。

`a2/bin-src/` 下的入口是由 Host 选择的根入口，不是位于 `a2/src/` 旁边的源码
模块。从该入口导入可复用代码时，应使用显式源码根路径：

```telora
# a2/bin-src/main.telora
import "@src/ontology.telora" { compile };
```

在 `bin-src/` 入口中，`./ontology.telora` 以及其他 `./` 或 `../` import 非法。
在 `a2/src/` 下的模块中，相对 import 仍然合法，并从导入模块的逻辑目录解析。
`@src/` 始终从导入模块所属 crate 的源码根解析。`ontology-lib/types.telora`
等 package 路径选择由 crate manifest 固定的依赖；`std/...` 选择由 Host 注册的
内置模块。

## 递归与有界工作

Telora 支持带显式契约的递归函数。调用和 back-edge 消耗 fuel；分配也受配额
限制。领域算法必须暴露其契约要求的任何语义深度界限，而不能依赖 Host fuel
最终耗尽来终止。

## 工作规则

- 通过 selector 和回调保留精确的泛型类型。
- 嵌套回调约束不足时，优先使用带显式契约的小型具名辅助函数。
- 企业事实和物理映射留在可复用 eDSL 之外。
- 不得参考仓库中已有的本体实现。
- 不得添加 Host 函数、native 声明、`Any` 或 `Dyn` 来绕过困难的泛型关系。

接下来阅读 `a1/EDSL-DESIGN.md`。它规定 eDSL 的可观察行为，同时把模块布局、
API、辅助项拆分方式和算法留给你决定。
