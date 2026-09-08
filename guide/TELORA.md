# Telora 语言教程

本文面向第一次使用 Telora 的程序作者和库作者，说明当前已经实现并经过验证的
公开语言表面。完整语义以 [`../docs/design/LANGUAGE.md`](../docs/design/LANGUAGE.md)
为准；本文没有说明的行为不能据此推断为存在。

本文把嵌入 Telora、准备外部输入、执行 Entry 效果并呈现诊断的 CLI 或运行时适配器
称为运行时宿主（Host）。

Telora 是一门确定、纯、面向表达式的语言，用于把意图编译为不可变计划。
值不可变，模块显式声明，TypeMetadata 是普通数据，并由执行程序代码的同一个
VM 求值。

最小 workspace 由 workspace 配置、crate manifest、lock 和入口模块组成：

```text
hello/
  telora-config.json
  telora-crate.json
  telora-lock.json
  src/app.telora
```

```json
{"version":1,"members":["."]}
```

```json
{"name":"hello","modules":["@src/app"],"dependencies":[]}
```

```telora
# src/app.telora；# 引入行注释
import "std/actor" as actor;
import "std/ees" as ees;
import "std/entry" as entry;
import "std/value" {Value};

type State = struct {};
def config: entry.ContextConfig = {sources: [], envs: [], args: False};
export def run = entry.run(State, config, ees.none, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            actor.Event.Request(request) => (state, [actor.reply(request.id, Value.String("hello, telora"))]),
            actor.Event.EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({}, reduce)
});
```

在 crate 目录运行：

```bash
telora lock
telora check @src/app
telora run @src/app:run
telora query exports @src/app
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
BuildState.Ready           # BuildState 的无 payload variant
True                       # Bool 的 True variant
Some(1)                    # Option(Int) 的 Some variant
(1, "one")                # Tuple 值
[1, 2, 3]                  # Array
{name: "Ada", active: True} # record/Dict 值
```

Bool 值是 `True` 和 `False`；Telora 不进行 truthiness 转换。Float 是有限的
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

### Unit 与 Block 返回值

`()` 是空元组值，也可在明确的类型位置表示空元组类型；`Unit` 是该类型的别名：

```telora
type Empty = ();
def nothing: Fn() -> () = fn() {};
def identity: Fn(()) -> Unit = fn(value: ()) { value };
```

Block 的无分号尾表达式决定返回值。表达式后加 `;` 会执行并丢弃其结果；没有尾
表达式时正常返回 `()`，适用于 `do`、函数体及分支 block：

```telora
let empty: Unit = do {};
let discarded: () = do { 42; };
let bindings: Unit = do { let a = 42; };
let answer: Int = do { 1; 42 };
```

分号不会吞掉失败，也不会把 `return` 或 `Never` 路径改成正常返回。裸 `{}` 仍是
字典；顶层模块不允许表达式语句。`Fn()` 没有参数，`Fn(())` 有一个 Unit 参数。

非空元组类型仍使用 `Tuple([A, B])`。普通元数据调用写作 `Array(Unit)`；其中的
数据实参 `()` 不会自动解释为类型。`.type` 和非空元组类型新语法尚未实现。

### 比较与算术

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
相同的名义类型。dict 字面量可以从另一侧获得具名类型上下文；具名 variant 构造
可以从另一侧补全泛型参数和 payload 的类型上下文。构造器名称确定 enum 类型族，
例如 `Wrapper.Box("x")` 始终构造 Wrapper 的 Box 成员。同一 enum 契约内的不同
运行时 variant 可以比较并返回 False。
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
`|` 支持 Int 按位运算，`<~` 用于具名 struct 的合并更新。从紧到松的优先级
依次为：一元运算、`*`/`/`/`%`、`+`/`-`、
`&`、`^`、`|`、`<~`、比较、`&&`、`||`。

### Struct 合并更新

`base <~ patch` 创建一个与 `base` 同类型的新值。左侧的具名 struct 类型决定
完整字段集合及泛型实参；右侧提供需要替换的字段，未提供的字段保留原值。

```telora
type State = struct {count: Int, label: String};
type Label = struct {label: String};
def update = fn(base: State, label: Label) {
    base <~ label <~ {count: 2, ...label}
};
```

右侧可以是具名 struct 值，也可以是更新字面量。更新字面量从左侧获得字段
类型上下文，其中 `...value` 展开一个具名 struct 的静态字段集合。所有更新
字段名必须属于左侧类型，最终生效的字段值必须符合对应字段类型。嵌套字段
按完整值替换；嵌套字面量可以按该字段的具名类型构造。更新字面量自身无需
独立的具名类型；结果身份始终来自左侧。`base <~ {}` 创建保留全部字段的新值。

Struct spread 用于更新字面量，普通 Dict spread 使用 `Dict(T)` 操作数。更新
操作数须具有已知的具名 struct 字段集合；Dict 的动态键集合不提供该证据。

字面量内从左到右覆盖，同名字段取最后一个值；显式书写的字段名须唯一。
`<~` 左结合，链中的每一步独立通过类型检查。操作数和更新字段按源码顺序各
求值一次，被覆盖的表达式也会求值。复制的字段保留原始位置，新容器的位置
来自更新表达式。

### 字段投影

`source.{x, y as Y}` 从具名 struct 选择字段，并可为目标字段指定新名称。

```telora
type Source = struct {x: Int, y: String, extra: Int};
type Foo = struct {x: Int, Y: String};
def select: Fn(Source) -> Foo = fn(source) { source.{x, y as Y} };
def update = fn(base: Foo, source: Source) {
    base <~ source.{x, y as Y}
};
```

构造时，由类型注解、函数参数、返回值或相等比较另一侧的类型确定目标具名类型；例如
`let selected: Foo = source.{x, y as Y};`。投影后的字段须完整匹配目标类型。
用于 `<~` 右侧时，投影提供更新字段子集，结果保持左侧类型。

源字段须存在，目标字段名须唯一，对应值须类型兼容。可把同一个源字段映射
到多个不同目标名。空投影 `.{}` 可用于构造具名空 struct 或提供空更新。
接收者只求值一次，所选字段保留原值的位置与类型身份。普通投影构造要求
目标类型上下文，字段形状本身不决定具名类型。

### 比较与控制流

六种比较共享一个不可结合的优先级层。需要比较某个比较结果时必须写括号；
`a < b <= c` 不是链式比较。`&&` 和 `||` 接受 Bool，并执行短路求值。
`if` 表达式始终具有 `else` 分支。`ctrl_block` 可以是普通 block、`if`、
`if let`、`match` 或 `return expression;`。`if` 和 `if let` 的 `else` 接受
`ctrl_block`；非 block 形式会被规范化为包含该控制流表达式的 block。因此可以
连续写 `else if`：

```telora
type Grade = enum {Excellent, Pass, Fail};
if score >= 90 { Grade.Excellent }
else if score >= 60 { Grade.Pass }
else { Grade.Fail }
```

```telora
if ready { value }
else if let Some(cached) = candidate { cached }
else match fallback { Some(value) => value, None => default }
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

回调参数和 Result 分支需要足够的类型上下文。例如，`Err("bad")` 只提供错误
类型，可以用完整契约确定成功类型及回调参数：

```telora
let mapped: Result(Int, String) = result.map(Err("bad"), fn(value) { value });
let explicit = result.map@[Int, String, Int](Err("bad"), fn(value) { value });
```

prelude 提供 `True/False`、`Some/None` 和 `Ok/Err`，分别属于 Bool、Option 和
Result。成员名称确定枚举家族；泛型参数由载荷、完整调用和结果上下文共同确定。
`Some(1)` 的类型是 `Option(Int)`；单独使用 `None` 时，需要确定其元素类型。
`Ok(1)` 还需要错误类型，`Err("bad")` 还需要成功类型。

匿名 Struct 实参同样参与完整调用上下文。泛型回调结果可以确定较早 seed 中的
空集合字段的元素类型。下列 fold 的 `flag` 由 `False` 确定为 Bool，
`items` 结合回调推断为 `Array(Int)`：

```telora
array.fold([1, 2, 3], {flag: False, items: []}, fn(state, item) {
    {flag: item > 1 || state.flag, items: array.push(state.items, item)}
})
```

同一家族的不同分支可以互相补全泛型参数，分支顺序不影响结果：

```telora
let selected = if True { Some(1) } else { None }; # Option(Int)
let result = match Some("hi") {
    Some(x) => Ok(x),
    None => Err(2),
}; # Result(String, Int)
```

这种证据合并适用于 `if`、`if let`、`match`、显式返回值和集合元素，也适用于
同一具名泛型 enum 的不同成员。嵌套参数逐层补全；不同枚举声明保持独立身份，
已有的具体类型必须相容。始终没有证据的参数需要返回类型注解、`.ty!(Ty)` 或
显式 `@[Ty]`。

## Struct、enum 与模式

`Unchecked(T)` 为具名字段 struct T 提供独立的候选值类型，保留 T 的字段类型
和泛型参数。候选值可读取字段；需要 T 的上下文将候选值完成构造为 T。
重复应用保持同一类型：`Unchecked(Unchecked(T))` 等于 `Unchecked(T)`。
Dyn 保留候选值身份，不能把它直接投影成 T 或另一具名 struct 的候选值。

```telora
type Point = struct {x: Int, y: Int};
let candidate: Unchecked(Point) = {x: 1, y: 2};
let point: Point = candidate;
```

声明类型的构造可通过 `@check(func)` 校验候选值。校验函数返回 `Option(BlameError)`：
None 接受原值，Some(error) 在构造处产生诊断。具名字段 struct 的参数为
`Unchecked(T)`，newtype 和带载荷 variant 的参数为载荷类型。无载荷 variant
直接成立，不接受 `@check`。

```telora
@check(fn(value) {
    if value.min <= value.max { None }
    else { Some(blame!("invalid range", value.min, value.max)) }
})
type Range = struct {min: Int, max: Int};
let range: Range = {min: 1, max: 3};
```

校验保留字段的来源位置。读取、复制和传递已完成构造的值不重复校验；
merge-update 的每个结果分别校验，投影构造的目标值也执行其校验。
类型计算期间的构造同样执行校验。工具阶段根据依赖准备校验函数及其捕获值，
在校验就绪后执行相应构造。
泛型函数体内的构造也执行校验，包括推断出的局部泛型函数和递归构造。
校验发生在值的构造处，与函数最终返回该值还是返回其他类型无关。

`type UserId = struct(Int);` 声明单元素具名 tuple（newtype）。`value.0` 读取
内部的 Int；外层 UserId 与 Int 是不同类型。newtype 可以参数化，例如
`type Box(T) = struct(T);`。载荷为具名类型时，`.0` 保留其具名身份。
JSON 编解码使用载荷的表示，成功解码后得到目标 newtype。

值位置的类型声明名称提供构造器函数：`UserId(1)` 构造 UserId，
`let make = UserId;` 可将构造器作为函数传递。`Box(1)` 推断载荷类型，
`Box@[Int](1)` 显式指定类型参数。类型注解、Type 参数以及显式的 Type 值契约
使用同一声明的类型用途，例如 `let ty: Type = UserId;`。import 和 reexport
保留声明的这两个用途；普通 Type 变量与返回 Type 的函数保持其值契约。

构造器模式按声明解构 newtype：`let UserId(value) = id;` 读取载荷，
`match id { UserId(0) => "zero", UserId(value) => "other" }` 按载荷匹配。
模式可以嵌套，支持泛型和模块限定名称，例如 `Box(UserId(value))` 与
`model.UserId(value)`。模式中的构造器名称引用类型声明；载荷保留自己的类型。

类型计算和 decorator 参数也可以使用构造器。例如
`type Wrapped = struct(Type); type Selected = Wrapped(Int).0;` 中 Selected
表示 Int。类型上下文决定声明的类型用途，值上下文提供构造器函数。

enum 成员通过类型名称引用：`type Event = enum { Progress(Int), Finished };`
声明后，`Event.Progress(1)` 构造带载荷的值，`Event.Finished` 表示无载荷的值。
`Event.Progress` 本身具有 `Fn(Int) -> Event` 契约，可以作为函数传递。
泛型成员支持上下文推断与显式参数，例如 `Option.Some(1)` 和
`Result.Ok@[Int, String](1)`；无载荷成员同样需要完整的类型证据，
例如 `let empty: Option(Int) = Option.None;`。限定名称确定所属 enum，
import/reexport 的类型别名保持这个身份。

模式同样可以使用限定成员名称：

```telora
match event {
    Event.Progress(value) => value,
    Event.Finished => 0,
}
```

限定模式检查所属声明与被匹配值的类型一致；带载荷成员要求载荷模式，
无载荷成员直接使用成员名称。它们可以用于嵌套模式、`if let` 和 `let else`。
模式构造器由声明身份确定；保存构造函数的普通函数绑定仅用于调用。

选择性成员导入为成员建立本地名称；成员导出同时建立本地名称和公开名称：

```telora
import Event.{Progress, Finished as Done};
export Result.{Ok as Success, Err as Failure};

let progress = Progress(1);
let result = Success@[Int, String](2);
```

成员名称保留所属声明和完整泛型参数。带载荷的成员名称可以用于调用、作为函数
传递和写在模式中，例如 `Progress(value)`。成员导入产生的名称须与同一作用域
中的其他绑定不同；重命名可以区分不同 enum 的同名成员。其他模块可以通过
普通模块导入取得公开的成员名称，后续 reexport 保留其声明身份。
类型可以保持模块私有，同时通过成员导出提供构造与模式匹配能力，例如
`export Status.{Ready};`。调用方通过公开的成员名称引用该声明。

在 `match`、`if let` 和 `let else` 的模式中，导入的无载荷成员直接写名称：

```telora
import Option.{Some, None};
match result {
    Some(value) => value,
    None => fallback,
}
```

名称解析依据成员的声明来源，不区分首字母大小写。无载荷成员模式参与穷尽性
检查；带载荷成员须写载荷模式。普通名称可以引入模式变量，普通值别名不取得
成员模式身份。`let name = value;` 建立普通变量绑定，可以遮蔽外层名称。

```telora
type Entity = enum {
    Ticket,
    Agent,
};

type Requirement = struct {
    target: Entity,
    reason: String,
};
```

Struct 和 enum 都是封闭的具名声明。不同声明即使结构相同也不是同一个类型；alias、
import 和 reexport 保留原声明身份。字段使用 `.field`；enum 值使用声明提供的
成员名称。`struct` 和 `enum` 用于 `type` 的直接初始化。

成员名称确定所属 enum，泛型参数可以从注解、函数参数、返回契约或完整调用中的
其他实参推断，也可以显式指定：

```telora
let entity = Entity.Ticket;
let explicit = Entity.Agent.ty!(Entity);
let enabled = True;
let some: Fn(Int) -> Option(Int) = Some;
let value = some(7);
```

尚未确定的泛型参数需要显式证据，例如 `None.ty!(Option(Int))`。
模式中的成员按声明身份和被匹配值的类型检查。

声明上下文中的记录字面量取得预期类型的声明身份。成员构造器的载荷也接收完整
调用提供的上下文，例如 `Some([{value: 7}])` 可以从另一个 `Option(Array(Item))`
实参取得内部记录的 `Item` 类型。外部 JSON/TOML/YAML
数据可以在 `codec.decode` 这类有精确 witness 的解码边界取得
身份。已经产生的匿名记录或另一个声明类型的值，不能只因结构相同而在后续标注、
参数或返回值边界被重新标记；应在字面量的产生点给出声明契约。

静态约束、checked cast 和动态投影是三种不同能力：

```telora
let empty = [].ty!(Array(Int));       # 只协助静态推断，无运行时调用
let truth = True.ty!(Bool);

let user_result = raw.cast!(User);   # Result(User, String)

import "std/dyn" as dyn;
let projected = dyn.project@[User](package); # Option(User)
```

`ty!` 必须能在编译期证明目标类型。Dyn 中的值通过显式投影取得具体类型。
`cast!` 只验证表示并保留原数据图：raw
匿名 Dict 可以在完整匹配时取得目标 struct witness，但两个不同具名类型不能按结构互转；
String parse、Int/Float 转换、`Value -> model`、rename/default/flatten 都属于 codec，
不属于 cast。Dyn 投影只在打包时的 canonical 类型与目标完全相同时成功，不做结构猜测。

```telora
match result {
    Some(value) => value,
    None => fallback,
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
位置，`array.get` 返回 `None`。`enumerate` 在保留来源顺序和重复项的同时，
把每一项与其从零开始的 Int 索引配对：

```telora
array.get(["a", "b"], 1)       # Some("b")
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

Tuple 字面量支持 `...` 展开：

```telora
let pair = (1, "hi");
let values = (...pair, True, 3);
# values: Tuple([Int, String, Bool, Int])
```

每个 spread 操作数须具有静态已知的 Tuple 类型；展开保留各位置的独立类型。
可组合多个 spread，空 Tuple 贡献零个元素。`(...pair)` 和 `(...pair,)` 都构造
展开后的 Tuple，`(pair)` 是普通分组，`(pair,)` 是包含 pair 的单元素 Tuple。
展开只进行一层，普通 tuple-valued 元素保持嵌套。

目标 Tuple 类型按展开后的位置提供上下文，包括 spread 中直接书写的字面量；
展开后的长度和每个元素类型须符合目标契约。操作数按源码顺序各求值一次，
空 spread 也会求值，复制元素保留原始位置和具名身份。Array spread 与 Tuple
spread 分别接受 Array 和 Tuple，不进行动态长度转换。

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
type BuildState = enum {Ready, Pending};
```

名义 Struct/Enum family 可以用当前全部参数原序直接自递归：

```telora
type Expr(Leaf) = enum {
    Leaf(Leaf),
    Call(Array(Expr(Leaf))),
};
```

它形成有限 symbolic graph；同一 concrete application 复用 canonical identity。参数
变换或换序、mutual family cycle、mixed cycle、无生产 alias，以及对普通局部 helper
的依赖仍然非法。family 也可以引用已经封闭的 concrete recursive type，并由类型参数保留静态关系。

## Value、格式、codec 与 schema

JSON、YAML 和 TOML 统一归一化为 `std/value.Value`。它是普通的 nominal recursive
enum，表示归一化后的语义数据：

```telora
type Value = enum {
    None, True, False,
    Int(Int), Float(Float), String(String), Bytes(Bytes),
    Array(Array(Value)), Object(Dict(Value)),
    LocalDate(String), LocalTime(String),
    LocalDateTime(String), OffsetDateTime(String),
};
```

`std/value.ScalarValue` 是带 untagged codec 的标量子集，包含 null、Bool、Int、Float 和
String。参数化查询用它表达 bindings，codec 会直接产生对应的 JSON scalar。

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
let encoded: Value = codec.encode(Value, query);
let compact: String = json.stringify(encoded);
let pretty: String = encoded |> json.stringify_pretty(2);
let query_schema = json.schema(Query);
let schema_text = json.stringify(query_schema);
```

也可以用 `json.decode(Query, text)` 直接把 JSON 文本解码成 `Query`。两条路径的
区别是边界位置：`json.parse` 只解析文本并返回 Value；`codec.decode` 对已经存在的
Value 施加类型契约。`codec.encode` 的首个参数固定为 canonical `Value` witness，
返回 Value；只有需要 JSON 文本边界时才调用 `json.stringify` 或
`json.stringify_pretty`。`yaml.parse` 和 `toml.parse` 同样返回
`Result(Value, codec.BlameError)`。`codec.decode` 和 `json.decode` 返回
`Result(A, codec.BlameError)`；错误为不可观察的 native 对象，保留消息和失败值的来源。
解码试探失败可以作为普通 Result 继续处理。需要产生诊断时使用
`raise!(error)`，数据位置来自保留的失败 Value；缺失字段使用父对象。
解码构造带有 `@check` 的类型时，先校验子值，再校验包含它们的候选值。
校验返回 `Some(error)` 时，解码返回 `Err(error)`。untagged 解码将这种拒绝视为
分支不匹配，要求恰好一个分支成功；校验函数主动 `fail!` 则中止执行。
编码已经校验的值不会重复执行构造校验。
`string.parse(T, text)` 将文本解析为 T，语法解析失败返回 `Err(ParseError)`，
成功解析的候选值及其嵌套字段经过构造校验，校验拒绝产生失败诊断。
使用 `@string.decode_by_parse` 的 codec 文本桥接也执行这些校验，拒绝时返回
`Err(BlameError)`，可以参与 untagged 分支试探。解析字段的来源是输入字符串。
静态数据模块保留每个子节点的位置。字符串解析产生的节点保留输入字符串的来源，
解析消息中的行列描述字符串内容；这些行列不作为 Telora 源码内的偏移。

Value 的每个递归 Array/Object 子节点都具有同一个 canonical TypeId，可以穷尽
match。`cast!` 只做表示不变的 checked refinement，不能解开 Value variant；
Value 与领域 model 的 rename/default/flatten 转换只能由 codec 完成。
`cast!` 形状不匹配时返回 `Err(String)`；形状匹配后，新增的声明类型身份必须通过
对应的构造校验，包括嵌套字段。校验拒绝产生失败诊断。转换已经校验的同类型值
不会重复执行校验。

parse 和 decode 的错误可以通过 `match` 恢复或选择其他路径。encode 直接返回
`Value`；无法编码的输入或有冲突的编码配置产生诊断。Codec 失败不会发布部分结果。

Struct 和 enum 默认从同一份 TypeMetadata 派生 codec 与 JSON schema。`std/json`
目前保留两个类型级 typed-property decorator：

```telora
@json.rename_all(json.RenameCase.CamelCase)
type Details = struct {
    order_id: String,
    note: Option(String),
};

@json.untagged
type Scalar = enum {
    Text(String),
    Count(Int),
};
```

`rename_all` 接受 `RenameCase` enum，支持 `json.RenameCase.CamelCase`。`json.schema` 返回 `Value`，
可以直接交给 `json.stringify`。`rename_all` 和 `untagged` 产生具名 property，codec 和 schema 按目标 TypeId 与
property TypeId 查询同一份 MainWorld 数据。字段和 variant property 按 owner TypeId、
canonical member index 和 property TypeId 安全存取。当前 JSON API 在类型层提供
`rename_all` 和 `untagged`；member 表示定制在领域模型或显式 codec 层表达。

### 自定义 typed property 与静态能力

Property carrier 必须是无类型参数的具名 Struct/Enum，并用 `@property` 声明允许的
owner。参数是内建具名枚举 `PropertyTarget` 的值，支持成员引用、别名和工具阶段
可求值的表达式。其成员 `Type`、`StructType`、`EnumType`、`Member`、`Field` 和
`Variant` 可以通过多个 `@property` 标记组合；
provider 接收 owner context 与同 key 的前一个值。多个同类型 decorator 按源码顺序
fold，因此一个 property 可以由多个局部标注逐步构成：

```telora
import "std/array" as array;
import "std/string" as string;
import "std/type-desc" { TypeDesc };
import "std/type-property" as property;
import "std/type-property" { FieldPropertyCtx };

@property(PropertyTarget.Field)
type Labels = struct { values: Array(String) };

@property(PropertyTarget.Type)
type Summary = struct { first_field_labels: Array(String) };

def label: Fn(String) -> Fn(FieldPropertyCtx, Option(Labels)) -> Labels = fn(value) {
    fn(ctx, previous) {
        let values = match previous {
            Some(labels) => array.push(labels.values, value),
            None => [value],
        };
        let result: Labels = { values };
        result
    }
};

def summarize: Fn(TypeDesc, Option(Summary)) -> Summary = fn(target, previous) {
    let values = match property.get_field_prop(target, 0, Labels) {
        Some(labels) => labels.values,
        None => [],
    };
    let result: Summary = { first_field_labels: values };
    result
};

@summarize
type User = struct {
    @label("identity")
    @label("public")
    id: Int,
    name: String,
};
```

Field/Variant provider 分别接收 `FieldPropertyCtx` / `VariantPropertyCtx`，其中包含
owner Type、canonical member index、name 和 member type/payload。所有 member
property 完成后才执行 type provider，所以 `summarize` 可以读取封闭的 member
snapshot。发布是原子的；任一 provider 失败都不会留下部分 property。

显式反射使用 `get_type_prop`、`get_field_prop` 和 `get_variant_prop`，返回
`Option(P)`。当 API 要求 imported property 必须存在时，使用静态 `Property(P)`
bound；编译器传递同一份已发布 payload，不在 VM 中搜索 registry：

```telora
import "std/fmt" as fmt;

trait Describe {
    describe: Fn(Self) -> String,
};

impl(T: Property(fmt.DisplayBy)) Describe for T {
    describe: fn(value) {
        fmt.render(fmt.display(T, value))
    },
};

def describe: for(T: Describe) Fn(T) -> String = fn(value) {
    Describe.describe(value)
};
```

`impl(T: Bound) Trait for Target` 中的参数属于 impl 声明；函数值的多态类型仍写作
`for(T) Fn(T) -> ...`。Trait 是静态 dictionary capability，不产生 trait object，也
不做运行期 method lookup。精确 impl 优先于满足约束的 property blanket impl；重复或
无优先级的重叠 impl 会被 coherence 检查拒绝。

Property payload 是普通有类型值，也可以包含普通 closure。closure 随 property root
从 WorkWorld 原子发布到 MainWorld，保留函数 identity、bytecode/native prototype 和
完整捕获图。适合把 TypeDesc/member property 的一次性解释结果准备为运行期 closure；
不要在每次业务调用中重新枚举 metadata 或 property registry。

JSON/TOML/YAML 文件也可以作为静态数据模块 import。它们在封闭模块图建立时加载，
不是程序执行期间的文件 IO，并且只导出 `data: Value`：

```telora
import "./request.json" { data as request };
import "./policy.yaml" { data as policy };
import "./config.toml" { data as config };
```

JSON/TOML 拒绝越界 Int 和非有限 Float。YAML 只接受 String mapping key，拒绝
custom tag，限制 alias 深度和展开量，并确定性展开 mapping merge；`!!binary`
经过 canonical base64 校验后成为 `Value.Bytes(...)`。格式归一化保留 array index/object
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
{"name":"plan","repr":"{...}","module":"@src/query","line":42,"message":"before lowering"}
```

`name` 是被观察表达式的源码文本，`repr` 是有界、确定且能处理 cycle 的临时表示。
它不是 JSON 编码契约。稳定结构化边界使用 codec，长期面向人的领域摘要应显式建模。
`dbg!` 不捕获其他局部变量，也不应观察敏感值或作为生产日志接口。

## 当前实现限制与缓解方法

本节描述当前实现中已经由源码和测试确认的边界。遇到这些边界时，应使用给出的
有类型写法，并通过类型参数保留输入输出之间的关系。

### 多元素能力目录的类型推断

显式 `Array(ConcreteFamily)` 契约会向每个元素下传完整的 expected item type。多个
匿名能力记录中的 variant 构造、不同闭包和空集合
因此可以直接按同一个 concrete family 检查：

```telora
type EventId = enum {Created, Updated};
type Event = struct {id: Int};
type Decision = enum {Accept, Reject};
type HandlerDefinition(Id, Input, Output) = struct {
    id: Id,
    handle: Fn(Input) -> Output,
};
type Handler = HandlerDefinition(EventId, Event, Decision);

let handlers: Array(Handler) = [
    {
        id: EventId.Created,
        handle: fn(event) { if event.id > 0 { Decision.Accept } else { Decision.Reject } },
    },
    {
        id: EventId.Updated,
        handle: fn(event) { if event.id == 0 { Decision.Reject } else { Decision.Accept } },
    },
];
```

元素顺序不影响检查结果；真正不兼容的字段会在对应元素处报告类型冲突。同样的原则
适用于其他高阶 family 的记录字面量。只有缺少共同的 Array expected
type，或记录需要先在数组之外分别构造时，才给完整记录或具名构建函数添加 concrete
family 契约。共同类型错误应首先检查是否缺少这个公共期望类型。

### 具名 enum 构造与类型推断

构造器名称确定 enum 类型族；类型上下文补全泛型参数和 payload 中的记录、闭包、
空集合等信息。完整契约会下传到 Array 元素、record 字段、函数参数和返回值、
`if`/`match` 分支以及带函数类型标注的 closure：

```telora
def make_expr: Fn() -> Expr = fn() { Expr.Column({alias: "orders", column: "id"}) };

def plan: Plan = do {
    let expr = if use_all { Expr.All } else { Expr.Column({alias: "orders", column: "id"}) };
    let operators = [Operator.Filter(expr), Operator.Project([expr])];
    {expr, operators}
};
```

prelude 提供 `Bool.{True, False}`、`Option.{Some, None}` 和 `Result.{Ok, Err}`。
同一类型族的分支和集合元素合并泛型参数证据，因此
`match Some("hi") { Some(x) => Ok(x), None => Err(2) }` 得到
`Result(String, Int)`，交换分支顺序结果相同。`None` 等没有提供全部参数证据的值
可使用类型注解、`.ty!(Ty)` 或 `@[Ty]` 补全参数。未知成员、不同类型族和不兼容
payload 都会产生类型错误。

### enum payload 不能是匿名 Struct 类型

Enum variant payload 是 TypeMetadata 表达式。`struct { ... }` 只允许作为直接
`type` 初始化器，因此匿名 Struct 不能嵌入 payload；应先声明具名 Struct：

```telora
# 不支持：struct 初始化器不能嵌入 enum payload
# type Expr = enum {Column(struct {alias: String, column: String})};

type ColumnRef = struct {alias: String, column: String};
type Expr = enum {Column(ColumnRef)};

# 值位置的匿名记录仍然合法
let expr: Expr = Expr.Column({alias: "o", column: "id"});
```

### Family 与递归具体类型

递归 enum/struct 在函数契约、参数化 family 契约和模块接口中保持精确类型。
Family 可以引用已经封闭的非参数化递归具体类型：

```telora
type Expr = enum {Literal(Value), Call(CallExpr)};
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
type Expr = enum {Literal(Value), Call(CallExpr)};
type CallExpr = struct {name: String, args: Array(Expr)};

type Renderer(Context) = struct {
    render: Fn(Context, Expr) -> String,
};
```

同一递归代数只需要替换叶节点类型时，优先使用上述同参递归 family。若递归过程中
必须改变参数，分别声明封闭递归类型或先把允许叶节点建模为闭合 enum。

### 复杂 family 值的 codec witness

`codec.encode(Value, value)` 的首个参数固定为公共 Value witness；编码直接返回
`Value`，失败产生诊断。codec 从输入
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
调用和包含封闭递归类型参数的 family。Alias 和函数契约由静态检查，witness 来自明确的类型元数据。

### Bytes 没有默认 JSON 表示

公共 Value 可以显式携带 `Value.Bytes(bytes)`，YAML `!!binary` 也映射到该 variant；但
JSON 没有原生 Bytes 类别，`json.stringify` 和 schema 不为 Bytes 选择隐式文本编码。
包含裸 `Bytes` 的类型不能作为完整 JSON text/schema 边界。设计需要稳定 JSON
codec/schema 的数据模型时，当前应从公共 `Val`、Model、Plan 和输出类型中排除 Bytes：

```telora
type Val = enum {
    String(String),
    Int(Int),
    Float(Float),
    Bool(Bool),
};
```

若应用要求本身必须携带二进制数据，把它记录为当前模型无法覆盖的边界，不自行
选择 Base64、tagged object 或其他协议。不要用 String 假装 Bytes，也不要通过手写
JSON 或 `Dyn` 绕过该限制。Array 元素、enum payload 和根 Bytes 同样没有
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

每个插值表达式都必须实现 `std/fmt.Display`；String、Int、Float 的实现
由标准能力提供。编译器静态选择 implementation，并把插值降低为普通 dictionary
member 调用。`Dyn` 必须先显式投影，插值处需要已确定的类型及其 Display 实现。Bool 和其他
具名 enum 不会因运行时使用 Atom 表示而自动获得 `Display`。Float 使用有限
binary64 的稳定文本表示：最短、可往返、不受
locale 影响；`3.0` 显示为 `3`，`-0.0` 显示为 `-0`，原始小数或指数拼写不会保留。

没有 `Display` implementation 的 Enum、Struct、Array、Dict、Tuple、Dyn 或用户值
不能插值。enum 值可以先通过 `match` 得到明确文本；Array/Dict
也可以显式 `array.map` 后使用 `string.join`。不要为了展示而把 Query binding 插入
SQL；仍应保持 `{ sql, bindings }` 的参数绑定边界。

`std/fmt` 是独立的显式 TypeMetadata-driven 能力：

```telora
import "std/fmt" as fmt;

@fmt.display_by("{host}:{port}")
type Endpoint = struct { host: String, port: Int };

def endpoint_text: Fn(Endpoint) -> String = fn(endpoint) {
    `endpoint=\{endpoint}`
};
```

`display_by` 是受控模板 eDSL：它发布 `DisplayBy` typed property，标准 blanket impl
据此为 `Endpoint` 提供 `Display` evidence。模块加载时会准备一个捕获固定字段 index 与
Display closure 的普通函数；插值路径只投影固定字段并调用这些 closure。
模板字段支持 String、Int、Float 和嵌套 `DisplayBy` struct；模板解释器不会动态选择
字段类型的其他显式 `Display` impl。
`Display.display` 和 `fmt.display`
返回 opaque `Fmt`；显式物化文本写作
`fmt.render(fmt.Display.display(endpoint))`。`fmt.concat(strings, items)` 要求
`strings.len == items.len + 1`，并以延迟 fragment 组合常量文本与展示值。插值在
整个结果的末端只物化一次。Fmt payload 和最终 UTF-8 输出都计入 allocation quota；
payload 在复制前预扣，最终输出在分配前按共享节点 memoize 测量。重复引用同一
fragment 仍按每次展开的长度核算，但拒绝路径不会实际展开指数大小的结果。这套机制
是静态 dictionary elaboration，不会把模板转换成 Telora 源码。

`blame!(message, subjects...)` 构造 `std/blame.BlameError`，保存 String 消息及任意
类型原值的来源，不产生诊断。BlameError 是不透明 native 类型，可以保存和跨模块
传递，其消息和来源不能作为字段读取。`raise!(error)` 发出失败并返回 Never；
`warn!(error)` 发出 warning、继续执行并返回 `None`，所属 `Option(T)` 的 T 由上下文
确定。两者都在发出诊断时补上当前 rule 位置，保留创建错误时选择的原值来源。
`fail!(message, subjects...)` 等价于构造 BlameError 后立即 raise。

```telora
let error = blame!("invalid value", candidate);
let observed: Option(Int) = warn!(error);
raise!(error)
```

`dbg!` 的 `repr` 是运行时专用、有界且 cycle-safe 的观察文本，不进入 Telora String；
codec/JSON 是数据交换协议，也不是展示 API。Float 的 debug repr 会保留 `3.0` 和
`-0.0`，有意不同于插值及 `fmt.render` 的 `3` 和 `-0`。

```telora
def check_capability: Fn(Subject) -> Result(Capability, String) = fn(subject) {
    match find_capability(subject) {
        Some(capability) => Ok(capability),
        None => Err("missing capability"),
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
- `fail!(message, subjects...)` 产生失败；规则归因到 authored caller，subjects 按参数
  顺序提供数据来源。直接调用时 caller 就是 `fail!` 自身。
- `panic!(message)` 只用于实现错误或不变量破坏。

`for` 契约引入的类型参数在对应实现体的局部标注、嵌套类型应用和内层闭包注解中
可见。仅为产生诊断且输出类型难以从上下文推断时，模块级同类型辅助 checker 仍然
有助于保持精确类型：

```telora
def reject_same: for(A) Fn(A, String) -> Result(A, String) =
    fn(evidence, message) { Err(message) };

let ignored = reject_same.should_ok!(subject, "missing capability");
```

### 面向契约的失败模式

函数的公共契约承诺返回 `T` 时，普通写法是直接返回 `T`；当当前输入无法产生一个
合法的 `T` 时，使用 `fail!(message, subjects...)`：

```telora
def make_plan: Fn(Model, Request) -> Plan = fn(model, request) {
    let checked = check_request(model, request);
    if checked.valid {
        assemble_plan(model, checked)
    } else {
        fail!("request cannot produce a complete plan", request, checked)
    }
};
```

普通 Telora 调用者不接收或处理诊断对象。它只表达值依赖、成功结果和失败位置；
求值器与运行时适配器依据这些依赖保留来源、跳过失败值的依赖计算，并尽力继续彼此独立的
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
哪些诊断计算；只要出现任何 error，本轮命令就不会发布结果，即使干净的最终根仍可算出，
codec、最终返回值和 SystemEffect 也不会越过运行时发布边界。Module 在
WorkWorld/MainWorld 间的内部固化不是对外发布，可以保留 Fail。需要业务恢复时仍显式使用
`Option`、`Result` 或领域 enum。

依赖库失败时仍是普通 Module。Module 的 `Available/Unavailable` 只表达源码是否存在；
定义和表达式分别携带 `Known/Unknown/Incomputable` 等事实状态。内部 Module 可以同时保留
健康 export 和含 Fail 的 export：下游读取健康项可继续工作，读取失败项则传播同一个 Fail。
没有 `PartialModule`/`UntrustedModule` 语言实体，也不会把原始 error 降级。

`Fn(Input) -> Output` 可以通过 `fail!` 报告无法产生结果的原因，诊断记录和发布
状态由运行时管理。Telora 调用者需要恢复、分支或组合失败时，使用 `Option`、
`Result` 或领域 enum 建模。`panic!` 表示实现错误或不变量破坏。

## 模块

```telora
import "std/array" as array;
import "@src/local" { compile };
import "plan-lib/types" as types;

def defaults: Defaults = do {
    let base = load_defaults();
    normalize(base)
};
export def compile: Fn(Input) -> Output = fn(input) { ... };
export { Entity, Requirement, compile };
```

import 是静态的。模块顶层是声明空间，只接受 import、type、trait、impl、decl、def、
native 和 export；普通模块值也使用 `def`。`let` 只用于函数或 `do` 等普通 block
中的顺序计算和局部 shadow。顶层 `let`、`export let`、裸表达式和 final expression
都不合法。模块只暴露显式 export。库必须导出向调用者承诺的每个类型和函数。

默认 prelude 提供 `PropertyAttr`、`True`、`False`、`Some`、`None`、`Ok` 和 `Err`。
它相当于可遮蔽的隐式 open import；本模块声明和显式模块导入优先，其他名字由
prelude 提供 fallback。这些名字不是保留字，本地 binding 可以正常使用同名；
仍需访问内建项时使用显式别名，例如
`import "std/prelude" { PropertyAttr as BuiltinPropertyAttr };`。

`src/` 下的文件由 crate module 清单发布；`tests/` 下的入口由 Host 以 `@test/...`
选择，`telora test NAME` 是对应的测试命令。测试支持子目录，选中入口时 Host 建立
整个 `tests/` 的临时清单，只预扫描和求值可达模块。测试通过 `@test/...` 或相对路径
互相导入（包括顶层测试），通过 `@src/...` 导入源码；源码不能反向依赖测试，循环
import 仍被拒绝。`test` 初始化完成后执行入口直接公开导出的 `std/test.Test`。
`should_ok`、`should_fail`、`should_fail_with` 保存 thunk，`with_fixtures` 保存
数据源和返回 Test 的 factory；构造时不执行，不读取 fixture。普通导出函数仍是
helper。完整规则见 [CLI 指南](TELORA-CLI.md)。
模块既可以使用显式源码根路径，也可以使用相对路径：

```telora
# src/app.telora
import "@src/compiler" { compile };
```

稳定逻辑模块 ID 与 crate 布局一一对应：

```text
@src/model       -> <crate>/src/model.telora
@test/compiler   -> <crate>/tests/compiler.telora
plan-lib/types     -> <plan-lib>/src/types.telora
```

CLI 从当前目录向上查找最近的 `telora-config.json`，因此可以在 workspace 内运行
命令。`run @src/app:run` 选择普通模块的 `run` export；该值必须是
`entry.Run(State)`。工具解开 wrapper、初始化具体 State、投递一个 Request，并把 Reply
中的 Value 编码为 JSON。环境与输入由 `entry.ContextConfig` 显式声明，不形成 ambient
binding。

普通 module 的纯结果使用 `telora eval module:name`；带显式 source、环境变量
白名单和字符串参数的纯函数使用 `eval-with`。两者都要求返回 `Value`，并且不创建 Entry
或 effect loop。完整示例：

```text
telora -C examples/my-crate run @src/app:run
telora -C examples/my-crate serve @src/app:serve --bind stdio://
telora -C examples/my-crate eval @src/model:answer
telora -C examples/my-crate check @test/compiler
```

`serve --bind stdio://` 的每行响应包含 `ok`、`error` 和 `diagnostics`。请求成功或
产生可恢复诊断后服务均继续运行；当前响应中的诊断项稳定公开 `message`。初始化失败、
协议失败和资源类 terminal failure 仍由运行时适配器带外报告。

`check` 用统一 Module 管线的 best-effort 策略求值所选模块；任何 error 都会非零退出，
但内部图仍可保留以查询健康事实。它不进行 Entry 调度，也不会调用已经
导出的函数，因此不等价于行为验收。纯导出由 `eval` / `eval-with` 验收，应用 service
由普通 `run` 严格执行；遇到
失败时可以用 `run --best-effort` 扩大诊断覆盖，并检查非零退出、CLI 诊断和无
output。不能仅以 `check` 成功作为行为证据。

在 test 入口中，`./compiler` 以及其他 `./` 或 `../` import 非法。
在 `src/` 下的模块中，相对 import 合法，并从导入模块的逻辑目录解析。
`@src/` 始终从导入模块所属 crate 的源码根解析。`plan-lib/types` 等 package 路径只能
选择当前 crate manifest 声明的直接依赖；`std/...` 选择 Telora 内置模块。CLI 在模块图
发现前依据 config 与 lock 准备完整的 crate-name 到物理 root 映射；resolver 随后按
crate 粒度建立清单，builtin crates 在先，当前 crate 和直接 dependencies 随后。已登记的
crate name 及 source 指向不再改变。
依赖只公开自身的普通 `src/` 模块。

## 递归与有界工作

Telora 支持带显式契约的递归函数。调用和 back-edge 消耗 fuel；分配也受配额
限制。算法必须暴露其契约要求的任何语义深度界限，而不能依赖运行时 fuel
最终耗尽来终止。

## 编程建议

- 通过 selector 和回调保留精确的泛型类型。
- 嵌套回调约束不足时，优先使用带显式契约的小型具名辅助函数。
- 应用事实和物理映射留在可复用方法库之外。
- 用泛型参数和明确的输入输出契约表达类型关系。
- 优先让类型表达静态约束；动态失败使用 `fail!` 并携带原始证据。
- 纯导出使用 `eval` / `eval-with` 验收，应用 service 使用严格 `run` 验收；失败排查时
  再使用 `--best-effort` 扩大诊断覆盖。
