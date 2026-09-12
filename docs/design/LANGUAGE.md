# Telora 语言设计

本文档是 Telora 当前语言设计的单一事实来源。它从语言使用者、库作者、工具和
Host 能够观察到的语义出发，定义语言表面、静态语义、求值模型、模块边界、诊断、
资源限制和 Host 协议边界。

核心概念与术语见 [`CONCEPT.md`](CONCEPT.md)，问题与动机见
[`../MOTIVATION.md`](../MOTIVATION.md)，文档的权威关系与维护规则见
[`../README.md`](../README.md)，当前实现架构与源码证据地图见
[`IMPLEMENTATION.md`](IMPLEMENTATION.md)。具体写法与编程指导属于
[`../../guide/TELORA.md`](../../guide/TELORA.md)。

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

这里的“纯”需要精确定义：普通值计算没有外部效果。`warn!`、`ok_or_warn!`、
`raise!`、`unwrap!` 和 `fail!` 可以产生 Host 可见诊断，因此属于 Telora 求值的受控可观察
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
BuildState.Ready           # BuildState 的无 payload variant
```

具名 enum 保留自己的 nominal TypeId。variant 的名称和 payload 由 enum 声明定义。

普通字符串支持 `\0`、`\n`、`\r`、`\t`、`\"`、`\\`、两位 ASCII `\xNN`、
Unicode scalar `\u{...}` 和反斜杠换行后的显式续行。反引号字符串使用同一组
标量转义，但以 `` \` `` 代替 `\"`，并作为结构化连接表达式使用 `\{...}` 嵌入
表达式。raw String 不处理转义或插值；需要保留大量反斜杠或引号时应增加 `#`
delimiter，而不是发明新的 escape。

```telora
let greeting = `hello \{name}`;
```

插值中的每个表达式都要求 `T: std/fmt.Display`，并在编译期降低为已选中
dictionary 的 `display` 成员调用。String、Int、Float 由标准能力提供
显式实现；插值处需要已确定的类型及其 Display 实现，`Dyn` 必须先显式投影。具名 enum
不会因运行时使用 Atom 表示而自动获得 `Display`。String 保持原文本，Int 使用
十进制表示。Float
使用有限 binary64 的文本表示：与 Rust `f64` 的 `{}` 一致，选择能往返到同一
binary64 值的最短十进制文本，不受 locale 影响。该表示保留负零的符号，但不保留
整数值的小数点，例如 `3.0` 表示为 `3`，`-0.0` 表示为 `-0`。输出不保留字面量的
原始小数或指数拼写。

`std/fmt` 定义标准静态展示能力：

```telora
import "std/fmt" as fmt;

@fmt.display_by("{host}:{port}")
type Endpoint = struct { host: String, port: Int };

def endpoint_text: Fn(Endpoint) -> String = fn(endpoint) {
    `endpoint=\{endpoint}`
};
```

`display_by` 是受控模板 eDSL。它发布 `DisplayBy` typed property；`std/fmt` 中的
property-constrained blanket impl 由此为 `Endpoint` 提供 `Display` evidence。
`DisplayBy` 保存普通 `DisplayTemplate` 数据和普通 `Fn(Dyn) -> Fmt` closure；模板数据
由常量 String 与字段名 Array 组成。property interpreter
在发布阶段解析模板、验证字段、把字段名解析为 canonical index，并捕获字段的 Display
closure。运行期路径只按固定 index 投影并调用已捕获 closure。codec 的 text bridge 通过
VM continuation 调用同一个 closure，共用当前 quota account；失败的公开 rule 保持调用者
位置，data-src 保持 closure 输入的数据来源。它直接支持
String、Int、Float 以及嵌套的 `DisplayBy` struct；它不动态调用字段类型上的任意
显式 `Display` impl。
`Display.display` 返回 opaque `Fmt`，`fmt.display` 也返回 `Fmt`；显式调用使用
`fmt.render(fmt.Display.display(endpoint))` 得到 String。`fmt.concat(strings, items)`
按 `strings.len == items.len + 1` 组合常量文本和 `Fmt` fragment。插值使用同一
`Display -> Fmt` 路径，并只在整个字符串末端物化一次。Fmt 节点及其 Host payload
在复制前预扣 allocation quota；物化前按共享节点 memoize 测量最终 UTF-8 字节数并
预扣配额。共享 fragment 的长度只计算一次，但每次出现在结果中都计入最终长度，
因此配额拒绝不会先展开指数大小的树。`dbg!` 的有界 repr 属于
Host-only 观察；codec/JSON 则是数据交换协议，三者
都不能作为彼此的隐式替代。

Bool 是闭合的内建 enum，其成员 `True` 和 `False` 由 prelude 提供。
这些名称直接确定 Bool 类型，条件、guard 和布尔操作符接受 Bool 值。
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

无 expected item type 的 Array 字面量必须得到共同元素类型。`[1, "one"]` 会报错；
位置不同的异质数据可用 Tuple，领域中的不同形态应显式包装为 enum。`Never` 不贡献
可达元素类型。
同一 enum 类型族的成员合并泛型参数证据，例如 `[Ok("hi"), Err(2)]` 的类型为
`Array(Result(String, Int))`。不同类型族或冲突的具体参数会报错；缺少证据的参数
可用 `.ty!(Ty)`、`func@[Ty](...)` 或类型注解补全。
显式 `Array(T)` expected type 会向每个普通元素和 spread operand 下传 `T`。因此当
`T` 是 concrete family 实例时，多个匿名记录元素中的 variant 构造、闭包、
Option variant 和空集合都按完整的共同契约检查。
该检查与元素顺序无关；真正不兼容的字段在对应元素位置报告类型冲突。
在 `if`、`if let` 或 `match` 的结构化分支结果中，同一 Array 或 Dict 元素位置的具体
分支为无元素分支提供类型证据；该合并与分支顺序无关。若所有可达分支均无元素且
没有 expected item type，必须用类型注解提供元素类型。

Tuple 是固定长度的异质积：

```telora
let entry: (String, Int) = ("port", 8080);
```

类型位置的 `(A, B)` 构造 Tuple 类型；数据位置的 `(a, b)` 构造 Tuple 数据。
`(A,)` 是单元素 Tuple 类型，`(A)` 只是分组。旧的 `Tuple([A, B])` 保留为
静态类型构造写法，参数必须是语法上明确的类型列表，不能由普通函数计算。

空 Tuple 值写作 `()`，其类型也可在显式类型槽位写作 `()`。Prelude 中的 `Unit`
是该结构类型的别名，与 `Tuple([])` 身份相同，不创建新的 nominal 类型：

```telora
type Empty = ();
let value: () = ();
let alias: Unit = value;
```

该类型记法适用于 annotation 根、type initializer 根、声明的 member 类型、显式
类型实参、`ty!` 目标和受限契约（含嵌套参数）。`Array(())` 表示
空 Tuple 的 Array 类型，`Array(()).type` 才是其元数据。普通函数的实参仍是数据。
`(Int, String).type` 是 Tuple 类型的元数据，`(Int.type, String.type)` 是元数据
组成的 Tuple 数据。混合类型和数据的 `(Int, 1)` 非法。类型 Tuple 的 spread 暂不支持。

Tuple 字面量的 spread 按位置拼接静态已知的 Tuple：
`(...pair, True, 3)` 展开 pair 后追加 Bool 和 Int 元素。每个位置
保持独立类型，多个 spread 按书写顺序展开一层；空 Tuple 不贡献元素，普通
tuple-valued 元素保持嵌套。Array、Dict、Dyn 和未知形状的类型变量不提供
Tuple spread 所需的固定长度证据。

`(...pair)` 和 `(...pair,)` 都是 Tuple 构造；普通 `(value)` 是分组，
`(value,)` 是单元素 Tuple。目标 Tuple 契约按展开后的下标传递元素上下文，
包括 spread 中直接书写的 tuple 字面量，最终完整长度和逐位置类型均须匹配。
非字面量 spread 操作数须自行提供静态 Tuple 形状，外层目标不猜测其长度。
操作数从左到右各求值一次，空 spread 仍求值；元素 Val 的来源和名义身份
被复制保留，新容器采用整个 Tuple 构造表达式的位置。

Record 字面量与 `Dict(T)` 在运行时都使用 String key 的 Dict 表示，但静态意义不同：

```telora
let user = {name: "Ada", active: True};
let labels: Dict(String) = {region: "east", tier: "gold"};
```

Record 的字段集合属于静态结构；`Dict(T)` 的 key 集合可以动态变化，所有 value 具有
同一静态类型。Dict 的无领域顺序观察和序列化采用规范顺序。

字段投影保留静态证据：已知 Record/Struct 必须声明该字段，`Dict(T)` 的字段结果是
T。非记录值和 `Dyn` 会产生静态诊断。
暂未确定的 receiver 可以在同一个单态推断边界内积累字段 obligation，后续
具体证据必须满足全部字段；这种 obligation 不会泛化或发布为开放 row constraint。
若推断边界结束时仍无具体证据，程序必须提供参数或函数契约。Dict key 是否实际存在
仍在求值时检查，缺失 key 是可恢复的程序失败。

Array 和 Dict 支持 spread：

```telora
[0, ...items, 9]
{...defaults, mode: "strict", ...overrides}
```

Dict spread 的操作数具有 `Dict(T)` 类型。字段和 spread 按源码顺序合并，后出现的
同名字段值覆盖先出现的值；同一字面量中重复声明的显式字段是错误。

`std/dict.keys`、`values`、`pairs` 和 `merge` 使用 `Dict(A)` 泛型契约；values 与
pairs 保留元素类型 A，merge 的两个输入共享 A，同名键由右侧覆盖。

#### Struct 合并更新

`base <~ patch` 构造与 base 具有相同具名 Struct 类型的新值，包括其全部泛型实参。
左操作数提供名义身份和完整字段类型，右操作数提供更新字段。右侧可以是另一个
具名 Struct 值，也可以是由左侧约束的更新字面量：

```telora
type State = struct {count: Int, label: String};
type Label = struct {label: String};
def update = fn(base: State, label: Label) {
    base <~ label <~ {count: 2, ...label}
};
```

右侧所有字段名必须属于左侧类型；未提供的字段保留原值，提供的字段按完整值
替换。右侧具名类型的身份仅用于确定其字段契约，结果身份始终来自左侧。
更新字面量是 `<~` 的上下文语法，其自身无需对应一个独立具名类型。

更新字面量中的 spread 展开具名 Struct 的静态字段集合。每个字段按源码顺序
覆盖，最终生效的显式字段表达式获得目标字段的 expected type，最终字段值须
满足对应类型。已存在的值保持自己的类型身份。被覆盖的表达式独立通过类型
检查，且仍须求值；所有字段名，包括被覆盖的字段名，都须属于目标类型。
显式字段名在同一字面量内唯一，包括跨 spread 的显式字段。

`<~` 左结合，每一步更新独立检查。`base <~ patch <~ {count: 2}` 中 patch 的字段
类型必须先符合 base 的契约。空更新 `{}` 合法。更新先求值 base，再按源码顺序
各求值一次右侧表达式，产生新的外层容器，保留被复制字段的 Val、嵌套身份和
来源位置；新容器的位置来自更新表达式。

Struct spread 的使用位置是更新字面量；普通 Dict spread 按 `Dict(T)` 契约处理。
更新要求已知的具名 Struct 字段集合，Dict、Dyn 和无字段证据的泛型参数不构成
更新操作数。外部结果类型注解不替代左操作数自身的具名类型证据。

#### 字段投影

`source.{x, y as Y}` 是 dot postfix 表达式，选择源字段 x 和 y，分别使用目标名
x 和 Y。源值须是静态字段集合已知的具名 Struct；源字段须存在，目标名须唯一。
允许空投影、尾逗号，以及把同一源字段选择为不同目标名。

普通表达式位置的投影要求具名 Struct 目标上下文，例如
`let selected: Foo = source.{x, y as Y};`。目标字段集合须与投影结果完全一致，
字段类型须兼容。参数、返回值契约和相等比较的具名另一侧同样提供目标上下文；已有字段值不被重新
解释为另一种名义类型。无目标上下文时产生静态诊断，字段形状不选择名义身份。

作为 `<~` 的直接右操作数时，投影是更新字段集合：
`base <~ source.{x, y as Y}` 检查目标名是 base 字段的子集，并保持 base 的类型。
投影无需独立具名身份。此上下文与完整构造共享字段选择和重命名规则。

接收者在读取任何字段之前只求值一次。复制字段保留 Val 来源和嵌套身份；
新投影容器的位置来自投影表达式，普通构造附加目标类型身份，更新字段容器
由外层更新消费。空投影仍求值接收者，随后构造空目标或提供空更新。

### 3.2 Sum 值

具名成员提供 enum 的无 payload 值或带 payload 的构造函数：

```telora
let absent: Option(Int) = None;
let present: Option(Int) = Some(1);
let success: Result(Int, String) = Ok(1);
let failure: Result(Int, String) = Err("reason");
```

Option、Result、Bool 以及用户 enum 都建立在 Atom/Tagged 表示上。类型元数据决定
合法 tag、payload 形状及静态穷尽性。

名称确定 enum 类型族，类型上下文和调用证据补全泛型参数。成员使用
`Event.Progress` 或模块限定的 `events.Event.Progress`；声明本身只绑定类型名。
`import Event.{Progress, Finished as Done};` 选择成员并引入本地名称；
`export Event.{Progress, Finished as Done};` 同时提供本地绑定与公开导出。
prelude 提供 Bool、Option 和 Result 的成员。payload 构造器可作为函数传递，
显式泛型特化使用 `Result.Ok@[Int, String]`，包含类型族的全部参数。
同一类型族的分支、返回值和集合元素合并参数证据；仍未确定的参数需要显式契约。
模块接口和 Dyn witness 使用完整类型。

### 3.3 相等性和顺序

普通标量和结构值支持 `==` 和 `!=`，其签名均要求两个操作数具有同一静态语义类型
`T`；已知类型或形状不兼容时在前端报错，不把类型错误解释为 False。若一侧具有
exact nominal identity，另一侧的 dict 字面量获得该具名类型上下文；具名 variant
构造获得同一类型族的参数和 payload 上下文，与操作数顺序无关。上下文沿 Array 元素、Tuple 对应位置、
dict 字段和值、同 variant 的 payload 递归传播，例如 `[item] == [{value: 42}]`
中的字段字面量可从 `item` 获得名义类型；不同位置的上下文可分别来自两个操作数。
构造上下文沿整图槽位传播，包括经局部变量和函数返回值连接的未定构造。
无标注的声明不会提前冻结一个匿名类型；已确定的具名类型身份不能被更换。
函数调用通过共享的泛型类型变量传播同样的构造上下文。例如 `for(T) Fn(T, T)`
和 `for(T) Fn(Array(T), T)` 均可从一个参数获得名义类型证据，为另一个参数中的
字面量提供上下文；回调的返回值也可提供该证据。不同泛型变量不会因为解析后的
类型相同而共享上下文。`std/eq.equal` 遵循这套通用规则，不依赖函数名称或导入别名。
复合值按结构比较，但两个具名值还必须
具有相同的 canonical TypeId；函数按不透明函数身份比较，而不是比较代码或闭包捕获
内容。enum 契约允许同一静态类型在运行时携带不同 variant，
此时不同 meta 或不同名义 identity 返回 False。只有一侧携带 exact nominal witness
时也返回 False，不会按 raw payload 猜测字面量来源。`!=` 是 `==` 的精确布尔补集。

`<`、`>`、`<=` 和 `>=` 只接受类型相同的 `Int`、`Float` 或 `String` 操作数；数值
之间没有隐式转换。Int 使用有符号数值顺序。Float 只包含有限 binary64 值，使用
通常的有限数值顺序，正负零相等。因此 Float 相等是自反的，不存在 unordered 比较。

String 顺序是其内部 UTF-8 字节序列的字典序：第一个不同字节较小者在前；若一方是
另一方的前缀，较短者在前。不执行 Unicode normalization、locale collation、
case folding 或自然数排序。

所有六种比较运算符处于同一非结合优先级；连续比较必须用括号明确分组。比较运算
使用固定的内建语义，不执行 trait implementation selection。

## 4. 表达式和控制流

Telora 是 expression-oriented 的。Block 中的最后一个表达式是 block 的值：

```telora
let result = {
    let adjusted = value + 1;
    adjusted * 2
};
```

普通 block 允许以 `;` 结尾的表达式语句，按顺序各求值一次并丢弃结果。没有尾表达式
时，正常到达末尾返回 `()`；该规则同样适用于函数体和分支中的 block：

```telora
do {}                       # Unit
do { let a = 42; }           # Unit
do { 42; }                  # Unit
do { 1; 42 }                # Int，结果为 42
```

丢弃结果不免除类型检查、失败传播或执行配额。`return`、`fail!`、`panic!` 等
`Never` 路径不会因尾分号变成 Unit；只有正常 fallthrough 才产生隐式空 Tuple。
`return expression;` 保留原有语法，`if` 仍须有 `else`。顶层模块不接受表达式语句；
有歧义的裸 `{}` 仍表示字典，`do {}` 明确表示 block。

当前表面包括算术、比较、短路布尔运算、field selection、调用、pipeline、条件、
模式匹配、传播和显式返回。主要运算符包括：

```text
!x  -x
*  /  %  +  -
&  ^  |
<~
<  >  <=  >=  ==  !=
&&  ||
|>
```

`!` 对 Bool 返回相反的 canonical Bool，对 Int 执行按位取反；操作数只求值一次。
`&`、`|` 和 `^` 对两个 Int 分别执行按位与、按位或和按位异或。`<~` 对具名 Struct
执行合并更新，规则见 3.1 节。`<~` 左结合，优先级低于位运算、高于比较。Int 位运算
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

Tuple 使用非负十进制字面量做位置投影，例如 `(left, right).0`。位置投影是可连续
组合的后缀操作；`value.1.0` 等价于 `(value.1).0`，也可继续接 field selection、
index 或调用。Tuple 的位置在分析期检查并得到精确成员类型。

`if` 必须有 `else`，两个分支产生可合并的类型：

```telora
if enabled { "on" } else { "off" }
```

`ctrl_block` 是普通 block、`if`、`if let`、`match` 或 `return expression;`。`if` 和 `if let` 的
`else` 接受一个 `ctrl_block`；非普通 block 的形式规范化为只含该控制流表达式的
block。因此 `else if` 可以连续使用：

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

提前返回同样可以直接作为 `else` 分支：

```telora
if ready { value } else return fallback;
```

`return expression;` 从最近的函数返回。它不是模块导出机制。

### 4.1 模式匹配

Pattern 可以匹配字面量、enum variant 及其 payload、Tuple 和 Struct 字段：

```telora
match result {
    Ok(value) => value,
    Err(message) => fail!(message, result),
}
```

Match arm 可以带 Bool guard。只有无 guard 且覆盖确定的 arm 才参与穷尽性和冗余性
证明。对闭合 enum 的匹配执行保守穷尽性检查。

局部绑定还支持：

```telora
if let Some(value) = candidate { value } else { fallback }

let Some(value) = candidate else {
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
        Ok((a, b))
    };
```

`?` 是 surface elaboration，最终执行普通 match 和 return 控制流。

## 5. 绑定、函数和递归

`let` 定义词法局部值；`def` 定义具名模块 binding；`decl` 可以先声明契约；`native`
只允许在 builtin `std` module 中声明 Host 实现：

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

泛型函数模板属于静态声明，不是运行时值。导出但未实例化的模板可以保留其绑定
类型参数并接受契约检查，不要求凭空选择具体类型。进入求值路径的函数引用必须
在全图类型求解期间，通过显式类型实参或上下文确定具体实例；未确定的类型参数
不能通过重新泛化来成为值，也不能用于值比较。MIR 封闭之后不再补充实例或求解
类型。例如 `identity@[Int] == identity@[Int]` 的类型身份明确，而缺少其他类型
证据的 `identity == identity` 必须报告静态错误。

Call section 使用占位符构造 closure，是普通函数的便利表面；显式 `fn` 始终可以表达
同一计算。

## 6. 静态类型

Telora 使用结构化静态类型和双向检查。当前公开类型类别包括：

```text
Int, Float, String, Bytes
Array(A), Dict(A), Tuple([...])
() (Unit)
Struct, Enum
Fn(...) -> ...
Type, TypeOf(A), Dyn, opaque native type
Never
```

匿名 Record 按字段结构检查。直接 `struct` / `enum` 类型声明具有声明拥有的名义身份；
相同结构的两个声明互不兼容。type alias 不创建新的身份，只保留被引用类型的身份。
Native opaque type 具有由注册模块和 slot 决定的名义身份，普通用户代码不能伪造其值。

类型关系通过泛型参数、具体类型和显式 enum 契约表达。需要开放值容器时，使用
带类型见证的 `Dyn`；归一化数据使用 `Value`。恢复分析的 Unknown 是 fact state。
`Never` 表示不产生值的路径，例如 `return`、`fail!` 和 `panic!`；它作为 bottom
参与方向性检查，避免根失败制造级联类型错误。

### 6.1 函数契约和 rank-1 多态

函数契约使用专用的 `Fn(P1, ..., Pn) -> R` 记法，降解到不受变量遮蔽影响的内部
类型构造器。它与静态类型写法 `Func([P1, ..., Pn], R)` 表示相同类型；二者都不是
普通数据函数。参数和结果位置都递归接受完整契约记法，例如：

```telora
Fn(A) -> Tuple([B, C])
Fn(Fn(A) -> B) -> Array(Tuple([A, B]))
Fn(types.Input, Array(types.Item)) -> types.Output
Fn() -> ()
Fn(()) -> Unit
```

`Fn() -> ()` 没有参数；`Fn(()) -> Unit` 有一个空 Tuple 参数。`Fn` 和调用 arity
规则不变，不能省略 `Fn`。

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
类型参数从完整调用收集证据；variant 构造的名称确定类型族，同次调用中的其他
实参可以补全该类型族的参数和 payload 的类型上下文。调用按具名 enum 检查
variant 及 payload；不相关 enum 或 enum 之外的 variant 是类型错误。
匿名 Struct 实参同样在完整调用上下文中检查。若 generic callback 的结果确定了共享
Struct 类型，较早书写的 seed 中的 variant 构造字段和空 collection 字段按该结果
检查；例如 fold callback 返回 Bool 时，seed 的 `{flag: False, items: []}` 可
参与 `{flag: Bool, items: Array(A)}`。若 callback 分支没有共同契约，必须显式提供
完整返回类型；不相关 variant、字段 shape 冲突和不唯一的补全仍是类型错误。
closure 中同一类型族的 variant 分支合并参数证据，例如 `Some("hi")` 和 `None`
合并为 `Option(String)`。缺少证据时可通过返回类型注解、`.ty!(Option(String))`
或调用的显式泛型参数补全。未知 variant 或不兼容 payload 仍产生类型错误。
未标记的 `expression[index]` 只表示 Array 索引，不表示类型应用。

显式 `def` 契约作为 rigid expected type 参与严格双向检查；这同时适用于 inline
契约和先 `decl` 后初始化的定义。工具阶段的 shallow projection 可以产生 provisional
fact；定义是否合法以严格检查的结果为准。

未标注、由 closure 字面量初始化的合格局部 binding 可以得到保守 rank-1 scheme。
Generalization 保留尚未解决的 callable 和数值 obligation；递归组、不稳定约束以及
普通 alias 不会被无条件泛化。跨模块导出的 scheme 会在每个合法使用点重新实例化，
且其私有 bound identity 不泄漏到导入方。

### 6.4 静态 Trait 与约束

Trait 声明定义一个由 provider module 和稳定 local slot 标识的 nominal capability：

```telora
trait Display {
    display: Fn(Self) -> Fmt,
};

impl Display for Endpoint {
    display: fn(value) {
        fmt.concat(
            ["", ":", ""],
            [fmt.from_string(value.host), fmt.from_int(value.port)],
        )
    },
};

impl(T: Property(DisplayBy)) Display for T {
    display: fn(value) {
        let property = type_property.evidence(T.type, DisplayBy.type);
        property.display(dyn.pack(T.type, value))
    },
};

def render: for(T: Display) Fn(T) -> String = fn(value) {
    fmt.render(Display.display(value))
};
```

`Self` 只在 trait member contract 中绑定。`impl(T: Bound) Trait for Target` 的参数
与约束属于该 impl 声明；`for(T) Fn(T) -> T` 中的 `for` 则属于值的多态类型。
Impl 是顶层静态声明，必须完整且精确地
实现 member contract；调用使用 `Trait.member(value)`，不进行 receiver method lookup。
泛型参数可用 `+` 声明多个约束。发布的 rank-1 `TypeScheme` 按 canonical `TraitId`
保留约束，import、alias、reexport 和 recovery 均复用同一身份。

编译器把 trait 调用 elaboration 为普通 dictionary 函数调用。受约束函数内部接收
隐藏 evidence 参数，调用方静态选择唯一 impl 并传入 dictionary。VM 不搜索 impl，
也没有 trait-object value kind。Coherence 拒绝重复的精确 impl 和无优先级的重叠
blanket impl；精确 impl 优先于满足约束的 property blanket impl。orphan boundary
要求 impl module 拥有 trait 或目标最外层 nominal constructor。

`Property(P)` 是内建约束：`T: Property(P)` 根据 provider 的返回类型证明精确的
`Ty(T, P)` typed property 存在，无需先求出 property 值。它可以驱动 blanket impl，
而普通反射查询仍返回 `Option(P)`。静态 evidence 引用待生成的 payload binding；
模块类型检查通过后才计算 property 值，全部成功后才能发布模块。
Implementation selection 不读取 payload 内容。

当前不存在 higher-rank type、用户定义或通用 subtyping、trait object、interface、
associated type、default member、specialization、trait inheritance 或 higher-kinded
type。

## 7. 类型元数据

类型由声明、受信任的结构类型构造器或参数化类型族产生。`T.type` 将静态类型
单向投影成规范元数据，结果为精确的 `TypeOf(T)`，可赋给 `Type`。理解该模型需要区分：

```text
Type       任意有效 TypeMetadata 值的静态类型
TypeOf(A)  精确证明某个元数据描述 A
TypeDesc   用户态解释器观察的擦除后 descriptor 视图
```

`TypeDesc` 在这里表示公开观察模型，不是与 `Type` 并列的可构造静态类型。当前
`std/type-desc` observer 接受 `Type` 值，并通过 kind、children 和 resolve 等操作
暴露规范 descriptor 视图。声明类型以 `TypeDescKind.Ref` 暴露，通过 `resolve` 获得其结构 descriptor；
property 由独立的 property registry 按目标与 property 类型查询。

类型构造及其显式元数据投影：

```telora
Array(String).type
(Fn(Int, Int) -> Int).type
Option(String).type
```

规范函数元数据的公开 descriptor kind 是 `TypeDescKind.Func`，其 `parameters` 是 TypeMetadata
Array，`result` 是单个 TypeMetadata。`std/type-desc` 和 `std/dyn` 对函数元数据的 kind
观察分别返回各自 kind enum 的 `Func` 成员。`Function` 不具有语言保留意义，可以作为普通领域标识符使用。

`let` / `def` 绑定数据，`type` 绑定类型。类型角色由解析后的声明身份和模块接口决定，
不依赖大小写或运行时 `TypeOf` 内容。`export { T }` 保留类型身份；导出 `T.type`
得到的普通数据不能再用于静态类型位置。`.type` 是关键字后缀，不是字段查询，不能用于
普通数据，也不触发无关 property provider 求值。

普通函数可以传递、返回和组合元数据，但不能用其结果定义类型或类型 annotation。
`type T = build_type(...)`、`type T = metadata` 和 `type T = Int.type` 都被拒绝。
结构类型仍可复用工具阶段 VM 的受信任构造机制；必须先验证类型/数据边界，不允许执行
任意用户 helper 来决定类型骨架。Property 是带外信息，不改变此边界。

### 7.1 Struct、Enum 和 typed property decorator

`Unchecked(T)` 是具名字段 struct T 的候选值类型，保留字段类型、泛型参数和
原值来源。它满足 `Unchecked(Unchecked(T)) = Unchecked(T)`。需要 T 的上下文
完成候选值的构造；Dyn 投影保持两者的类型身份区别。

`@check(func)` 为 struct、newtype 或带载荷 enum variant 定义构造校验。
具名字段 struct 的校验参数为 `Unchecked(T)`，newtype 与 variant 的校验参数
为载荷类型；返回 `Result((), BlameError)`，`Ok(())` 接受原候选值，`Err(error)` 拒绝构造。
校验器可用 `?` 组合 Result 校验；不返回替换值，也不隐式将 `()` 提升为 `Ok(())`。
无载荷 variant 直接成立，不接受校验装饰器。没有校验的候选值直接完成构造。

普通构造的拒绝产生失败诊断，codec 解码的拒绝返回 `Err(BlameError)`。
校验覆盖泛型函数体、工具阶段、merge-update 和投影中的每次新构造；嵌套值
先完成自身校验，再校验外层候选值。读取、复制、编码和传递已构造的值不重复
校验。运行时用类型见证实例化泛型函数体内的构造目标，并保留规则与输入来源。

构造校验适合表达值自身的不变量，merge-update 则复用已有合法值的其余字段：

```telora
@check(fn(window) {
    if window.limit <= 0 {
        Err(blame!("limit must be positive", window.limit))
    } else if window.offset < 0 {
        Err(blame!("offset must be non-negative", window.offset))
    } else {
        Ok(())
    }
})
type Window = struct { limit: Int, offset: Int };

def first: Window = { limit: 10, offset: 0 };
def next = first <~ { offset: 10 };
```

`next` 保留 limit，更新后的整个 Window 再接受校验；`first <~ { limit: 0 }` 会被拒绝。
同时修改互相约束的字段时，应放在同一次更新中，避免要求中间值也满足最终约束。
需要外部授权、关系图或最终排序信息的规则仍由掌握这些信息的函数检查，不能仅凭
局部对象的字段完成。校验器返回 `Err(blame!(...))` 可保留具体字段的输入来源。

装饰器名称也遵循普通 resolve 规则。例如 `import "std/type-property" as property;`
会遮蔽 prelude 的 `property` 函数；需要同时使用时可显式导入
`import "std/prelude" as prelude;`，并写 `@prelude.property(...)`。

`type A = struct(B);` 声明单元素具名 tuple（newtype），具有独立的 nominal
identity。`a.0` 返回 B，并保留载荷的类型身份和来源；其他位置索引不成立。
newtype 和 B 不存在隐式包装或解包转换。其元数据解析后的 kind 为 Newtype，
children 包含唯一的载荷类型；JSON codec 使用载荷表示并在成功解码后附加外层身份。

构造器模式 `A(payload)` 按声明的具名身份匹配 A，并对其唯一载荷递归匹配。
模式支持模块限定名称、泛型和嵌套；模式中的名称必须引用类型声明。
`let A(value) = a;` 绑定载荷，`a.0` 直接读取同一载荷。

构造器也可以用于工具阶段。`type Wrapped = struct(Type);` 后的
`let metadata = Wrapped(Int.type).0;` 得到描述 Int 的元数据数据，不能反向用于
`type Selected = metadata;`。工具函数和 decorator 参数可以构造、传递和读取
newtype 值，遵守同样的具名身份与类型上下文规则。

enum 的成员名称提供值构造器。对于
`type Event = enum { Progress(Int), Finished };`，`Event.Progress` 的类型是
`Fn(Int) -> Event`，`Event.Finished` 的类型是 Event。解析到的声明确定成员所属
enum；同形的另一个具名 enum 不改变该身份。泛型成员的契约保留所属类型族的
所有参数，可使用 `Result.Ok@[Int, String]` 显式指定。缺少载荷或结果上下文中
不能确定的泛型参数时，编译器要求补充类型证据。

模块命名空间与选择性导入的类型是不同的绑定。模块别名与导出类型可以同名，
限定链逐级解析，例如 `Model.Model.Data(1)` 中前两级分别表示模块和类型。

newtype 声明在值位置提供 `Fn(B) -> A` 构造器；`A(value)`、函数传递和显式
泛型应用遵循普通函数契约。类型位置使用声明的类型用途；普通 Type 参数必须
显式传入 `A.type`，不能靠预期类型把裸 A 隐式转为元数据。
模块接口保留导出名称的类型声明身份，因此限定引用、导入别名和再导出都能保留
这两个用途。普通 Type 值以及返回 Type 的普通函数不因其返回契约而成为构造器。

具名 Struct 和 Enum 使用 `type` 的专用声明初始化器：

```telora
type User = struct {
    id: Int,
    name: String,
};

type Status = enum {
    Pending,
    Failed(String),
};
```

`struct` 和 `enum` 只在 `type Name(...) =` 后的直接初始化位置具有该含义；它们不是
普通 Function，不能捕获、传递或调用。公开 prelude 不提供同名 metadata 构造器，
`@struct` 和 `@enum` 也不是兼容语法。

每个直接声明拥有由 provider module 与声明位置确定的私有身份。不同声明即使字段或
variant 完全相同，也不是同一个类型；alias、import 和 reexport 则保留原身份。普通
TypeMetadata 值、codec、schema、`Dyn`、TypeDesc 和工具从同一份权威元数据图工作。

Decorator 只适用于没有类型参数的具名 Struct/Enum 声明及其直接字段/variant。
Property carrier 本身也必须是具名 Struct/Enum，并用内建 capability 标记：

```telora
@property(PropertyTarget.Type)
type DisplayBy = struct { template: String };

def display_by: Fn(String) -> Fn(TypeDesc, Option(DisplayBy)) -> DisplayBy = fn(template) {
    fn(target, previous) {
        let property: DisplayBy = {template};
        property
    }
};

@display_by("{host}:{port}")
type Endpoint = struct { host: String, port: Int };
```

`@property` 接收一个工具阶段可求值的 `PropertyTarget` 表达式。
这个内建具名枚举包括 `Type`、`StructType`、`EnumType`、`Member`、`Field` 和 `Variant`；
`Type`/`Member` 分别覆盖两类 type/member owner，多个标记按位合并。系统先封闭
`Endpoint` 的 TypeId、TypeMetadata 和 canonical member index，再执行 provider，并以
`Ty(Endpoint, DisplayBy)` 发布结果。provider 从只读 context 计算 property value，
property registry 与目标 descriptor 独立；这个执行模型保持目标结构与身份稳定。
Field/Variant 使用 owner
TypeId、按 member name 排序得到的
零基 canonical index 和 property TypeId 作为键。Decorator 的当前适用范围是具名
Struct/Enum 及其直接 member。

同 key 的多个 provider 接受 `Option(previous)` 并按词法顺序 fold，provider 可自行
合并、替换或拒绝前值。字段/variant decorator 全部完成后才运行 type decorator；后者
可读取完整 member-property snapshot。失败不会发布声明的部分 property。

当前反射接口显式传递两个类型 witness：

```telora
import "std/type-property" as type_property;
let property: Option(DisplayBy) = type_property.get_type_prop(Endpoint.type, DisplayBy.type);
```

member 查询分别是 `get_field_prop(Owner.type, index, P.type)` 和
`get_variant_prop(Owner.type, index, P.type)`，其中 Owner 和 P 是类型声明。
查询返回 MainWorld 中 property 的廉价引用。
在 `T: Property(P)` 约束范围内，编译器通过隐藏 evidence 传递同一份已发布 payload；
约束外的显式查询仍保持上述 `Option(P)` API。

`std/type-desc.fields/variants` 使用相同 canonical index 枚举 member；
`std/dyn.get_field_value/get_variant_index/get_variant_payload` 根据 `Dyn` 携带的权威
descriptor 安全投影。错误 kind、越界和 variant mismatch 是有来源的运行错误；合法
unit variant 的 payload 是 `None`。

Interpreter、静态 trait implementation 和后续 quote/codegen 可以读取同一封闭
骨架和 property registry，并基于这两个稳定输入赋予 property 行为意义。

### 7.2 参数化 TypeMetadata family

参数化 `type` 声明创建可命名的元数据 family：

```telora
type Box(Item) = struct {value: Item};

def wrap: for(Item) Fn(Item) -> Box(Item) = fn(value) {
    {value}
};
```

类型族应用和元数据投影分别写作：

```text
类型位置  Box(A)
元数据    Box(A).type : TypeOf(Box(A))
```

声明 body 使用刚性符号参数求值一次，产生规范模板；`Box(String)` 对模板做避免捕获
的替换，不用 String 重新执行 body。每个完整应用的身份由 provider 声明 head 与
规范类型实参共同确定；同一应用重复求值返回同一个类型身份。因此 family 不是任意
type-level function，也不是 higher-kinded constructor。

Family 必须一次提供全部参数，不支持 partial application。无环 family 可以组合本
模块中的其他 family 和非参数化 concrete type，也可以跨完整、选择性、alias 和 open
import 保留精确 scheme。Family 可达的本地 TypeMetadata 依赖按语义依赖图求值，声明
顺序不影响结果。

Family 声明可以捕获已经封闭的非参数化递归 concrete type，但不能依赖同模块的
普通 helper；可以依赖内建类型构造器和 imported 类型声明/类型族。这一边界
避免把普通源码求值顺序或尚未 sealing 的 recursive reference 带入符号模板。

名义 Struct/Enum family 可以直接以完全相同、顺序不变的参数自递归：

```telora
type Expr(A) = enum {
    Leaf(A),
    Call(Array(Expr(A))),
};
```

该声明只求值一次，并把 `Expr(A)` 表示为同一 symbolic root 的有限图回边。具体化以
`(TypeConstructorId, TypeArgs)` 为 canonical key；重复 `Expr(Int)` 是同一类型，
`Expr(String)` 则是另一类型。严格分析与恢复分析都使用这一闭合规则。

这一能力不是一般递归 type-level function。递归调用必须原样使用当前全部 Bound 参数；
`F(Array(A))`、`F(B, A)`、mutual family cycle、family/concrete mixed cycle，以及
`type F(A) = F(A)` 这样的无生产 alias 均在声明处拒绝。具体应用以有限符号模板进行替换。

### 7.3 递归元数据

递归类型使用有限图和受控 reference 表示，而不是无限展开的树。成功初始化的递归
root 在冻结后发布；未初始化 reference 不能进入 persistent world。

严格模块分析与 `check`/`query` 的恢复分析对递归 concrete type 使用相同的 component
sealing：先为同一递归 component 的全部 root 建立具名 reference，再整体求值并安装
有限图。第 7.2 节允许的单个名义 family 同样先建立带 Bound 参数的 symbolic root，
再封闭同参自回边。旁支 binding 的语法、类型或运行时失败不会把这些类型误报为不可
部分求值。裸 type alias 环、mutual family cycle 和 mixed cycle 仍拒绝。

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

同一 WorkWorld 中，`interpreter!` 产生的同一个外层 closure 使用相同 canonical
`TypeId` witness tuple 调用时，复用同一个成功生成的 inner closure identity。key 是
`(interpreter closure identity, TypeId tuple)`；不同 interpreter identity 或 TypeId
不会共享。命中仍计一次函数调用 fuel，但不重复分配 inner closure。非 Type Host 参数
在运行时边界拒绝，失败结果不进入缓存。跨 World 稳定性来自普通 closure publication：
发布后的 property/evidence root 被 import、reexport 和重复读取时保持同一函数 identity。

memoization 只缓存 lifting 所构造的 wrapper；它不会提前执行 inner operand。读取类型
metadata、解析 eDSL 并产生领域 closure 的准备逻辑仍由普通 tool-stage/property 代码
显式定义，如 `DisplayBy.display`。

### 7.5 静态约束与 Dyn 投影

以下边界具有不同语义，不能互相替代：

```telora
let empty = [].ty!(Array(Int));
let truth = ty!(True, Bool);

import "std/dyn" as dyn;
let exact = dyn.project_with(User.type, package); // Option(User)
let exact_sugar = dyn.project@[User](package);
```

`ty!(expr, T)` 与 `expr.ty!(T)` 是零运行时的静态 ascription。`T` 作为 expected type
向 `expr` 内部传递；无法证明时前端报错。它不改变 payload、名义 witness 或来源，
Dyn 中的值需要显式投影为具体类型。


`Dyn` 精确投影比较打包 descriptor 与目标的 canonical type identity，不走结构比较或
assignability。`project@[T]` 只对解析到 `std/dyn` namespace 的 `project` 生效，并等价于
`project_with(T.type, package)`；它不是所有泛型调用的隐式 witness 规则。当前符号泛型 T
若没有可物化的运行时 `TypeOf(T)` witness，会在前端拒绝。

## 8. 模块和封闭世界

模块通过静态路径组成不可变有向图。Import 必须在初始化前解析，程序不能根据运行
时值决定依赖。

公开 import 表面包括：

```telora
import "std/array" as array;
import "@src/model" { User, make_user };
import "./validation" { validate };
import "package/path";
import "package/path" as model, { User };
```

支持 namespace、选择性、alias 和 open import。所有形式必须观察同一个 export
scheme，不能因导入写法不同而丢失泛型关系。

默认 prelude 等价于一个隐式 open import：它只向尚未被本模块声明的名字提供
fallback binding，不是保留名称集合，也不形成不可遮蔽的词法外层。本地 `let`、
`def`、`type` 等 binding 正常遮蔽同名 prelude 项。若同一模块还需要访问被遮蔽的
prelude 项，应显式使用 namespace 或 selective alias import，例如：

```telora
import "std/prelude" { PropertyAttr as BuiltinPropertyAttr };
```

`import` 永远只在当前模块建立 local binding；它本身不改变当前 Module 的公共接口，
也不存在 implicit public import。只有显式 `export` 才把一个当前可见的 local binding
映射到 Module interface：

```telora
import "@src/types" {Plan as LocalPlan};
export {LocalPlan as Plan};
```

`export {a as b}` 只建立接口映射 `local a -> public b`。它不在当前模块建立名为
`b` 的 local binding，因此后续本地表达式不能因该 export 而 resolve `b`。Public
alias 只供下游 import 和 Module member resolution 使用。

`export def` 和 `export type` 是普通 module binding 后接 export marker
的语法糖。例如 `export def f = value;` 与 `def f = value; export {f};` 具有相同
语义。本地 `f` 由 `def` 建立；export marker 不建立 lexical binding、不执行用户代码，
只选择要发布的 local binding。

Module 顶层是声明空间，不是普通 expression block。顶层值只使用 `def`；`let` 只允许
出现在函数或 `do` 等普通 block 中，用于顺序求值、词法局部和 shadow。复杂模块值的
初始化把局部步骤封装在 initializer expression 中：

```telora
def answer: Int = do {
    let base = 40;
    base + 2
};
```

模块顶层不接受 `let`、`export let`、裸表达式或 final expression。

被 export 选中的 local binding 可以由当前模块声明，也可以由 selective、aliased、
open 或 namespace import 建立。导出 imported local 时保留原 value identity、精确
TypeScheme、concrete/recursive TypeMetadata graph、type-family template、opaque provider
identity 和 provenance；不包装、重求值或重建 binding。Namespace binding 的导出仍是
语义 Module，必须保留其 nested Module interface，不能退化为普通 Dict。

Telora crate 使用 `src/` 和 `tests/`。Host 从当前工作目录向上查找最近的
`telora-config.json`，并从 workspace 的 `telora-crate.json` 得到稳定 crate 身份、
权威模块清单和直接依赖。`src/` 中的目录没有执行身份；工具对普通 module export 的
名义类型进行验收。测试由 Host 以 `@test/<name>` 选择，`telora test <name>` 使用同一身份。

逻辑 ID 到 crate 内物理位置的映射为：

```text
@src/x          -> <crate>/src/x.telora            -> my-crate/x
@src/a/x        -> <crate>/src/a/x.telora          -> my-crate/a/x
@test/x         -> <crate>/tests/x.telora          -> my-crate/tests/x
dependency/x    -> <dependency-crate>/src/x.telora -> dependency/x
```

`src/` module 可使用 `./`、`../` 或 `@src/` 导入同 crate module。Host 选择测试根时，
先为当前 crate 的整个 `tests/` 建立固定模块清单，再从选中根发现可达图。测试根可以
位于子目录；顶层测试和辅助模块均可通过相对路径、`@test/` 或 `<crate>/tests/` 相互
导入，通过 `@src/` 导入已声明的 crate source。源码模块与其他 crate 不能访问测试
模块。依赖公开其清单中的 public module。

测试清单包含 Telora 和受支持的静态数据模块，不写入 manifest 或 lock，不扩展普通
production catalog。枚举顺序确定，符号链接（包括 tests 根）被拒绝，其他文件类型被
忽略。若 `tests/x.telora` 与已声明的 `src/tests/x.telora` 产生相同 canonical name，
测试准备阶段拒绝该冲突。清单成员不自动成为执行根，未引用文件不解析、不求值；
枚举或路径校验失败仍属于准备错误。清单固定不意味着 Host 提供原子文件内容快照。

`@src/` 以 importing module 所属 crate 为准，因此依赖模块中的 `@src/` 仍指向该依赖
自己的 source root。`./` 和 `../` 表示相对于 importing module 逻辑目录的导入，
测试内部的相对路径不能越出测试清单。Package import 的首段选择固定依赖，
`std/...` identity 来自 Telora 内置 crate。上述解析均在模块初始化前完成。

模块 import graph 不允许初始化 cycle。递归函数和递归 TypeMetadata 在单个模块及
已建立的模块接口内由各自机制处理，不把 module cycle 当作递归定义机制。

Production module 使用显式命名导出：

```telora
export def version = 1;
export def compile: Fn(Input) -> Option(Output) = fn(input) { ... };
export { User, compile };
```

模块没有由“最后一个表达式”定义的公共默认结果。Source module 的公共接口由
export record 定义，Host 再按所选协议校验或选择其中的值。

### 8.1 模块身份和依赖

模块的语义身份由 crate 和逻辑路径组成的 canonical cname 决定，不等于偶然的物理绝对路径。
相同模块经不同相对边解析时应复用同一身份；resolver 拒绝词法或 symlink 越界。

workspace 根的 `telora-config.json` 为每个 crate name 选择唯一的 workspace member
或远程 tarball source，并可为远程 source 设置 workspace 内的开发 override。每个
crate 的 `telora-crate.json` 声明 canonical name、权威普通模块清单和直接依赖名称。
`telora-lock.json` 固定 workspace 唯一精确包图。
crate 名与逻辑路径共同形成 canonical cname，物理 workspace 与缓存路径不进入身份。

`telora lock` 是唯一创建或改写 lock 的命令。其他 crate-mode 命令和 LSP 校验 lock，
通过内嵌 `telora-ees` facade 向 IMOS component 提交 `InstallShared`，物化缺失的远程
tarball，再把准备好的 immutable crate-root map 交给 resolver。resolver、module loader、
求值器和 VM 均不执行网络 I/O，也不改写 lock。

`telora-crate.json` 的 `modules` 只接受 `@src/...` selector，并且是 `src/` module 的
权威集合。未声明文件不可 import；`telora check` 会为存在但未声明的 source 或静态
数据文件产生 warning。`tests/**` 属于测试调用的独立清单，不进入 source manifest。

resolver 按顺序查询 vendor：builtin vendor 在先，当前发布 `std/*`；prepared
workspace 建立的 configured vendor 在后。vendor 以 crate 为选择颗粒；一旦
builtin vendor 提供 `std`，全部 `std/*` selector 都只在该 crate 内解析，后序同名 crate
整体被遮蔽，不能补充或覆盖 module。crate 清单在模块图发现前完成；注册采用 first-win，
当前 crate 先于 dependencies，已登记的 crate name 及其 source 指向此后不可改变。
当前没有 registry、语义版本求解或同名多版本能力；远程 source 是确定的 HTTP(S)
tarball URL。

resolver 配置只来自 prepared workspace。runtime Context、EES 和执行工具契约由
`std/entry` 的名义 wrapper 表达。Telora 源文件没有独立的 `option` 声明；`option`
可以作为普通 identifier 使用。

Telora selector 不写且不能写 `.telora`；resolver 在物理查找时补上后缀。静态数据
selector 必须保留 `.json`、`.yaml`、`.yml` 或 `.toml`。除这一项已知后缀外，文件名
不能含 `.`。文件 stem 以 `_` 开头表示 private；只有同 crate 模块和内置工具 adapter
可以访问。只有内置 `std` crate 可以声明 native symbol。

### 8.2 静态数据模块

JSON、TOML 和 YAML 与 Telora 文件共同进入模块图。它们不是运行时文件读取：Host
在封闭世界建立时加载并注册源码。每个静态数据文件都是只导出 `data: Value` 的
Module，不是一个 raw root：

```telora
import "./request.json" { data as request };
import "./policy.yaml" { data as policy };
import "./config.toml" { data as config };
```

`Value` 是 `std/value` 定义的唯一 nominal recursive semantic sum：

```telora
type Value = enum {
    None, True, False,
    Int(Int), Float(Float), String(String), Bytes(Bytes),
    Array(Array(Value)), Object(Dict(Value)),
    LocalDate(String), LocalTime(String),
    LocalDateTime(String), OffsetDateTime(String),
};
```

`ScalarValue` 表达可直接穿过标量协议边界的闭合子集，并以 untagged codec 映射为
JSON scalar：

```telora
type ScalarValue = enum {
    None,
    Bool(Bool),
    Int(Int),
    Float(Float),
    String(String),
};
```

`ScalarValue.None`、`ScalarValue.Bool(True)`、数值和字符串成员分别编码为 JSON null、boolean、number 和 string。
参数化查询使用 `Array(ScalarValue)` 表达 bindings；数据库 component 再把逻辑标量适配为
后端的物理参数类型。

每个递归子节点都携带同一个 canonical `Value` TypeId，因此可以用闭合 `match`
穷尽处理。它是规范化后的语义数据，不是 lossless AST：不保留注释、anchor/alias
身份、原始标量拼写或 table 拼写，也不暴露 VM 的 meta/runtime layout。

当前格式行为包括：

- JSON 严格解析数字、字符串和重复 key；Int 越界或非有限 Float 失败；
- TOML 支持 1.0 的核心值与表结构，四种 date/time 类别规范化为独立 Value variant；
- YAML 使用固定的保守 schema：mapping key 必须是 String，拒绝 custom tag 和非有限
  Float；标准 `!!binary` 经过 canonical base64 校验后成为 Bytes；alias 有深度与总
  展开量限制；merge 只接受 mapping 或 mapping sequence，显式字段覆盖 merged 字段，
  未被显式覆盖的重复 effective key 失败；旧式隐式 bool 和时间戳按 String 处理。

运行时文本解析使用同一边界：

```telora
json.parse(text)  # Result(Value, codec.BlameError)
yaml.parse(text)  # Result(Value, codec.BlameError)
toml.parse(text)  # Result(Value, codec.BlameError)
```

Typed model 与 Value 之间只通过 codec 重建数据图：

```telora
let model = codec.decode(Model.type, request).unwrap!();
let value = codec.encode(Value.type, model);
```

`Dyn` 是带 canonical witness 的存在类型，公共数据交换
使用 `Value`。JSON stringify 只接受 Value，但 JSON
没有 Bytes 或 temporal scalar，因此含这些 variant 的 Value 必须先由显式领域 codec
转换为 JSON 可表达的模型。

解析失败的静态数据模块不能供严格执行使用，但 workspace recovery 仍保留其 source、
syntax diagnostic 和不依赖成功值的事实。

格式 frontend 可以在内部建立 raw graph，但发布前必须一次性规范化为 Value。Variant
wrapper 不增加 provenance path segment；数组索引、对象 key 和原始 scalar 位置继续
指向输入数据。Strict load、recovery、`check` 和 `query` 观察同一个 `{data: Value}`
接口。

### 8.3 初始化和发布

模块求值区分 persistent main world 与每次操作的 temporary/work world。Import export、
closure capture、递归 metadata root 和 static data root 只有在完整初始化后才能被
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

诊断相关的 contextual intrinsic 为：

```telora
blame!(message, subjects...)   # BlameError，构造数据，不报告
raise!(error)                 # Never，报告失败
warn!(error)                  # Option(T)，报告 warning，永远返回 None
unwrap!(result)               # T
ok_or_warn!(result)           # Option(T)
fail!(message, subjects...)   # Never
panic!(message)               # Never，实现错误
```

普通函数调用不附带诊断策略。对于 `Result(T, E)`，`unwrap!` 在 Ok 时返回原
payload，在 Err 时调用 `raise!(error)`；`ok_or_warn!` 在 Ok 时返回 Some(payload)，
在 Err 时直接调用 `warn!(error)`。E 必须是 String 或规范的 opaque BlameError。
两者都只求值一次，并支持前置和后置写法。生成的报告位置属于用户的宏调用处。

`raise!` 与 `warn!` 共享错误归一化：String 只提供消息，不把 String 自身的来源
加入数据引用；BlameError 提供消息及显式 subjects，保留其来源。两者在实际调用处
添加 rule 位置。调用参数和 Result 容器不会自动成为 subjects，不接受任意可显示值。
`warn!` 是返回 Option(T) 的表达式，值永远为 None，T 由上下文确定。`raise!`
不返回值，类型为 Never。`?` 只传播原 Option/Result 分支，不产生诊断。

诊断由 message、rule 和有序数据来源组成；重复数据位置只保留一次。
rule 指向实际 authored intrinsic 调用位置，即使该调用在嵌套函数或导入的 helper
内，也不替换成外层函数调用点。实现调用栈可作为 trace 保留，不覆盖 rule。

`fail!(message, subjects...)` 严格等价于在该调用点执行
`raise!(blame!(message, subjects...))`。实现可以融合两步、避免分配临时错误对象。
`panic!(message)` 仍表示实现错误或不变量破坏，不是可预期的领域拒绝。

所有 contextual intrinsic 都支持统一后置糖：
`receiver.ident!(arguments...)` 等价于 `ident!(receiver, arguments...)`。
这不是 method lookup，也不开放用户定义宏。`dbg!` 与 `ty!` 保持各自
既有语义。函数、参数和待处理表达式按普通求值顺序执行一次。

解码使用不可观察的 native `codec.BlameError`，保存消息和失败值的来源。
`codec.decode` 与 `json.decode` 返回 `Result(A, BlameError)`，失败本身不产生诊断。
错误保留当前解码 Value，调用方可以继续试探，或显式使用其来源产生诊断：

```telora
match codec.decode(User.type, raw) {
    Ok(user) => user,
    Err(error) => raise!(error),
}
```

`string.parse` 返回 `Result(A, string.ParseError)`，错误的 value 为原始 String。
`dyn.field/fields/array_items/tuple_items/tag/payload` 返回带 `dyn.AccessError` 的
Result，错误的 value 为原始 Dyn。`type-desc.resolve` 使用 `ResolveError`，value
为输入 Type。这些错误都具有 message 字段，并直接保留原输入的来源。

被选中的 Entry 可以通过 `std/_rt` 将一次普通函数调用放入独立诊断
作用域：

```telora
rt.with_diagnostics:
    for(A, R) Fn(Fn(A) -> R)
        -> Fn(A) -> Result(Tuple([R, Array(rt.Diagnostic)]), Array(rt.Diagnostic))
```

`rt.Diagnostic` 是类型化快照，包含 `severity`、`message`、`labels` 和 `notes`。
每个标签包含 `location`、`message` 和 `primary`；位置记录源码名称以及起止字节偏移。
这些类型和捕获能力限于已有的特权 Entry 边界。

调用成功时返回值和该作用域内产生的 Warning；可恢复 failure 时返回该 failure 以及
此前产生的 Warning。返回的诊断从 evaluation account 中消费，不再由外层 Host 重复
输出。fuel、stack、allocation 和 cancellation 等终止性失败不被捕获。
作用域只改变诊断的观察边界，不把值导出到 Host，也不建立 Host-owned value。

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

观察使用有界、确定、cycle-safe 的 debug formatter，直接读取 VM 值图。
其表示独立于 JSON serialization contract。Host sink、格式化、截断或输出
失败不能改变 Telora 的值、失败、诊断、控制流、fuel、stack 或 allocation account。
Float 的 debug 表示与 Rust `f64` 的 `{:?}` 一致且不受 locale 影响；它与 Display
表示有意区分，例如 `3.0` 和 `-0.0` 的 debug 表示分别保留为 `3.0` 和 `-0.0`。

CLI 把每个事件作为一行紧凑 JSON 写入 stderr：

```json
{"name":"value","repr":"3","module":"@src/query","line":42}
{"name":"plan","repr":"{...}","module":"@src/query","line":43,"message":"generated"}
```

`name` 是首个参数的 authored expression text，`repr` 是有界 debug 表示。stderr 事件
不进入模块 export、stdout、诊断集合或内置 run Entry 的 `output` 发布协议。语言不
提供 `std/debug` 模块或 context-free `dbg` 函数。

### 9.4 Host-observed diagnostic

Warning 和 failure 诊断属于 evaluation account，而不是普通 Array 返回值。Host
负责排序、去重、渲染、JSONL 格式和退出协议；Telora 代码不能观察 Host 是否保存或
展示 Warning。`warn!` 与 `ok_or_warn!` 产生非阻塞 Warning；`raise!`、
`unwrap!` 和 `fail!` 使当前结果不可产生。

严格求值和 best-effort 求值使用相同的 rule 与 data sources。Fail 进入 failure arena
之后，所有依赖传播都引用同一个 root failure；传播点不得生成替代诊断，也不得改写
原 rule、data sources 或其顺序。

在 best-effort 求值中，`fail!` 得到的内部 `Never` 会阻止所有依赖计算执行；Host
仍可继续已经证明独立的求值单元，以一次收集更多根因。Struct、Tuple、Array、tagged
payload 和 Dict 在诊断求值图中可以暂时保留失败子节点。这类节点保持原有静态类型，
但不是 Telora 值，源码不能构造、匹配或恢复它们。保形逐项操作可以跳过失败槽位并
继续健康槽位；只依赖容器形状的操作不依赖子节点；选择失败槽位则传播同一根诊断。

Fail 传播按操作实际读取的数据确定：

- 标量一元/二元运算、比较、插值、条件、match 判别、字段/索引/tuple 投影，以及
  普通 Func 的 callee 或任一实参直接为 Fail 时，结果传播该 Fail；被阻断的 Func
  body 不执行。tag constructor 属于复合值构造，可以保留失败 payload。
- Array、Tuple、Struct/Dict 和 tagged 构造只确定外层形状，可以保留失败子节点。
  `array.length`、`enumerate`、`push`、`zip` 以及 Dict 的 keys/values/pairs 等仅依赖
  已知形状或保形搬运的操作可以继续；选中失败子节点时才传播。
- `array.map` 和 `dict.map_values` 保留原位置的失败节点，并继续处理健康成员。
  `filter`、`flat_map`、`concat`、spread 和 fold 一类输出形状或 accumulator 依赖
  失败节点的操作，其最终值传播 Fail。filter/flat_map 可以先继续彼此独立的健康
  callback 以收集诊断；fold 的 accumulator 失败后立即停止依赖它的 reducer。
- `any` 找到健康的 True、`all` 找到健康的 False 时，短路结果不依赖其他槽位；否则
  任一失败 predicate 令结果为 Fail。`find` 只有在所选成员之前没有失败 predicate
  时才能产生 Some；先前失败会令成员身份不确定并传播 Fail。
- 结构相等、codec、JSON 和 Host value 会读取完整可达数据图；任一可达 Fail 都传播
  原根因或拒绝边界，不能把内部节点渲染成普通类型错误。Module 在 WorkWorld 与
  MainWorld 间的内部固化不是 Host publication，可以保留 Fail 以供下游继续诊断。

传播节点不是新的根因诊断。实现可以保存有界的传播 lineage，但同一 Fail 穿过 callback、
native 函数、模块或边界时不得产生诸如“expected Func/Array”之类的二次类型错误。
非保形操作不提供源码可观察的“健康投影”；为收集诊断而暂存的健康成员也不能被下游
代码、codec、Entry 或 Host 当作结果使用。

任何 error diagnostic 都会使本轮命令的最终结果失去 Host 发布意义，即使最终根值不依赖
内部失败且能够算出。失败节点的可达性只决定 best-effort 还能继续哪些诊断计算，不决定
结果能否交付。codec、最终返回值和 SystemEffect 都不得越过 Host 边界；一批
SystemEffect 必须先整体完成可信审计，才能执行其中第一个 effect。严格执行遇到未处理
失败立即失败。

跨模块 best-effort 始终使用普通 Module。Module 的 `Available/Unavailable` 只表达源码
是否存在；定义和表达式各自携带 `Known/Unknown/Incomputable` 等事实状态。Module 可以在
MainWorld 内部同时保留健康 export 与含 Fail 的 export：下游读取健康 export 可继续工作，
读取失败 export 则传播同一个 Fail。不存在 `PartialModule` 或 `UntrustedModule` 语言实体，
依赖边界也不重复制造根因诊断。

### 9.5 失败类别

VM 区分可恢复的程序失败与终止整个 evaluation session 的资源/一致性失败。前者包括
类型不匹配、missing field、non-exhaustive dynamic match、panic 和 `fail!`；后者
包括取消、fuel/分配/栈/调用深度耗尽以及无效 bytecode。

Workspace recovery 可以在一个 binding 或模块失败后继续独立工作，但严格 Host
执行不会把 recoverable failure 当作成功值。

## 10. 求值和资源语义

当前工具链使用 lossless CST、AST/HIR、类型分析、LIR、bytecode 和寄存器 VM 实现
语言。各层共同服从本文定义的可观察语义和来源映射，不建立彼此竞争的语言模型。
各阶段、运行时布局、模块骨架、World 和 promotion 的当前实现见
[`IMPLEMENTATION.md`](IMPLEMENTATION.md)；这些私有结构不是第二套语言语义。

### 10.1 两个阶段，一个求值器

Tool stage 执行 annotation、type initializer、decorator、module interface 和其他分析
所需的闭合计算。Program stage 使用显式 Host 输入执行普通应用函数。两者共用：

- bytecode 与函数调用规则；
- 不可变值和 heap 表示；
- fuel、stack、call-depth 和 allocation account；
- runtime failure 与来源规则。

静态 annotation 和 witness 默认从程序执行中擦除；当程序显式把 TypeMetadata 当作
普通值使用时，该值会保留到运行时。

`codec.decode(Target.type, value)` 的首个参数是受检查的 `TypeOf(Target)`；
解码返回 `Result(Target, codec.BlameError)`。编码直接返回 `Value`，失败产生诊断，
并保留失败值和编码规则的来源位置。
`codec.encode(Value.type, model)` 的首个参数固定为 canonical `TypeOf(Value)`，并从 model
已经携带的 nominal witness 选择 schema。Dyn 中的模型需先投影到具体类型。
复杂 concrete family 的定义模块应拥有一次完整实例化，并导出 concrete
alias 或 typed boundary function：

```telora
type Rejection = RejectionPayload(Entity, Dimension, Intent, Expr, Plan, Sql);

def encode_rejection = fn(value: Rejection) {
    codec.encode(Value.type, value)
};

export { Rejection, encode_rejection };
```

下游调用 `encode_rejection(value)`，不重复 family 实参；函数契约仍严格检查 value，
其 canonical witness 随值跨模块传播。该模块/API 方案适用于包含封闭递归参数的
family，不引入隐式反射或新的表面语法。

### 10.2 Fuel 和配额

Fuel 在函数调用及实际执行的 control-flow back edge 等动态扩展点扣减。直线指令和
未采取的 back edge 不消耗同类进度 fuel。Fuel 是确定的终止边界，不代表 CPU 时间。

Fuel 用于对抗执行能否收敛的不确定性，而不是对执行成本精确计费。这是既有的
求值语义（RFC 0010）：不要求逐指令、逐元素或逐字节核算，也不要求按 native
算法的内部操作次数扣减。指令融合、数据复制方式或库内部实现的变化，不应成为
重新定义 fuel 计费规则的理由。

Native function 自身必须终止，并遵守输入规模与结构限制；重新进入 Telora 的
callback 按正常调用扣减 fuel。昂贵的 native 操作可以有专门的工作量限制，但
fuel 不是墙钟超时，也不能单独保证任意 native 调用及时返回。

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
Host 选择普通模块中的导出值
  -> 固定依赖与 source snapshot
  -> 按工具要求验证 Value、Eval、Run(State) 或 Serve(State) 名义类型
  -> 解开 wrapper 中的 ContextConfig 与 Ees，并构造含参数和平台事实的 Env
  -> 在准备 WorkWorld 调用内置 Entry.config(env, wrapper)，取得 SystemCaps 和 initializer
  -> Host 按 SystemCaps 配置事件源，并提供私有 resources native
  -> 在新的运行 WorkWorld 内由 native 生成 SystemResources，并直接调用 initializer(resources, wrapper)
  -> 构造具体 State，并在唯一工具边界擦除为 actor.Service
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
telora [-C <context>] check <module>
telora [-C <context>] test <name>
telora [-C <context>] eval <module:export>
telora [-C <context>] eval-with <module:export> [--source <name>=<source>]... [-- <arg>...]
telora [-C <context>] run <module:export> [--best-effort] [--source <name>=<source>]... [--ees-var <name>=<value>]... [-- <arg>...]
telora [-C <context>] serve <module:export> [--source <name>=<source>]... [--ees-var <name>=<value>]... --bind stdio:// [-- <arg>...]
telora [-C <context>] query|q modules [-p <substring>]
telora [-C <context>] query|q exports <module> [-p <substring>]
telora [-C <context>] query|q at <module>[:<line>[:<column>]] [-p <substring>] [-k type,let,def,import]
telora [-C <context>] lock
telora lsp
```

Clap 拥有 help、version 和命令行参数校验，其输出是面向人的文本。命令通过参数校验后，
Telora Host 在 stdout 和 stderr 上只产生 JSON 或 JSONL。成功的 `eval` 输出一个 JSON
Value；`dbg!` 和命令错误在 stderr 输出 JSONL record。`check`、`test`、`query`、`run` 和
`serve` 输出各自的 JSONL 协议，`lock` 输出生成的 lock path JSON String。进程退出码
独立表达命令是否成功。

`eval @src/model:answer` 是选择普通 module 公开导出的一个示例。`eval` 要求导出类型是
`Value`；`eval-with` 要求导出类型是 `entry.Eval`。该 wrapper 中的
`entry.ContextConfig` 声明 source、环境变量和参数能力。前者只读取求值后的导出，
后者只做一次普通函数调用；二者都不运行 reducer loop 或应用 EES。

Context 声明中的名称必须唯一；CLI 提供的 source 必须与声明全集相等，环境变量必须
存在并可表示为 String。
`--` 后的参数保持顺序进入 `args`。source transport、格式验证、data limits 与
run context 相同，但 canonical source path 使用 `@eval-ctx/<name>`。物理 locator 与
未声明环境变量不进入 Telora World。

`run` 和 `serve` 从 CWD 向上发现最近的 manifest，解析 `MODULE:EXPORT`，执行普通模块，
再按 wrapper 的名义类型验收 export。`run` 要求 `entry.Run(State)`；`serve` 要求
`entry.Serve(State)`。`-C` 指定 manifest discovery 的起始目录，该目录不必就是 crate root。

`test <name>` 选择 `tests/<name>.telora`，名称必须是无后缀的规范相对路径；不接受
绝对路径、parent traversal 或 export selector。当前只支持显式选择一个 Telora 根，
private 文件不能作为根。它先使用既有诊断恢复流程检查和初始化完整可达图；有错误时
不执行用例。随后按 UTF-8 公开导出名执行入口直接导出的精确 `std/test.Test`，
普通导入不执行依赖模块的 Test，显式重导出按公开别名执行，容器和 Dyn 不参与发现。
`Test` 是标准库拥有的不透明名义类型。`should_ok`、`should_fail`、`should_fail_with`
保存零参数 thunk；正常返回（包括 False/Err）满足 should_ok，可恢复执行失败满足
should_fail，should_fail_with 还检查非空、区分大小写的主消息子串。预期失败在
用例内消费，warning 保留；终止错误不能被消费，模块初始化错误也不能被捕获。
`with_fixtures(Array(String), Fn(Value) -> Test)` 构造可嵌套组，Host 在 factory 前
准备整组直接输入。fixture 是非模块数据源，相对实际构造模块、限制在声明 crate
内；空组、准备失败和 factory 失败均明确报告，不由子 should_fail 捕获。
构造 Test、check 和 query 均不执行 thunk/factory 或获取 fixture。
输出为 `telora.test/v2` 的 diagnostic、case 和 summary；诊断保留规则与数据来源，
用例身份是入口模块、导出名、fixture 索引数组。成功要求至少一个通过用例且无失败；
可恢复失败后继续，中止错误设置 aborted 并停止，不产生未启动用例记录。
准备错误沿用 stderr 的 `telora.error/v1`，不伪造求值 summary。
此命令不运行 reducer 或应用 EES，不改变循环初始化
的拒绝规则。`check @test/...` 与显式测试 query 使用相同清单和模块身份。

其他命令的 `<module>` 是 `@src/...`、`@test/...`、依赖模块 ID，或
公开 `std/...` 模块 ID，不是物理文件名。`query exports std/string` 与
源码中的 `import "std/string"` 选择同一个内置模块身份；不存在的 `std/...` 得到
明确的 built-in module-not-found 错误，不按 workspace dependency 解析。`query`（可见
alias `q`）以稳定 `telora.query/v1` JSONL 输出语义事实。`query at <module>` 查询选中
模块的顶层 local definitions；追加 `:<line>` 或 `:<line>:<column>` 后改查整行或精确点
相交的 definition、reference 和 expression facts。`query modules` 不加载、解析或求值
模块，而是列出 `-C` 所确定
crate 的模块视图：输出 canonical cname，包含本 crate 的 public/private source、
dependency 的 public source，以及公开的内置 `std` 模块。test 不进入 catalog。
每条 module record 携带规范 module ID、`crate` / `dependency` / `builtin` origin、
`public` / `private` visibility 和 resolver format，并按 module ID 稳定排序。test 和
private built-in 不属于普通 catalog 视图。
`query exports` 独立查询公开 Module interface。`-p` 执行大小写敏感的字面子串匹配，
不解释 glob 或正则表达式：在 `modules` 下匹配规范 module ID，在 `exports` 下匹配公开
名称，在无坐标的 `at` 下匹配本地符号名。`-k` 接受由逗号分隔的 `type`、`let`、
`def`、`import`；坐标化的 `at` 不接受 `-p` 或 `-k`。空匹配成功且不输出记录。
`line` 从 1 开始，`column` 从 0 开始并按 UTF-8 byte 计数；column 必须落在 Unicode
scalar 边界。JSONL 中所有 `line/end_line` 同样从 1 开始，`column/end_column` 从 0
开始并固定为 UTF-8 byte offset，range 使用半开区间。LSP 仍按客户端协商的
UTF-8/UTF-16/UTF-32 encoding 输出其协议规定的 0-based position；终端诊断继续使用
面向人的字符位置。每条 query 记录显式区分 `authoritative`、`recovery` 或 `debug` 权威层级；
表达式级记录属于 `debug`，错误恢复所得记录的权威层级服从对应事实状态，而不是模块级
“完整性”状态。
Namespace import 的 definition record 以 `target` 给出被导入模块的稳定 ID，并省略
普通值的 `type` 字段；其成员的精确公开 type/scheme 由 `query exports <target>`
记录定义。Selective import 仍在本地 definition record 中直接携带所选成员的精确
type/scheme。Namespace 保留模块接口中每个导出项的精确契约。

`check` 用统一 Module 管线的 best-effort 调度策略求值。独立计算可以在失败后继续，以收集
更多诊断；任何语法、类型、解析或运行时 error 都会令命令失败，即使某个干净的最终根仍可
算出。内部 Module graph 仍可保留，用于查询健康事实和诊断因果，但不会作为 Host 结果交付。
没有 error 时，best-effort 与严格模式同属成功且值语义一致；`check` 不再额外重跑一条
strict/recovery finalization 管线。Module graph 不物化为 legacy Host `Value`，因此合法的
递归 TypeMetadata 和递归函数闭包不会因 Host value 边界而失败。`check` 的 stdout 完全采用
`telora.check/v1`
JSONL：先按稳定顺序输出
零到多条 `diagnostic` record，最后恰好一条 `summary` record；summary 包含稳定 module
ID、dependency 数量和 `ok` 或 `error` status。Warning 本身不阻止成功；失败不伪造
Module value，并以非零退出。普通 stderr 只用于 CLI/Host 故障，`dbg!` 仍是独立旁路。

`query` 查询同一普通 Module 管线产生的全面证据图，包括 recoverable CST、部分语义事实和
诊断求值结果，因此存在错误或求值失败时仍可返回不受影响的事实。`query` 成功只表示查询
成功，不表示模块健康；恢复节点通过独立 fact state 表达未确定或失败的状态。

`run --best-effort` 在启动 Entry 前对 Main 执行静默的 best-effort 诊断求值，并把
`telora.run/v1` diagnostic records 写入 stderr。它只用于遇到问题时扩大诊断覆盖：只要
本轮出现任何 error，恢复得到的 Main 就整体废弃，命令输出 error summary、非零退出，且
不初始化 Entry、不解释 SystemEffect；一个不依赖失败的干净根值也不例外。没有 error 时，
命令重新进入严格 Entry reducer 与 Host effect lifecycle，不进行 speculative recovery。
最终验收必须使用省略该参数、保持 fail-fast 的普通 `run`。

普通 module 用 `std/entry` 构造工具 wrapper。应用以具体 State 类型编写：

```telora
import "std/ees" as ees;

def config: entry.ContextConfig = {sources: [], envs: [], args: False};
export def run = entry.run(State.type, config, ees.none, fn(ctx) {
    let initial: State = ...;
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = ...;
    (initial, reduce)
});
```

`entry.Run(State)` / `Serve(State)` 保留 State 类型到工具阶段。内置 Entry adapter 调用
wrapper 中的初始化函数，再在 actor service 边界把 State 擦除为 Dyn；应用 reducer 内
仍保留具体 State 类型。每个 transition 返回完整新 State 与 `Array(actor.Effect)`。
Event 与 Effect 都是一阶数据，不包含 callback 或 continuation。

```text
Event  = Request {id, input}
       | EesReply {id, request_id, result}

Effect = EesCall {id, request_id, request}
       | Reply {request_id, value}
```

`run` 产生唯一 `Request {id: "run", input: None}`，持续处理 EES reply，收到对应 Reply
后输出其中的 Value 并退出。`serve` 为每个 stdin JSONL 输入分配 request ID；多个请求和
EES call 可以同时 pending，reply 通过显式 ID 关联并可按完成顺序输出。重复的活动 call
ID、未知 request、存在活动 call 时提前 Reply，以及同一请求重复 Reply 都是协议错误。

`ees.Config` 声明完整 actor 集合。Host 据此构造 application EES service：

```telora
def ees_config: ees.Config = {
    vars: {"tenant": "[a-z][a-z0-9-]{0,31}"},
    models: [
        ees.imos_model(
            "materializer",
            "user-cache:imos/{tenant}",
            "user-data:materialized/{tenant}",
        ),
        ees.sqlite_model("catalog", "user-data:catalog/{tenant}/db.sqlite"),
    ],
};
```

actor name 在所有 component 间唯一。`ees.Config.vars` 的 Dict key 是变量名，value 是自动
进行整串匹配的正则表达式。
每个声明变量都必须被 locator 使用并通过 `--ees-var NAME=VALUE` 恰好绑定一次；未知、缺失、
重复、格式不匹配以及含路径分隔符的值都失败。EES 声明不改变 reducer 接口；Host 校验
model 名称、kind 和配置完全一致。

`std/ees.Request` 是 `(model, operation, input)` 的 component-neutral 普通数据。
应用用 `actor.ees_call(id, request_id, request)` 发出调用；Host 异步执行并把结果作为
`actor.EesReply` 重新送入 reducer。多步调用所需的阶段与关联信息保存在显式 State 中。
`std/ees.install_shared` 和 `std/ees.sqlite_query` 只构造 component-neutral request，
不执行 I/O。资源 locator
使用 `user-data:`、`user-cache:`、`user-config:` 或 `user-state:`，冒号后是规范化
相对路径。component adapter 优先使用对应的 `XDG_DATA_HOME`、`XDG_CACHE_HOME`、
`XDG_CONFIG_HOME`、`XDG_STATE_HOME`；缺失时分别回退到 `$HOME/.local/share`、
`$HOME/.cache`、`$HOME/.config`、`$HOME/.local/state`。Entry 能看到 wrapper 声明的
逻辑 locator，普通应用代码不能读取解析后的物理路径；物理路径不进入 Telora World，也不能由
operation 改写。

Package Host 使用单独的私有 Service，其中的 actor 名为 `telora-packages`。该 actor 不进入应用
`Env`、`SystemCaps` 或 manifest；应用即使使用相同逻辑名字，也只能寻址自己 Service 中
显式绑定的实例。

`entry.ContextConfig.sources` 声明初始化 source 契约。声明名必须唯一，且 CLI 提供的
source 名与声明集合完全相等。`--source name=path.json` 按文件扩展名选择
JSON/YAML/TOML；`file+json://path` 等形式显式选择文件 transport 和格式；
`stdin+json://` 等形式从 stdin 读取一次。一次运行最多有一个 stdin source。
`serve --bind stdio://` 使用 stdin 作为 JSONL 请求通道，因此拒绝 stdin 初始化 source。
`Context.sources` 的 key 是 CLI 中的 `name`，value 是对应 source 解析出的数据。
数据的 canonical source path 是由 key 确定的 `@run-ctx/<name>`；保留字符按 UTF-8 byte
百分号编码。文件路径或 stdin URI 只是 Host 私有 locator，不进入 provenance 和普通
诊断。`@run-ctx/<name>` 不是 module ID：它不进入模块图、不能被 import，也不出现在
`query modules` 中。
所有 source 先经过统一 CST admission，再作为带 provenance 的 `Value` 直接物化到
Entry WorkWorld；内置 adapter 只把 `SrcItem.data` 投影为 `Context.sources`。

Entry 在纯 Telora 中实现以下 ABI：

```telora
import "std/_rt" as rt;

export type MainType = ...;
export type State = ...;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.Env, MainType) -> Tuple([rt.SystemCaps, Initializer])
    = ...;
```

`Env` 包含 `--` 后的 Entry 参数、OS/arch 平台事实和 CLI 提供的具名 data sources。
准备 WorkWorld 中的 `config` 接收已验收的 wrapper，并返回经 Host
校验的 `SystemCaps` 和 initializer；Host 根据 caps 配置事件源并提供私有 native，随后初始化
运行时服务。runtime 在同一个 Entry WorkWorld 中调用
native 生成 `SystemResources`，并把结果直接传给 initializer；该值不返回 Rust Host，也不
在 Host 侧解码或重建。所有环境上下文必须由
initializer 显式传给 wrapper factory。运行阶段使用一系列 WorkWorld；
`MainType` 是内置 Entry 验收的 `Run(State)` 或 `Serve(State)` wrapper family。
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
DataFormat = Json | Yaml | Toml
DataSrc = { default: Option(Value), fmt: DataFormat, src: String }
Env = {
    args: Array(String), ees: Dict(String),
    platform: Platform, sources: Dict(DataSrc),
}
TextSrc = { default: Option(String), src: String }
SystemStdin = Text | Lined | Null
SystemCaps = {
    data_srcs: Dict(DataSrc), ees: Dict(String),
    text_srcs: Dict(TextSrc), vars: Array(String), stdin: SystemStdin,
}
SrcItem(T) = { data: T, src: String }
SystemResources = {
    data: Dict(SrcItem(Value)), texts: Dict(SrcItem(String)),
    vars: Dict(String), stdin: Option(String),
}

SystemEvent = Initialize
            | EesReply({key: String, result: Result(Value, String)})
            | StdinLine(Option(String))

SystemEffect = EesCall({actor: String, input: Value, key: String, operation: String})
             | Output(String)
             | Exit(Int)
```

`Text` 在 initializer 前读到 EOF，并通过 `SystemResources.stdin` 注入完整文本；
`Lined` 不注入完整文本，而是在 `Initialize` 之后逐行发送不含换行符的 `Some`，EOF
恰好发送一次 `None`；`Null` 不读取也不产生事件。私有 resources native 对 `data_srcs`
复用 JSON/YAML/TOML import 的完整数据源管线，在当前 Entry WorkWorld 中直接生成带
provenance 的 `Value` 和完整 `SystemResources`；静态 import 则在依赖图和稳定 module
slot 建立后，直接把相同的 `Value` 物化到尚未封闭的 MainWorld。两条路径都不构造中间
`DataWorld`，不把 Telora value 解码为 Host-owned 表示，也不在 Host 与 World 之间复制
完成的数据图；`text_srcs` 保留文本和
来源名。请求的数据文件不存在时，`default: Some(value)` 直接提供已经类型化的
`Value`，不按 `fmt` 再解析；文本文件的默认值仍是 String。`None` 表示缺失即失败，
其他 I/O 错误也始终失败。数据文件只要存在，解析失败就直接报错，不使用默认值。
`vars` 是环境变量快照诉求：不存在的名字从 `SystemResources.vars` 省略，存在但不能
表示为字符串的值会在 Main/initializer 运行前失败。

统一数据源管线严格分成三步：读取物理 source 并注册逻辑 source name；构造 lossless
CST，并在不分配运行时数据对象的前提下完成格式级验证；只有验证全部成功后，才向目标
Heap 直接物化通用 `Value`。格式级验证覆盖所有可能使物化失败的数据条件，包括重复键、
TOML table 冲突、非法数字/时间值，以及不受支持或有歧义的 YAML graph 特性。失败产生
带 source location 的诊断，不产生部分 `Value`。这一层不检查业务 schema：import 和
`data_srcs` 的结果都只是 `Value`，业务数据是否符合某个 struct/enum 属于 codec。

验证阶段必须优先借用 lossless CST，而不是再构造一棵递归 Owned 数据树。实现以一个
扁平 arena 表达 validated plan：节点只保存 source span、已验证的节点种类，以及指向
arena 节点的轻量边；JSON/TOML 节点来自对应 CST node，YAML 节点来自行级 CST span。
字符串、规范化时间、binary 等必须在物化前验证的解码结果，以及重复键检测、YAML
graph 解引用、TOML table 组装所需的索引，进入必要的 side table。plan
不会包含递归 Owned payload graph，也不会分配运行时对象；目标 Heap materializer 在
验证和结构限制预检全部成功后单次消费它。

数据源不消耗也不依赖 VM 的 fuel、stack 或 allocation quota。Host 在完整分配 source
字符串前以有界读取检查原始 `file_size`；验证计划随后独立检查 `nodes`（YAML alias/merge
按最终每次出现计数）、`depth`（根为 1）、单个 `container_size`、单个 `bytes_len`、单个
UTF-8 `string_len`，以及所有 String、对象键、时间字符串和 Bytes 解码后长度之和
`payloads_bytes`。对象键不是节点。全部计数使用 checked arithmetic，任何溢出均视为
超限；只有所有检查通过后才物化。每个节点直接携带本次运行共享 source registry 分配的
`SourceId + range`，后续诊断可以稳定地把该节点作为 source 位置。MainWorld 和 Entry
WorkWorld 仅是不同 target，CST、格式验证、location、data limits 和 `Value` 构造逻辑相同。

Host 在单个异步事件循环中执行 EES effect 并回送 event；reducer 调用始终串行，每次只
注入一个已排队的 `SystemEvent`。`Exit(code)` 是 terminal barrier：Host 完成活动 EES
任务并提交已缓冲 Output 后，才向 CLI 交付退出状态。Host 以结构化任务集合持有异步
EES 调用。terminal、reducer 失败、协议失败或 Host 失败时，Host 取消并 join 全部任务；
这些任务不得脱离所有权树继续运行。

`Output(String)` 是 Entry reducer 的输出效果，不是 Main 返回类型，也不要求 Host
编码 Telora 值。Entry 可以用自己的 `MainType`、codec 和 formatter 生成任意多个
String chunk。CLI 在 terminal effect 前缓冲它们；协议失败不暴露部分输出。
`Exit(Int)` 是 terminal effect，必须位于 effects 尾部。没有内部 Wake 或任意 turn
上限；无队列事件且无活动 EES 调用时判定无进展。每次 reducer 调用仍受普通 VM quota
约束。

`check`、`test`、`query` 和 `lsp` 当前仍是 Host 固定命令路径，尚未通过 run Entry ABI。它们
把目标当作 module。`check` 给出严格 module load/compile verdict，但不等价于一次
`run`：它不选择 application output，也不承诺执行期成功。`query` 和 LSP 可以使用
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

Known 携带有依据的类型事实，Unknown 表示分析依据尚不完整。

Workspace 使用 copy-on-write document snapshot 和单调 revision。每次 rebuild 绑定到
一个 revision 和 cancellation token；被取消或因新编辑而过期的工作不得覆盖较新的
published snapshot。Snapshot 的发布是原子的。

当前 LSP 实现支持增量文档同步、diagnostic、hover、definition、references 和
completion，并协商 UTF-8/UTF-16 等位置编码。Completion 只使用恢复后确有证据的模块
export 和 Struct field。

CLI `query` 和 LSP 可以展示 recovery fact；`check`、`run` 和 Host entry 仍采用严格
成功边界。两者共同展示的事实必须具有同一含义。

## 13. 标准库和 native 边界

标准库的 builtin `std` modules 同时承载普通 Telora definitions 与受信 native
declarations。普通 definitions 实现 Option、Result、argv 等组合政策；native
declarations 提供需要高效 heap 观察或受控 runtime identity 的确定操作。

当前通用能力包括 Array/Dict 组合、String、lexical path、SHA-256、regex、JSON codec
与 schema、typed property、Dyn observer、文本 parse/display 等。
这些 API 不授予环境或文件系统访问权限。例如 path 操作是词法操作，hash 操作只
处理显式输入。

Native 声明的可用性由精确 module identity 决定。只有 Telora 内置的 `std` crate
拥有 native authority；configured application 和 dependency crates 都不能声明 native
symbol。内部 runtime 接口使用 `_` stem，并继续由 resolver 执行可见性检查。

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
- trait object、interface、subtyping、associated type、higher-rank/HKT；
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

特别是，当同一个 Array 没有显式 item expected type，而元素同时包含不同
variant 构造、不同 closure 或匿名 Struct 时，仅凭元素字面量可能
无法主动找到预期的封闭 enum、函数契约或参数化 family。严格模式会报告冲突或未
解决约束，并要求各元素满足共同类型。使用 `.ty!(Ty)` 或 `@[Ty]` 可以提供完整
上下文；领域中的多种数据形态通过显式 enum 定义。

在最小公共边界给 Array 提供具体契约即可把同一个 family 实例下传到匿名元素：

```telora
type Entry(Id, Value) = struct {
    id: Id,
    value: Option(Value),
};

type EntryId = enum {First, Second};
type IntEntry = Entry(EntryId, Int);

let entries: Array(IntEntry) = [
    {id: EntryId.First, value: Some(1)},
    {id: EntryId.Second, value: None},
];
```

该标注提供检查目标，并授权这些字面量在构造点获得 `IntEntry` 的声明身份。若记录需要在数组
之外分别构造，应给完整记录或具名构建函数标注 `IntEntry`。若完整泛型调用仍有
歧义，可以进一步使用 `@[...]` 显式提供无法由值参数唯一确定的类型实参。

### 14.2 Enum payload 的具名类型要求

Enum variant 的 payload 是一个 TypeMetadata 表达式。`struct { ... }` 只允许作为
直接 `type` 初始化器，不是普通 TypeMetadata 表达式，因此匿名 Struct 不能直接写在
payload 位置；应先声明具名 Struct：

```telora
# 非法：struct 初始化器不能嵌入 enum payload
# type Expr = enum {Column(struct {alias: String, column: String})};

type ColumnRef = struct {alias: String, column: String};
type Expr = enum {Column(ColumnRef)};
```

该限制只属于类型声明。构造 tagged value 时，payload 的匿名记录仍会按具名 Struct
契约检查，因此下式合法：

```telora
let expr: Expr = Expr.Column({alias: "orders", column: "id"});
```

### 14.3 Family 与递归类型

递归 concrete TypeMetadata 在普通 definition contract、参数化 family contract 和
模块接口中保持声明 identity 与有限图回边。一个递归 component 会先预留全部声明
身份，再验证并原子封闭；失败的 component 不发布部分类型。契约检查按递归图比较，
并以已经访问的 reference pair 终止，保留有限回边。
因此递归表达式类型可以直接进入函数和 family：

```telora
type Expr = enum {Literal(Value), Call(CallExpr)};
type CallExpr = struct {name: String, args: Array(Expr)};

type Dialect(Context) = struct {
    render: Fn(Context, Expr) -> String,
};
```

同一个递归声明经 whole-module、selective 或 open import 得到的契约保持原声明身份；
另一个同结构声明仍是不同类型。私有 identity 不属于显示名称，也不是源码可引用的值。

Family 可以在直接 Struct/Enum initializer 中用原参数自递归；其余循环 family
component 仍拒绝。Family 也不能调用同模块普通 helper。这里的限制是 family 模板
闭合与求值依赖的限制，不限制 family 字段引用已经封闭的递归 concrete type。

### 14.4 多态 binding 与外围类型参数

Telora 不提供任意 binding 的无限制 let-polymorphism。一个泛型 callable 经普通
alias 绑定后，只在该 alias 初始化时实例化一次；需要在多个类型上使用时，应直接
调用原泛型定义、在调用点使用 `@[...]`，或声明具有完整 `for` 契约的具名定义。

此外，泛型函数 body 内的局部 annotation 当前不能引用由外围模块级 `for` 契约
引入、但未在该局部定义契约中重新量化的类型参数。需要这种 annotation 时，应把
相关计算提升为模块级辅助定义，并在其完整 `for(...) Fn(...) -> ...` 契约中显式
列出所需参数。

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
