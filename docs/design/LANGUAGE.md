# Telora 语言设计

本文档是 Telora 当前语言设计的单一事实来源。它从语言使用者、库作者、工具和
Host 能够观察到的语义出发，定义语言表面、静态语义、求值模型、模块边界、诊断、
资源限制和 Host 协议边界。

核心概念与术语见 [`CONCEPT.md`](CONCEPT.md)，问题与动机见
[`../MOTIVATION.md`](../MOTIVATION.md)，文档的权威关系与维护规则见
[`../README.md`](../README.md)。具体写法与编程指导属于
[`../../tutorial.md`](../../tutorial.md)。

本文描述当前成立的语言，不记录设计演进过程，也不讨论备选方案。它不是完整的
形式化语法、标准库 API 索引或版本兼容性承诺；这些内容分别属于语法定义、标准库
文档和版本政策。其他文档与本文冲突时，冲突本身需要通过同步设计或实现来消除。

## 1. 语言定位

Telora 是一门在封闭世界中进行不可变数据计算的静态类型语言，并可作为领域 eDSL
的承载语言。一次执行发生在由 Host 预先封闭的世界中：可执行代码、静态数据模块、
依赖以及显式运行时输入都在执行前确定。程序计算普通值；只有 Host 能把某个结果
解释为进程计划、构建产物、查询计划或其他具有外部意义的对象。

语言的主要用途不是一般应用编程，而是把高层表示验证并 lowering 为更具体的制品：

```text
静态模块 + 显式输入
  -> 类型检查和元数据计算
  -> 有界的纯数据计算
  -> 值、结构化失败或来源化诊断
  -> Host 决定是否发布或解释结果
```

当前语言具有以下基础性质：

1. 模块依赖静态解析，不支持运行时选择的 import 或 `eval`。
2. 普通值不可变，程序不能直接访问文件、网络、时钟、进程或环境。
3. 相同的封闭输入和资源预算应得到相同的值、失败及规范顺序的观察。
4. 递归被允许，但求值受 fuel、栈、调用深度、分配和取消限制。
5. 值和类型元数据可以保留来源，诊断可以关联输入与规则位置。
6. 严格执行只发布完整结果；失败、Error 诊断和过期分析不能发布部分状态。
7. 机制优先：领域政策应写成库；只有一般语言模型无法忠实表达的能力，才是
   核心机制候选。

这里的“纯”需要精确定义：普通值计算没有外部效果。`should_ok!`、`try_unwrap!`、
`must_ok!` 和 `fail!` 可以产生 Host 可见诊断，因此属于 Telora 求值的受控可观察
行为。`dbg!` 则是 Host 对求值的旁路观察：Telora 内部世界不能感知 Host 是否安装
observer、是否输出事件或是否截断表示。

## 2. 源文件和词法表面

Telora 源文件通常以 `.telora` 结尾。`#` 引入行注释；文件开头可以包含 shebang。
空白和注释不影响求值，但 lossless parser 会保留它们以供工具使用。

基础字面量包括：

```telora
42                         # Int
0.75                       # Float
1e308                      # 使用十进制指数记法的 Float
"text"                     # String
b"bytes"                   # Bytes
r"raw text"                # raw String
r#"a "quoted" value"#      # 带 delimiter 的 raw String
'Ready                     # Atom
```

普通字符串支持转义和显式续行。反引号字符串是结构化连接表达式，使用 `\{...}`
嵌入表达式：

```telora
let greeting = `hello \{name}`;
```

插值支持 String、Int、Float 和 Atom，不是隐式调用任意用户 `Display`。String 保持
原文本，Int 使用十进制表示，Atom 省略前导 `'`。Float 使用有限 binary64 的 Display
表示：与 Rust `f64` 的 `{}` 一致，选择能往返到同一 binary64 值的最短十进制文本，
不受 locale 影响。该表示保留负零的符号，但不保留整数值的小数点，例如 `3.0` 表示
为 `3`，`-0.0` 表示为 `-0`。输出不保留字面量的原始小数或指数拼写，而由同一
Display 规则选择十进制或指数形式。

Bool 没有独立运行时类别。它是闭合的 Atom 类型，其值为 `'True` 和 `'False`。
条件位置只接受 Bool，不进行 truthiness 转换。

Float 是有限的 IEEE 754 binary64 值。字面量可以写成 `digits.digits`、
`digits exponent` 或 `digits.digits exponent`；exponent 使用 `e` 或 `E`，并可带
`+`/`-` 号。舍入为 `NaN`、`+Inf` 或 `-Inf` 的形式不是合法字面量；语言也不提供
这些值的关键字或特殊拼写。正负零、正规数和次正规数都属于 Float。

## 3. 运行时值

用户可观察的普通运行时值由较小的集合构成：

```text
Int, Float, String, Bytes,
Atom, Tagged,
Tuple, Array, Dict,
Func,
Type metadata, native opaque value, Dyn
```

### 3.1 集合和积

Array 是有序同质序列：

```telora
let ids: Array(Int) = [1, 2, 3];
let second: Int = ids[1];
```

`array[index]` 对 `Array(A)` 使用零基 `Int` 索引并直接返回 `A`。负数或超出范围的
索引执行等价于 `fail!("OutOfRange", array, index)` 的结构化失败；需要把缺失作为值
处理时使用总操作 `array.get(array, index) -> Option(A)`。

无 expected item type 的 Array 字面量对各元素类型做规范 join。不同的已知类型形成
Union，例如 `[1, "one"]` 的类型是 `Array(Int | String)`；`Never` 不贡献可达元素
类型，已有的 `Any` 则保持擦除。严格推断不会因为元素类型不同而自行制造 `Any`。
显式 `Array(T)` expected type 会向每个普通元素和 spread operand 下传 `T`。因此当
`T` 是 concrete family 实例时，多个匿名记录元素中的 singleton Atom、闭包、窄
Option variant 和空集合都按完整的共同契约检查，而不是先各自形成 variant union。
该检查与元素顺序无关；真正不兼容的字段在对应元素位置报告类型冲突。
在 `if`、`if let` 或 `match` 的结构化分支结果中，同一 Array 或 Dict 元素位置的具体
分支为无元素分支提供类型证据；该合并与分支顺序无关。若所有可达分支均无元素且
没有 expected item type，必须用类型注解提供元素类型。

Tuple 是固定长度的异质积：

```telora
let entry: Tuple([String, Int]) = ("port", 8080);
```

`Tuple` 是接收单个 TypeMetadata Array 的普通元数据构造器。该形式在类型 alias、
显式元数据表达式和受限契约中一致；`Tuple(A, B)` 不是 Tuple 类型的另一种写法。

Record 字面量与 `Dict(T)` 在运行时都使用 String key 的 Dict 表示，但静态意义不同：

```telora
let user = {name: "Ada", active: 'True};
let labels: Dict(String) = {region: "east", tier: "gold"};
```

Record 的字段集合属于静态结构；`Dict(T)` 的 key 集合可以动态变化，所有 value 具有
同一静态类型。Dict 的无领域顺序观察和序列化采用规范顺序。

字段投影保留静态证据：已知 Record/Struct 必须声明该字段，`Dict(T)` 的字段结果是
T，Union receiver 的每个可达 variant 都必须允许投影并对结果做规范 join。已知的
非记录值和 `Dyn` 会产生静态诊断；只有 receiver 本来就是 `Any` 时，字段结果才是
`Any`。暂未确定的 receiver 可以在同一个单态推断边界内积累字段 obligation，后续
具体证据必须满足全部字段；这种 obligation 不会泛化或发布为开放 row constraint。
若推断边界结束时仍无具体证据，程序必须提供参数或函数契约。Dict key 是否实际存在
仍在求值时检查，缺失 key 是可恢复的程序失败。

Array 和 Dict 支持 spread：

```telora
[0, ...items, 9]
{...defaults, mode: "strict", ...overrides}
```

Dict spread 按源码顺序合并，后出现的 spread 值覆盖先出现的值；同一字面量中重复
声明的具名字段是错误。Record 没有独立的可变 update 操作。

### 3.2 Sum 值

Atom 是无 payload 的符号。调用 Atom 构造带 payload 的 Tagged 值：

```telora
'None
'Some(1)
'Ok(value)
'Err(error)
```

Option、Result、Bool 以及用户 enum 都建立在 Atom/Tagged 表示上。类型元数据决定
合法 tag、payload 形状及静态穷尽性。

### 3.3 相等性和顺序

普通标量和结构值支持 `==` 和 `!=`。复合值按结构比较；函数按不透明函数身份比较，
而不是比较代码或闭包捕获内容。`!=` 是 `==` 的精确布尔补集。

`<`、`>`、`<=` 和 `>=` 只接受类型相同的 `Int`、`Float` 或 `String` 操作数；数值
之间没有隐式转换。Int 使用有符号数值顺序。Float 只包含有限 binary64 值，使用
通常的有限数值顺序，正负零相等。因此 Float 相等是自反的，不存在 unordered 比较。

String 顺序是其内部 UTF-8 字节序列的字典序：第一个不同字节较小者在前；若一方是
另一方的前缀，较短者在前。不执行 Unicode normalization、locale collation、
case folding 或自然数排序。

所有六种比较运算符处于同一非结合优先级；连续比较必须用括号明确分组。比较运算
不执行用户定义的 trait 查找，因为当前语言没有 trait 系统。

## 4. 表达式和控制流

Telora 是 expression-oriented 的。Block 中的最后一个表达式是 block 的值：

```telora
let result = {
    let adjusted = value + 1;
    adjusted * 2
};
```

当前表面包括算术、比较、短路布尔运算、field selection、调用、pipeline、条件、
模式匹配、传播和显式返回。主要运算符包括：

```text
!x  -x
*  /  %  +  -
&  ^  |
<  >  <=  >=  ==  !=
&&  ||
|>
```

`!` 对 Bool 返回相反的 canonical Bool，对 Int 执行按位取反；操作数只求值一次。
`&`、`|` 和 `^` 只接受两个 Int，并分别执行按位与、按位或和按位异或。Int 位运算
使用其有符号二进制补码表示。位运算的优先级依次为 `&`、`^`、`|`，低于算术、
高于比较。前缀 `!` 和数值负号结合得更紧。`&&` 和 `||` 只接受 Bool 并短路。
`left |> right` 统一降低为 `right(left)`。

`%` 与 `*`、`/` 处于同一优先级并左结合，接受两个类型相同的 Int 或 Float。
它使用截断商余数：`r = left - trunc(left / right) * right`。因此非零余数与左操作数
同号，且其绝对值小于右操作数的绝对值；例如 `-7 % 3 == -1`、
`7 % -3 == 1`。Int 的零除数产生 `DivisionByZero`；最小 Int 对 `-1` 求余产生
`IntegerOverflow`。

Float 的 `+`、`-`、`*`、`/` 和 `%` 执行 binary64 运算，但结果必须仍是有限 Float。
如果结果为 `NaN`、`+Inf` 或 `-Inf`，求值执行等价于
`fail!("NonFiniteFloat", left, right)` 的结构化失败；两个操作数按源码顺序各求值
一次，完整运算表达式是 rule origin。Float 除以或对正零、负零求余也使用这一失败，
而不是 Int 的除零错误。Float 一元负号保持有限域不变。

`!` 的 Bool/Int 重载由已知操作数或期望结果类型选择；两者都未知时不任意默认。
通过 `Any` 边界但结果已约束为 Bool 或 Int 时，运行期仍检查所选择重载的输入。
只有输入和结果都显式保持 `Any` 时，运行期才在 Bool 和 Int 两种语义之间分派。

Tuple 使用非负十进制字面量做位置投影，例如 `(left, right).0`。位置投影是可连续
组合的后缀操作；`value.1.0` 等价于 `(value.1).0`，也可继续接 field selection、
index 或调用。已知 Tuple 的位置在分析期检查并得到精确成员类型；通过 `Any` 边界的
Tuple 在运行期检查类型和范围。

`if` 必须有 `else`，两个分支产生可合并的类型：

```telora
if enabled { "on" } else { "off" }
```

`ctrl_block` 是普通 block、`if`、`if let`、`match` 或 `return expression;`。`if` 和 `if let` 的
`else` 接受一个 `ctrl_block`；非普通 block 的形式规范化为只含该控制流表达式的
block。因此 `else if` 可以连续使用：

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

提前返回同样可以直接作为 `else` 分支：

```telora
if ready { value } else return fallback;
```

`return expression;` 从最近的函数返回。它不是模块导出机制。

### 4.1 模式匹配

Pattern 可以匹配字面量、Atom、Tagged payload、Tuple 和 Struct 字段：

```telora
match result {
    'Ok(value) => value,
    'Err(message) => fail!(message, result),
}
```

Match arm 可以带 Bool guard。只有无 guard 且覆盖确定的 arm 才参与穷尽性和冗余性
证明。对闭合 enum 的匹配执行保守穷尽性检查。

局部绑定还支持：

```telora
if let 'Some(value) = candidate { value } else { fallback }

let 'Some(value) = candidate else {
    return fallback;
};
```

普通解构 `let` 要求 pattern 对已知输入形状不可失败；`let ... else` 的 else 分支必须
发散，成功路径才会获得 pattern binding。

### 4.2 Option/Result 传播

后缀 `?` 对 Option 或 Result 做同家族的提前传播，传播边界是最近的函数或模块
block。不同家族不能混合传播：

```telora
def parse_pair: Fn(String, String) -> Result(Tuple([Int, Int]), String) =
    fn(left, right) {
        let a = parse_int(left)?;
        let b = parse_int(right)?;
        'Ok((a, b))
    };
```

`?` 是 surface elaboration，最终执行普通 match 和 return 控制流。

## 5. 绑定、函数和递归

`let` 定义词法局部值；`def` 定义具名模块 binding；`decl` 可以先声明契约；`native`
只允许在受信 native module 中声明 Host 实现：

```telora
let local = 1;

def add: Fn(Int, Int) -> Int = fn(left, right) {
    left + right
};
```

函数是一等不可变值，可以捕获词法环境。参数和返回值可以显式标注；调用 arity 是
静态和运行时契约的一部分。尾位置的 bytecode 函数调用支持 proper tail call。

模块级 `def` 按依赖 component 分析。无环 definition 可以按依赖顺序推断和泛化；
递归及相互递归 definition 在没有显式泛型契约时保持单态，并通过固定点约束获得
可证明的函数形状。

单个 binding 的 alias 只实例化其右侧泛型一次。它不会自动获得任意 let-polymorphism。

Call section 使用占位符构造 closure，是普通函数的便利表面；显式 `fn` 始终可以表达
同一计算。

## 6. 静态类型

Telora 使用结构化静态类型和双向检查。当前公开类型类别包括：

```text
Int, Float, String, Bytes
Atom 与 Tagged
Array(A), Dict(A), Tuple([...])
Struct, Enum, Union
Fn(...) -> ...
Type, TypeOf(A), Dyn, opaque native type
Any, Never
```

Record/Struct 类型按字段结构检查；type alias 不创建新的名义身份。Native opaque type
具有由注册模块和 slot 决定的名义身份，普通用户代码不能伪造其值。

`Any` 表示源码契约或 Host 边界显式放弃静态精度。严格推断不会用 `Any` 代替已知
类型之间的冲突；它也不是 editor 因源码损坏而暂时不知道类型的状态。
`Never` 表示不产生值的路径，例如 `return`、`fail!` 和 `panic!`；它作为 bottom
参与方向性检查，避免根失败制造级联类型错误。

### 6.1 函数契约和 rank-1 多态

函数契约使用专用的 `Fn(P1, ..., Pn) -> R` 记法。它精确降解为普通元数据构造
`Func([P1, ..., Pn], R)`；`Fn` 不是值环境中的 callable，`Func` 才是构造函数元数据的
普通内建 callable。参数和结果位置都递归接受完整契约记法，例如：

```telora
Fn(A) -> Tuple([B, C])
Fn(Fn(A) -> B) -> Array(Tuple([A, B]))
Fn(types.Input, Array(types.Item)) -> types.Output
```

契约中的类型名接受模块限定路径；限定路径在参数、结果、嵌套 family 实参和普通
类型标注中的含义一致。消费者可以用 whole-module alias 保持类型 namespace，
不必仅为函数契约额外选择性导入未限定类型名。

显式写作 `Func([A], B)` 与 `Fn(A) -> B` 产生相同的规范 TypeMetadata。`Fn([A], B)`
不是显式构造形式，也不被当作旧语法兼容。

显式多态写为：

```telora
def identity: for(A) Fn(A) -> A = fn(value) { value };
```

定义 body 必须对刚性的每一个 A 成立。调用时可以推断类型参数，或者使用
`@[...]` 显式应用：

```telora
identity@[Int](1)
pair@[Int, _](1, "text")
```

`_` 留下一个必须由完整调用上下文解决的参数。无法解决或证据冲突都会产生诊断。
类型参数从完整调用收集证据；裸 closed Atom 实参不会在同次调用的结构化或完整 enum
实参之前把共享参数固定为 singleton。若后者确定了包含该 Atom 的闭合 enum，调用采用
该 enum；不相关 enum 或 enum 之外的 Atom 仍是类型错误。
匿名 Struct 实参同样在完整调用上下文中检查。若 generic callback 的结果确定了共享
Struct 类型，较早书写的 seed 中的 singleton Atom 字段和空 collection 字段按该结果
检查并拓宽；例如 fold callback 返回 Bool 时，seed 的 `{flag: 'False, items: []}` 可
参与 `{flag: Bool, items: Array(A)}`。若 callback 产生多个同形 Struct variant，联合
保留字段之间的相关性，seed 只在唯一兼容 variant 中补全未定字段。不相关 Atom、字段
shape 冲突和不唯一的补全仍是类型错误。
由 closure 字面量初始化的未标注局部 binding 可以从后续 generic call 获得 expected
function type。若 closure 分支产生的 variant union 完整映射到 expected 闭合 enum，
各分支参与 enum payload 推断，例如 `'None | 'Some(String)` 可精化为
`Option(String)`；未知 variant 或不兼容 payload 仍产生类型错误。
未标记的 `expression[index]` 只表示 Array 索引，不表示类型应用。

显式 `def` 契约作为 rigid expected type 参与严格双向检查；这同时适用于 inline
契约和先 `decl` 后初始化的定义。工具阶段的 shallow projection 可以产生 provisional
fact，但不能用其中为恢复而保守引入的 `Any` 在严格检查前否决定义。

未标注、由 closure 字面量初始化的合格局部 binding 可以得到保守 rank-1 scheme。
Generalization 保留尚未解决的 callable 和数值 obligation；递归组、不稳定约束以及
普通 alias 不会被无条件泛化。跨模块导出的 scheme 会在每个合法使用点重新实例化，
且其私有 bound identity 不泄漏到导入方。

当前不存在 higher-rank type、subtyping、trait、interface、associated type、
higher-kinded type 或通用 constraint resolution。

## 7. 类型元数据

Telora 的类型不是仅存在于编译器中的标签。类型声明求值一个表达式，结果必须是
规范 TypeMetadata。理解该模型需要区分：

```text
Type       任意有效 TypeMetadata 值的静态类型
TypeOf(A)  精确证明某个元数据描述 A
TypeDesc   用户态解释器观察的擦除后 descriptor 视图
```

`TypeDesc` 在这里表示公开观察模型，不是与 `Type` 并列的可构造静态类型。当前
`std/type-desc` observer 接受 `Type` 值，并通过 kind、children 和 resolve 等操作
暴露该擦除视图。

内建元数据构造器也是普通 callable 值：

```telora
Array(String)
Func([Int, Int], Int)
Option(String)
```

规范函数元数据的公开 descriptor kind 是 `'Func`，其 `parameters` 是 TypeMetadata
Array，`result` 是单个 TypeMetadata。`std/type-desc` 和 `std/dyn` 对函数元数据的 kind
观察均返回 `'Func`。`Function` 不具有语言保留意义，可以作为普通领域标识符使用。

类型 annotation、decorator 和 `type` initializer 在工具阶段由同一套 VM 求值。工具
阶段与程序阶段共享函数语义、值模型、fuel 和失败规则；区别在于 Host 调用它们的
目的和允许发布的结果，而不是存在第二门类型级语言。

### 7.1 Struct、Enum 和 decorator

常见声明使用普通 metadata decorator：

```telora
@struct
type User = {
    id: Int,
    name: String,
};

@enum
type Status = {
    Pending: 'None,
    Failed: String,
};
```

`@struct` 和 `@enum` 不是独立的 class/ADT 编译通道；它们调用 metadata-to-metadata
函数，将普通字面结构规范化为相应 descriptor。其他 decorator 可以添加 attribute，
供 codec、schema、文本表示或用户态解释器消费。

Decorator 应保持确定和纯粹。失败的 metadata 构造不能发布部分类型图。

### 7.2 参数化 TypeMetadata family

参数化 `type` 声明创建可命名的元数据 family：

```telora
@struct
type Box(Item) = {value: Item};

def wrap: for(Item) Fn(Item) -> Box(Item) = fn(value) {
    {value}
};
```

`Box` 同时具有两个表面：

```text
类型位置  Box(A)
值位置    for(A) Fn(TypeOf(A)) -> TypeOf(Box(A))
```

声明 body 使用刚性符号参数求值一次，产生规范模板；`Box(String)` 对模板做避免捕获
的替换，不用 String 重新执行 body。因此 family 不是任意 type-level function，也
不是 higher-kinded nominal constructor。

Family 必须一次提供全部参数，不支持 partial application 或参数化递归。无环 family
可以组合本模块中的其他 family 和非参数化 concrete type，也可以跨完整、选择性、
alias 和 open import 保留精确 scheme。Family 可达的本地 TypeMetadata 依赖按语义
依赖图求值，声明顺序不影响结果；包含 family 的循环 component 是错误。

Family 声明可以捕获已经封闭的非参数化递归 concrete type，但不能依赖同模块的
普通 helper；可以依赖内建 metadata 构造器和 imported metadata 能力。这一边界
避免把普通源码求值顺序或尚未 sealing 的 recursive reference 带入符号模板。

这一拒绝是稳定的语言边界，不是待补齐的推断 fallback。Family 是有限符号模板，
递归 concrete type 是预声明并 sealing 的有限图；参数化递归 back-edge 还需要携带
family 应用及参数环境，无法表示为普通 `Bound` 替换或 concrete `Named` reference。
语言不通过 eager unfolding、深度截断、`Any` 或按 concrete 参数重跑 family body
近似该能力。需要变化叶节点集合时，声明不同的封闭递归类型或一个包含全部受支持
variant 的闭合 enum；需要共享行为时，在递归数据之外参数化 renderer、visitor、
capability、policy 或 dialect。

### 7.3 递归元数据

递归类型使用有限图和受控 reference 表示，而不是无限展开的树。成功初始化的递归
root 在冻结后发布；未初始化 reference 不能进入 persistent world。

用户态 descriptor observer 可以识别 Ref 并显式解析。遍历不伴随具体值时，解释器
必须自行处理循环；遍历有限、无环的运行时数据时，可以让普通值递归提供终止进度。

### 7.4 Dyn 和 `interpreter!`

`Dyn` 是 existential package：它把一个值、权威 descriptor 和来源绑定在一起。
普通代码可以用匹配的 `TypeOf(A)` 与 A 安全打包，但不能从 Dyn 未经检查地恢复任意 A。
结构 observer 同时派生子 descriptor 和子值，避免把不相关的类型和值拼在一起。

`interpreter!` 将擦除的消费型函数提升为带静态 witness 的 API：

```telora
def show_dyn: Fn(Dyn) -> Result(String, String) = ...;

def show:
    for(A) Fn(TypeOf(A)) -> Fn(A) -> Result(String, String) =
    interpreter!(show_dyn);
```

提升要求显式的 `for` 契约和每个类型参数的 `TypeOf` witness。只有直接出现为内层
参数的 A 会被打包成 Dyn；包含 A 的 `Array(A)`、`Fn(A) -> B` 等位置不会被递归适配。
结果不得包含被解释的 A，因此该机制不能制造 `A`、`Option(A)` 或类似值。

该 lifting 的可观察语义等价于构造普通 closure，并使用相应 witness 将直接 A 参数
安全打包为 Dyn。它不是 macro system、代码生成器、trait derivation 或动态 cast。

## 8. 模块和封闭世界

模块通过静态路径组成不可变有向图。Import 必须在初始化前解析，程序不能根据运行
时值决定依赖。

公开 import 表面包括：

```telora
import "std/array" as array;
import "@src/model.telora" { User, make_user };
import "./validation.telora" { validate };
import "package/path.telora";
import "package/path.telora" as model, { User };
```

支持 namespace、选择性、alias 和 open import。所有形式必须观察同一个 export
scheme，不能因导入写法不同而丢失泛型关系。

`import` 永远只在当前模块建立 local binding；它本身不改变当前 Module 的公共接口，
也不存在 implicit public import。只有显式 `export` 才把一个当前可见的 local binding
映射到 Module interface：

```telora
import "@src/types.telora" {Plan as LocalPlan};
export {LocalPlan as Plan};
```

`export {a as b}` 只建立接口映射 `local a -> public b`。它不在当前模块建立名为
`b` 的 local binding，因此后续本地表达式不能因该 export 而 resolve `b`。Public
alias 只供下游 import 和 Module member resolution 使用。

`export let`、`export def` 和 `export type` 是普通 local binding 后接 export marker
的语法糖。例如 `export def f = value;` 与 `def f = value; export {f};` 具有相同
语义。本地 `f` 由 `def` 建立；export marker 不建立 lexical binding、不执行用户代码，
只选择要发布的 local binding。

被 export 选中的 local binding 可以由当前模块声明，也可以由 selective、aliased、
open 或 namespace import 建立。导出 imported local 时保留原 value identity、精确
TypeScheme、concrete/recursive TypeMetadata graph、type-family template、opaque provider
identity 和 provenance；不包装、重求值或重建 binding。Namespace binding 的导出仍是
语义 Module，必须保留其 nested Module interface，不能退化为普通 Dict。

Telora crate 使用 `src/`、`src/bin/` 和 `tests/`。Host 从当前工作目录向上查找最近的
`telora-deps.json`，再通过稳定逻辑 ID 选择根：普通 source 是 `@src/<path>`，应用
入口是 `@bin/<path>`，测试入口是 `@test/<path>`。例如：

```telora
# src/bin/report.telora，逻辑 ID 为 @bin/report.telora
import "@src/model/report.telora" { compile };
```

逻辑 ID 到 crate 内物理位置的映射为：

```text
@src/x.telora       -> <crate>/src/x.telora
@bin/x.telora       -> <crate>/src/bin/x.telora
@test/x.telora      -> <crate>/tests/x.telora
dependency/x.telora -> <dependency-crate>/src/x.telora
```

`@bin` 和 `@test` 只能由 Host 选择，任何 Telora import 都不能引用它们。Binary 和
test 根也不能使用 `./` 或 `../`；它们必须以 `@src/` 导入本 crate 的可复用源码。
依赖仅公开其 `src/`，不公开 `src/bin/` 或 `tests/`。不存在 `@main` 身份。

`@src/` 以 importing module 所属 crate 为准，因此依赖模块中的 `@src/` 仍指向该依赖
自己的 source root。`./` 和 `../` 只表示 source module 或 dependency module 内部、
相对于 importing module 逻辑目录的导入。Package import 的首段选择固定依赖，
`std/...` 等 builtin identity 由 Host 注册。上述解析均在模块初始化前完成。

模块 import graph 不允许初始化 cycle。递归函数和递归 TypeMetadata 在单个模块及
已建立的模块接口内由各自机制处理，不把 module cycle 当作递归定义机制。

Production module 使用显式命名导出：

```telora
export let version = 1;
export def compile: Fn(Input) -> Option(Output) = fn(input) { ... };
export { User, compile };
```

模块没有由“最后一个表达式”定义的公共默认结果。Source module 的公共接口由
export record 定义，Host 再按所选协议校验或选择其中的值。

### 8.1 模块身份和依赖

模块的语义身份由 authority、crate 和逻辑路径决定，不等于偶然的物理绝对路径。
相同模块经不同相对边解析时应复用同一身份；resolver 拒绝词法或 symlink 越界。

Path crate 的依赖由根目录 `telora-deps.json` 固定。依赖名是 package import 的首段。
当前没有通用 registry 获取、版本求解或运行时 package acquisition。

普通 crate 模式的 resolver 配置只来自向上发现的最近一个 `telora-deps.json`；模块中
不得声明 `crate.dependency` 或 `crate.format` resolver options。`run -S` 的 standalone
模式不查找 manifest，其 resolver 配置只来自根文件内的 `crate.dependency` 和
`crate.format` options，并相对该文件的父目录解析；被导入的文件不能继续声明这些
options。其他静态 `option` 可声明模块或 Host 协议配置，必须位于 import 解析之前的
有效位置，嵌套依赖不能借此修改根 Host 的策略。

普通 `.telora` 模块可以公开导入；`.priv.telora` 受 crate 可见性限制；
`.native.telora` 只承载由 Host 注册、slot 明确的 native 声明。源文件声明 native
函数并不赋予自己实现外部能力的权限。

### 8.2 静态数据模块

JSON、TOML 和 YAML 与 Telora 文件共同进入模块图。它们不是运行时文件读取：Host
在封闭世界建立时加载并注册源码，解析结果是带字段来源的不可变值。

当前格式行为包括：

- JSON 严格解析数字、字符串和重复 key，并保留字段路径来源；
- TOML 支持 1.0 的核心值与表结构，日期和时间保留为不同 tagged representation；
- YAML 采用保守的 1.2 Core Schema，mapping key 必须是 String，拒绝 custom tag、
  merge key 及会引入歧义的行为；旧式隐式 bool 和时间戳按 String 处理。

解析失败的静态数据模块不能供严格执行使用，但 workspace recovery 仍保留其 source、
syntax diagnostic 和不依赖成功值的事实。

### 8.3 初始化和发布

模块求值区分 persistent main world 与每次操作的 temporary/work world。Import export、
closure capture、递归 metadata root 和 Host virtual module 只有在完整初始化后才能被
promotion 到持久世界。

Promotion 保留共享结构、递归引用和来源，并保证目标 heap 自包含。失败、取消、配额
耗尽或过期 revision 的 work world 被整体丢弃，不得留下半初始化 export。

## 9. 来源、失败和诊断

来源是值流的一部分。Telora expression、JSON/TOML/YAML 字段、imported value、
metadata application 和 codec normalization 都可以贡献 origin。来源影响诊断，
但不改变普通值的逻辑相等性。

### 9.1 Value-level outcome

Option 和 Result 是普通数据协议：

```text
Option(A)     预期缺失或可选证据
Result(A, E) 调用者必须显式处理的边界结果
```

它们不会自动产生 Host diagnostic，也不会自动使模块失败。

### 9.2 检查、解包和立即终止

表面语言提供七个 contextual intrinsic 名称：

```telora
dbg!(value [, "message"])
should_ok!(checker, arguments...)
must_ok!(checker, arguments...)
try_unwrap!(result)
unwrap!(result)
fail!(message, subjects...)
panic!(message)
```

对于 `checker: Fn(A1, ..., An) -> Result(R, String)`：

```text
checker.should_ok!(a1, ..., an) : Option(R)
checker.must_ok!(a1, ..., an)   : R
```

checker 和参数各求值一次，顺序从左到右。`should_ok!` 把 `Ok(r)` 变成 `Some(r)`；
`Err(message)` 产生一条 Warning，并以有序参数作为诊断证据，然后返回 `None`。
`must_ok!` 在 `Ok(r)` 时返回 `r`，在 `Err(message)` 时产生失败并得到 `Never`。
checker 可以没有参数，但不能省略 checker。

对已有的 `result: Result(R, String)`：

```text
result.try_unwrap!() : Option(R)
result.unwrap!()     : R
```

`try_unwrap!` 对 Err 产生 Warning 和 `None`；`unwrap!` 对 Err 产生失败和 `Never`。
result 只求值一次并作为诊断证据。两者不同于 `?`：`?` 只传播原 Option/Result 分支，
不产生诊断，也不改变容器家族。

`fail!(message, subjects...)` 产生失败和 `Never`。message 必须是 String，subjects
作为有序证据保留来源。`panic!(message)` 表示实现错误或无法恢复的不变量破坏，
而不是可预期的领域拒绝。

所有 contextual intrinsic 都支持统一的后置糖：

```text
receiver.ident!(arguments...) == ident!(receiver, arguments...)
```

例如 `check_order.should_ok!(a, b)` 等价于 `should_ok!(check_order, a, b)`，
`result.try_unwrap!()` 等价于 `try_unwrap!(result)`，`"OutOfRange".fail!(arr, idx)`
等价于 `fail!("OutOfRange", arr, idx)`。这只是把 receiver 放到第一个参数位置；
它不执行 method lookup，也不开放用户定义宏。未知 intrinsic 在前置和后置形式下
都被拒绝。

`BlameError` 是求值器与 Host 之间的 opaque native carrier。只有 `.native.telora`
中的 native ABI 声明可以引用它；普通 `.telora` 不能命名、构造、导入或导出该类型。
普通代码可以匹配 native 调用返回的推断错误值并读取 ABI 字段，例如：

```telora
match validate(User, raw) {
    'Ok(user) => user,
    'Err(error) => fail!(error.message, error, raw),
}
```

native module 不得 re-export `BlameError` 类型绑定。

### 9.3 Host debug observation

`dbg!` 临时观察一个显式表达式：

```telora
dbg!(value)
dbg!(value, "message")
value.dbg!()
value.dbg!("message")
```

前置和后置写法语义相同。首个参数只求值一次，`dbg!` 返回同一个运行时值并保留其
精确静态类型；可选 message 必须是 String literal。编译器同时记录首个参数的源码
文本、稳定 module ID 和调用行。`dbg!` 不捕获作用域中的其他变量。

观察使用有界、确定、cycle-safe 的 debug formatter。它不经过 `Any`、`Dyn`、codec
或值导出，表示也不是 JSON serialization contract。Host sink、格式化、截断或输出
失败不能改变 Telora 的值、失败、诊断、控制流、fuel、stack 或 allocation account。
Float 的 debug 表示与 Rust `f64` 的 `{:?}` 一致且不受 locale 影响；它与 Display
表示有意区分，例如 `3.0` 和 `-0.0` 的 debug 表示分别保留为 `3.0` 和 `-0.0`。

CLI 把每个事件作为一行紧凑 JSON 写入 stderr：

```json
{"name":"value","repr":"3","module":"@src/query.telora","line":42}
{"name":"plan","repr":"{...}","module":"@src/query.telora","line":43,"message":"generated"}
```

`name` 是首个参数的 authored expression text，`repr` 是有界 debug 表示。stderr 事件
不进入模块 export、stdout、诊断集合或内置 run Entry 的 `output` 发布协议。语言不
提供 `std/debug` 模块或 context-free `dbg` 函数。

### 9.4 Host-observed diagnostic

Warning 和 failure 诊断属于 evaluation account，而不是普通 Array 返回值。Host
负责排序、去重、渲染、JSONL 格式和退出协议；Telora 代码不能观察 Host 是否保存或
展示 Warning。`should_ok!` 与 `try_unwrap!` 产生非阻塞 Warning；`must_ok!`、
`unwrap!` 和 `fail!` 使当前结果不可产生。

在 best-effort 求值中，`fail!` 得到的内部 `Never` 会阻止所有依赖计算执行；Host
仍可继续已经证明独立的求值单元，以一次收集更多根因。Struct、Tuple、Array、tagged
payload 和 Dict 在诊断求值图中可以暂时保留失败子节点。这类节点保持原有静态类型，
但不是 Telora 值，源码不能构造、匹配或恢复它们。保形逐项操作可以跳过失败槽位并
继续健康槽位；只依赖容器形状的操作不依赖子节点；选择失败槽位则传播同一根诊断。
任何 error diagnostic 都会使本轮 World 导出整体失去发布意义，即使最终根值不依赖
内部失败且能够算出。失败节点的可达性只决定 best-effort 还能继续哪些诊断计算，不决定
结果能否交付。普通 module、codec 或 Host value 均不得发布本轮结果；严格执行遇到未处理
失败立即失败。

跨模块恢复时，依赖库可以在诊断图中保留为内部 `UntrustedModule` 状态，使其他不相关
依赖仍可继续分析；它不是普通 Module export，也不把原始 error 降级为 warning。命令所选
根模块在 finalization 时，只要其恢复图中存在 error，就以 `UntrustedModule` 失败并废弃
根导出。依赖边界不重复制造根因诊断。

### 9.5 失败类别

VM 区分可恢复的程序失败与终止整个 evaluation session 的资源/一致性失败。前者包括
类型不匹配、missing field、non-exhaustive dynamic match、panic 和 `fail!`；后者
包括取消、fuel/分配/栈/调用深度耗尽以及无效 bytecode。

Workspace recovery 可以在一个 binding 或模块失败后继续独立工作，但严格 Host
执行不会把 recoverable failure 当作成功值。

## 10. 求值和资源语义

当前工具链使用 lossless CST、AST/HIR、类型分析、LIR、bytecode 和寄存器 VM 实现
语言。各层共同服从本文定义的可观察语义和来源映射，不建立彼此竞争的语言模型。

### 10.1 两个阶段，一个求值器

Tool stage 执行 annotation、type initializer、decorator、module interface 和其他分析
所需的闭合计算。Program stage 使用显式 Host 输入执行普通应用函数。两者共用：

- bytecode 与函数调用规则；
- 不可变值和 heap 表示；
- fuel、stack、call-depth 和 allocation account；
- runtime failure 与来源规则。

静态 annotation 和 witness 默认从程序执行中擦除；当程序显式把 TypeMetadata 当作
普通值使用时，该值会保留到运行时。

`codec.encode(witness, value)` 等需要 `TypeOf(A)` 的运行时边界必须收到实际、受检查
的 TypeMetadata 值。语言不从普通值的运行时表示反射其已擦除静态类型，也不允许从
`Any` 或 `Dyn` 伪造 witness。复杂 concrete family 的定义模块应拥有一次完整实例化，
并导出 concrete alias 或 typed boundary function：

```telora
type Rejection = RejectionPayload(Entity, Dimension, Intent, Expr, Plan, Sql);

def encode_rejection = fn(value: Rejection) {
    codec.encode(Rejection, value)
};

export { Rejection, encode_rejection };
```

下游调用 `encode_rejection(value)`，不重复 family 实参；函数契约仍严格检查 value 与
witness 对应。该模块/API 方案适用于跨模块和包含封闭递归参数的 family，不引入隐式
反射或新的表面语法。

### 10.2 Fuel 和配额

Fuel 在函数调用及实际执行的 control-flow back edge 等动态扩展点扣减。直线指令和
未采取的 back edge 不消耗同类进度 fuel。Fuel 是确定的终止边界，不代表 CPU 时间。

独立配额限制：

- 逻辑分配量；
- VM stack slot；
- 调用深度；
- 输出和递归格式化深度；
- module 与整个 session 的工作量。

Native function 和 Telora callback 共享调用 session 的 account，不能通过跨 native
边界绕过预算。配额耗尽产生带来源的结构化失败。

### 10.3 确定性

在相同源码、解析依赖、显式 Host 输入、native module 实现和预算下，执行结果应可
重放。无领域顺序的 Dict、模块身份、诊断、类型合并和输出使用稳定顺序。

确定性不意味着所有程序终止，也不保证不同编译器版本产生字节级相同的内部表示。
它要求当前语义版本内，内部 cache 命中、heap 地址、物理路径别名和无意义遍历顺序
不能改变可观察结果。

## 11. Host 边界

Host 负责所有开放世界行为：文件和 package 解析、环境捕获、外部输入、权限、时钟、
持久化、重试、事务以及真实效果。典型调用顺序为：

```text
Host 选择 Main 和 Entry
  -> 固定依赖与 source snapshot
  -> 在准备 WorkWorld 调用 Entry.prepare，取得环境诉求
  -> 注册显式输入，初始化 Main，并按 Entry.MainType 校验完整 export record
  -> 冻结 MainWorld
  -> 在新的运行 WorkWorld 调用 Entry.initialize(main)
  -> 以 SystemEvent 驱动纯 reducer，解释返回的 SystemEffect
  -> 原子发布成功结果，或丢弃失败过程的候选结果
```

Host virtual input 属于一次调用，在 Main 执行前冻结。Main 不能反向 import Host entry
runtime，也不能请求 Host 在执行途中打开新的观察窗口。

Plan 没有语言级权限。一个值即使静态类型为应用定义的 `ExecPlan`，也只是一段数据；
只有相应 Host adapter 可以解释它。

### 11.1 当前 CLI Host

当前 `telora` 二进制提供：

```text
telora check <module> [-C <context>]
telora run <binary-name> [-C <context>] [--input <json|->] [--entry <file>] [--best-effort]
telora run -S <file> [--input <json|->] [--entry <file>] [--best-effort]
telora show <module> [-C <context>] [-p <substring>] [-k type,let,def,import] [--exports]
telora show <module> [-C <context>] --at <line>[:<column>]
telora lsp
```

`run abc` 的 binary name 是一个不含路径分隔符和 `.telora` 后缀的 stem；Host 从 CWD
向上发现最近的 manifest，并固定选择 `@bin/abc.telora`。调用者写 `run abc`，不写
`run @bin/abc.telora`。`run`、`check` 和 `show` 的 `-C` 都指定 manifest discovery
的起始目录，该目录不必就是 crate root。`run -S file` 是独立 standalone 模式：
即使文件的祖先目录存在 manifest 也不
查找，只使用根文件内的 `crate.dependency` / `crate.format` options，且 options 相对
文件所在目录解析。只有 standalone 根文件可以声明这些 resolver options。`-S` 与
binary name、`-C` 互斥。

其他命令的 `<module>` 是 `@src/...`、`@bin/...`、`@test/...` 或依赖模块 ID，不是物理
文件名。`show` 以稳定 `telora.show/v1` JSONL 输出语义事实，默认查询选中模块的顶层
local definitions。`-p` 执行大小写敏感的字面子串匹配，不解释 glob 或正则表达式；
`-k` 接受由逗号分隔的 `type`、`let`、`def`、`import`。`--exports` 改查公共接口并与
`-k` 互斥。`--at` 接受从一开始计数的 `line[:column]`：只有行号时选择与该行相交的
事实，带列号时选择覆盖该点的事实；它与 `-p`、`-k`、`--exports` 互斥。空匹配成功
且不输出记录。每条记录显式区分 `authoritative`、`recovery` 或 `debug` 权威层级；
表达式级记录属于 `debug`，错误恢复所得记录的权威层级服从其事实和模块状态。
Namespace import 的 definition record 以 `target` 给出被导入模块的稳定 ID，并省略
普通值的 `type` 字段；其成员的精确公开 type/scheme 由该目标模块的 `--exports`
记录定义。Selective import 仍在本地 definition record 中直接携带所选成员的精确
type/scheme。Namespace 不把模块接口压缩为含 `Any` 的近似 Struct 类型。

`check` 用 best-effort 模式求完整模块并以严格 finalization 决定退出状态。独立计算可以
在失败后继续，以收集更多诊断；但任何语法、类型、解析或运行时 error 都会令整轮导出
失去意义，命令非零退出且不产生 Module value，即使某个干净的最终根仍可算出。只有没有
error 且严格加载也能成功时才可交付。`check` 的 stdout 完全采用 `telora.check/v1`
JSONL：先按稳定顺序输出
零到多条 `diagnostic` record，最后恰好一条 `summary` record；summary 包含稳定 module
ID、dependency 数量和 `ok` 或 `error` status。Warning 本身不阻止成功；失败不伪造
Module value，并以非零退出。普通 stderr 只用于 CLI/Host 故障，`dbg!` 仍是独立旁路。

`show` 不执行上述 finalization。它查询由 recoverable CST、部分语义分析和诊断求值形成
的全面证据图，因此模块不完整或求值失败时仍可返回不受影响的事实。`show` 成功只表示查询
成功，不表示模块健康；恢复节点不得以权威 `Any` 伪装成已知值。

`run --best-effort` 在启动 Entry 前对 Main 执行静默的 best-effort 诊断求值，并把
`telora.run/v1` diagnostic records 写入 stderr。它只用于遇到问题时扩大诊断覆盖：只要
本轮出现任何 error，恢复得到的 Main 就整体废弃，命令输出 error summary、非零退出，且
不初始化 Entry、不解释 SystemEffect；一个不依赖失败的干净根值也不例外。没有 error 时，
命令重新进入严格 Entry reducer 与 Host effect lifecycle，不进行 speculative recovery。
最终验收必须使用省略该参数、保持 fail-fast 的普通 `run`。

`run` 选择一个 Main application 和一个 Edge Entry。省略 `--entry` 时使用内置 Entry：
它只在提供 `--input` 时请求把外部 JSON 安装为 Main 的 `input` binding，并从 Main 的
完整显式 export record 中选择 String `output`。`--entry file.telora` 是 Host 的显式授权动作；
该物理源码获得保留的 Entry 身份，可以访问依赖图内任意 `.priv.*`、
`.native.telora` 和已注册 native module。特权仅属于这个 requester，不传递给它导入
的普通模块。Entry 不能使用相对、`@src`、`@bin` 或 `@test` import；依赖必须使用
manifest 中的稳定 module ID。

Entry 在纯 Telora 中实现以下 ABI：

```telora
import "std/rt.priv.telora" as rt;

export type MainType = ...;
export type State = ...;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = ...;
export def initialize:
    Fn(MainType) -> Tuple([
        State,
        Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]),
    ])
    = ...;
```

准备 WorkWorld 只能返回经 Host 校验的 `SystemCaps`。Host 随后准备并初始化 Main，按
`MainType` 校验完整 export record，再冻结 MainWorld。运行阶段使用一系列 WorkWorld；
`MainType` 没有系统约定的形状，它完全由所选 Entry 定义。内置 Entry 使用 `Dyn`
边界只是该 Entry 自己的适配策略。
`State` 对 Host 不透明，也不会物化为 Host-owned Value。每轮结束时，runtime 只把
`SystemEffect` 导出给 Host；它从下一 State root 开始 trace，保留 MainWorld edge，借助
同一个 forwarding table 把可达 Work object 直接复制到新的 WorkWorld，再释放旧
WorkWorld。共享、循环、身份和 provenance 在迁移后保持，reducer 临时垃圾不迁移。
当前每轮均执行一次这种迁移；未来可以在确定性阈值内复用 WorkWorld，再用相同机制做
周期性 copying GC，但这不是当前语义。

Entry reducer 接受单个
`SystemEvent`，返回下一 State 和 `SystemEffect` 数组。Effect 没有同步返回值；新的
外部信息只能在后续 turn 作为 Event 注入。当前固定协议为：

```text
Stdin  = Piped | Inherit | Null
Stdout = PipedLine | PipedToEnd | Inherit | Null
Stdio  = { stdin: Stdin, stdout: Stdout, stderr: Stdout }

ChildOpts = {
    bin: String,
    cwd: Option(String),
    envs: Dict(Option(String)),
    clear_env: Bool,
}
SpawnStdioChild = { key: String, opts: ChildOpts, stdio: Stdio }
ChildText       = { key: String, data: Option(String) }
ChildSpawnResult = { key: String, result: Result(Int, String) }
ChildExited     = { key: String, exited: Result(Int, Option(Int)) }

SystemEvent = Initialize
            | ChildStdout(ChildText)
            | ChildStderr(ChildText)
            | ChildSpawnResult(ChildSpawnResult)
            | ChildExited(ChildExited)

SystemEffect = SpawnStdioChild(SpawnStdioChild)
             | PostStdin(ChildText)
             | Exec(ChildOpts)
             | Output(String)
             | Exit(Int)
```

`SpawnStdioChild` 使用 Entry 给出的稳定 `key` 启动进程，并始终产生对应的
`ChildSpawnResult`：成功分支携带 pid，失败分支携带可由 reducer 处理的错误文本。stdin 可为
`Piped | Inherit | Null`；stdout/stderr 可为
`PipedLine | PipedToEnd | Inherit | Null`。`PostStdin` 的 `Some(text)` 写入 UTF-8
文本，`None` 关闭管道。PipedLine 逐行产生不含行终止符的 `Some`，PipedToEnd 在 EOF
后至多产生一个完整 `Some`，二者均再以 `None` 明确表示 EOF；`ChildExited` 在管道
EOF 后产生。

Host 在单个异步事件循环中并发执行 effect。每个 child 的监督、stdin 写入、stdout
读取和 stderr 读取都是独立调度的任务；某个管道发生背压或等待数据时，不得阻塞其他
effect 或 event。这里保证并发而不保证并行。reducer 调用始终串行：Host 每次只注入一个
已排队的 `SystemEvent`，不会并发调用 reducer。

每个成功 Spawn 的 child 都必须由 Host 回收。正常完成时，Host 取得并回收退出状态后
才发送 `ChildExited`；Entry 发出 `Exit` 或 `Exec`、reducer 失败、协议失败或 Host
管道处理失败时，Host 必须终止并 wait 所有仍活动的 child。Entry 不承担防止 zombie
process 的责任。具体地，`Exit(code)` 的 terminal barrier 顺序是：停止剩余 child，
wait/reap 所有 child，提交已缓冲 Output，最后才允许 CLI 调用
`std::process::exit(code)`；任一 wait 失败都会阻止该 exit code 生效。
Host 必须以结构化任务集合持有全部 effect 与 child supervisor；supervisor 同样持有其
stdin/stdout/stderr 任务。terminal、reducer 失败、协议失败或 Host 失败时，Host 先发出
取消信号并关闭输入邮箱，再 join 全部任务；这些任务不得脱离所有权树继续运行。

`Output(String)` 是 Entry reducer 的输出效果，不是 Main 返回类型，也不要求 Host
编码 Telora 值。Entry 可以用自己的 `MainType`、codec 和 formatter 生成任意多个
String chunk。CLI 在 terminal effect 前缓冲它们；协议失败不暴露部分输出。
`Exit(Int)` 和 `Exec(ChildOpts)` 是 terminal effect，必须位于 effects 尾部；Exec 在
支持进程替换的 Host 上替换 Telora 进程。没有内部 Wake 或任意 turn 上限；无队列
事件且无活动 child 时才判定无进展。每次 reducer 调用仍受普通 VM quota 约束。

`check`、`show` 和 `lsp` 当前仍是 Host 固定命令路径，尚未通过 run Entry ABI。它们
把目标当作 module。`check` 给出严格 module load/compile verdict，但不等价于一次
`run`：它不选择 application output，也不承诺执行期成功。`show` 和 LSP 可以使用
recovery snapshot 展示仍有证据的语义事实。

CLI 不提供领域专用的 `exec` 或 `build` adapter。Exec plan、build plan、SQL plan 等
都是普通应用值；是否解释它们以及是否产生现实效果，由显式的外部 Host 决定。

## 12. Workspace 和语言工具

严格检查与编辑器分析共享 parser、HIR、类型图、工具阶段计算和 semantic snapshot，
但发布策略不同：严格模式要求完整证据，workspace recovery 保留错误周围仍可证明的
事实。

语义事实不仅是 `known/unknown` 二值。当前状态包括：

```text
Known
Unknown(MissingSyntax | InvalidSyntax | UnresolvedName |
        BlockedBy | UnavailableDependency)
Conflicted(DuplicateDefinition | IncompatibleContract)
Incomputable(QuotaExceeded | RuntimeOnly | UnsupportedOperation |
             CyclicEvaluation | Cancelled)
```

显式 `Any` 是一个已知类型，不等于 Unknown。

Workspace 使用 copy-on-write document snapshot 和单调 revision。每次 rebuild 绑定到
一个 revision 和 cancellation token；被取消或因新编辑而过期的工作不得覆盖较新的
published snapshot。Snapshot 的发布是原子的。

当前 LSP 实现支持增量文档同步、diagnostic、hover、definition、references 和
completion，并协商 UTF-8/UTF-16 等位置编码。Completion 只使用恢复后确有证据的模块
export 和 Struct field，不在 unknown/Any 上虚构结构。

CLI `show` 和 LSP 可以展示 recovery fact；`check`、`run` 和 Host entry 仍采用严格
成功边界。两者共同展示的事实必须具有同一含义。

## 13. 标准库和 native 边界

标准库由普通 Telora module 与受信 native module 共同组成。普通模块实现 Option、
Result、argv 等组合政策；native 模块提供需要高效 heap 观察或受控 runtime identity
的确定操作。

当前通用能力包括 Array/Dict 组合、String、lexical path、SHA-256、regex、JSON codec
与 schema、TypeMetadata attribute、Dyn observer、文本 parse/display 等。
这些 API 不授予环境或文件系统访问权限。例如 path 操作是词法操作，hash 操作只
处理显式输入。

Native 声明的可用性由 Host 注册表和精确 module identity 决定。未知 native module
不可用；用户不能仅通过书写 `.native.telora` 获得 native implementation。

语言核心、标准库、领域库和应用应保持以下依赖方向：

```text
language/VM
  -> generic standard library
  -> domain or method library
  -> application model and authored intent
```

放置新行为时依次询问：

1. 它能否只是普通 application function？
2. 它是否属于可复用的 domain/method library policy？
3. 它是否是通用、确定的 standard-library operation？
4. 只有前三层都无法忠实表达时，才讨论缺少哪项最小语言或 Host 机制。

Ontology、analytics、build、deployment 或 Agent workflow 目前都不是语言内建概念。

## 14. 当前边界和非保证

当前语言设计不包含：

- ambient IO、文件访问、网络、时钟或环境读取；
- 通用 effect handler 或语言级 action protocol；
- runtime code generation、`eval`、动态 import 或通用 macro system；
- trait、interface、subtyping、associated type、higher-rank/HKT；
- 任意 binding 的无限制 polymorphic generalization；
- 全局 termination proof；
- 通用 package registry、获取和版本求解；
- 生产级领域 effect executor；
- 对外部生态或未来版本的长期兼容性保证。

此外，下列能力具有明确的当前限制：

- `interpreter!` 只提升直接 A 参数的消费型解释器，不能适配高阶或返回 A 的位置；
- Func 在公开 Type descriptor 解释中是受限的 opaque leaf；
- 普通 CLI 严格失败输出不保证一次展示 recovery 已收集的全部独立诊断；
- Host-observed diagnostic 具有 severity/message/location/labels，但还没有稳定的领域
  category、cause graph 或 repair schema；
- 工具可以保守丢失精度，不能以猜测填补缺失语法或失败依赖。

这些限制是当前规范的一部分。应用不能依赖规范之外的推断、反射或 Host fallback。

### 14.1 复合值的保守类型推断

第 6 节规定的完整调用证据、expected function type、callback widening 和 enum
payload 精化均属于当前语义。但推断不保证为任意一组分别构造的窄值主动寻找一个
公共的高层 family 实例。

特别是，当同一个 Array 没有显式 item expected type，而元素同时包含不同 singleton
Atom、不同 closure、不同 `Option` variant 或匿名 Struct 时，仅凭元素字面量可能
无法主动找到预期的封闭 enum、函数契约或参数化 family。严格模式会报告冲突或未
解决约束，不会把元素静默擦除为 `Any`。错误中出现较大的 variant union，通常表示
缺少共同的 expected type，而不表示运行时存在动态 union。

在最小公共边界给 Array 提供具体契约即可把同一个 family 实例下传到匿名元素：

```telora
@struct type Entry(Id, Value) = {
    id: Id,
    value: Option(Value),
};

@enum type EntryId = {First: 'None, Second: 'None};
type IntEntry = Entry(EntryId, Int);

let entries: Array(IntEntry) = [
    {id: 'First, value: 'Some(1)},
    {id: 'Second, value: 'None},
];
```

该标注提供检查目标，不改变值的运行时表示，也不授权 `Any` fallback。若记录需要在
数组之外分别构造，可以改为给完整记录或具名构建函数标注 `IntEntry`。若完整泛型
调用仍有歧义，可以进一步使用 `@[...]` 显式提供无法由值参数唯一确定的类型实参。

### 14.2 Enum payload 的具名类型要求

`@enum` decorator 当前要求每个有 payload 的 variant 引用可解析的具名
TypeMetadata。不能在 enum 类型声明的 payload 位置直接放置匿名 Struct：

```telora
# 非法：payload 是匿名 Struct TypeMetadata
# @enum type Expr = {Column: {alias: String, column: String}};

@struct type ColumnRef = {alias: String, column: String};
@enum type Expr = {Column: ColumnRef};
```

该限制只属于类型声明。构造 tagged value 时，payload 的匿名记录仍会按具名 Struct
契约检查，因此下式合法：

```telora
let expr: Expr = 'Column({alias: "orders", column: "id"});
```

### 14.3 Family 与递归类型

递归 concrete TypeMetadata 在普通 definition contract、参数化 family contract 和
模块接口中保持具名 identity 与有限图回边。契约检查按递归图比较类型，并以已经
访问的 reference pair 终止；不会把回边展开为无限树，也不会将递归位置擦除为
`Any`。因此递归表达式类型可以直接进入函数和 family：

```telora
@enum type Expr = {Literal: Value, Call: CallExpr};
@struct type CallExpr = {name: String, args: Array(Expr)};

@struct type Dialect(Context) = {
    render: Fn(Context, Expr) -> String,
};
```

同一递归结构经 whole-module、selective 或 open import 得到的契约保持结构等价；
模块内部用于避免名称碰撞的 identity 不属于显示名称，也不是源码可引用的类型名。

Family 自身仍不能参数化递归或形成循环 family component，也不能调用同模块普通
helper。这里的限制是 family 求值依赖的限制，不限制 family 字段引用已经封闭的
递归 concrete type。

### 14.4 多态 binding 与外围类型参数

Telora 不提供任意 binding 的无限制 let-polymorphism。一个泛型 callable 经普通
alias 绑定后，只在该 alias 初始化时实例化一次；需要在多个类型上使用时，应直接
调用原泛型定义、在调用点使用 `@[...]`，或声明具有完整 `for` 契约的具名定义。

此外，泛型函数 body 内的局部 annotation 当前不能引用由外围模块级 `for` 契约
引入、但未在该局部定义契约中重新量化的类型参数。需要这种 annotation 时，应把
相关计算提升为模块级辅助定义，并在其完整 `for(...) Fn(...) -> ...` 契约中显式
列出所需参数。不得通过 `Any` 擦除该依赖。

## 15. 规范性总结

一个符合当前 Telora 模型的程序，应能被理解为以下组合：

1. 静态解析的 source/module graph；
2. 对不可变小型值模型的普通函数计算；
3. 由同一 VM 求值并由类型检查器解释的 TypeMetadata；
4. 明确受限的 `Dyn`、诊断和 Host bridge；
5. 由资源 account 包围、成功后原子发布的一次执行；
6. 最终仍需 Host 赋予意义的普通输出值。

Telora 的核心承诺不是“所有错误都能静态发现”，也不是“所有程序都会终止”。它的
承诺是：可影响一次执行的世界是明确的；执行在有限边界内产生值或结构化失败；类型、
来源和诊断尽量共享一个权威语义模型；任何现实效果都位于普通程序之外的 Host 授权
边界。
