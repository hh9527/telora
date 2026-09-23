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
  -> 模块、符号与类型静态求解
  -> codegen、数据注入与顶层值/property 初始化
  -> 有界的纯数据计算
  -> 值、结构化失败或来源化诊断
  -> Host 决定是否发布或解释结果
```

当前语言具有以下基础性质：

1. 模块依赖静态解析，不支持运行时选择模块或 `eval`。
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
BuildState::Ready           # BuildState 的无 payload variant
```

具名 enum 保留自己的 nominal TypeId。variant 的名称和 payload 由 enum 声明定义。

enum variant 首先是类型域身份，`use` / `pub use` 只为身份提供名字。在值表达式中
使用时，variant 如 literal 一样在当前位置物化：无载荷成员产生 enum 值，有载荷
成员产生构造器函数值。`let a = True` 的值来源精确落在 True，不追溯到 prelude；
`let b = a` 则保留 a 的来源。普通 `def disabled: Bool = False` 创建的是值，绑定或重导出
disabled 不会在读取处重新物化。以上规则由 resolve 后的身份决定，不按名字识别。

有载荷构造器被调用后，enum 外层值继承构造器物化位置，payload 保留自己的来源。
例如 `arr |> map\(_, Some)` 的输出外层来源是这里的 Some，内部值来源是各输入元素。
同一封闭签名和 variant 的构造器具有相同身份，来源位置不参与相等；泛型构造器
必须在 MIR 中完成类型求解后才能成为运行时值。LSP 仍可按符号引用导航到声明。

普通字符串支持 `\0`、`\n`、`\r`、`\t`、`\"`、`\\`、两位 ASCII `\xNN`、
Unicode scalar `\u{...}` 和反斜杠后的空白忽略。反斜杠及其后连续的 Unicode
空白（包括空格、Tab 和换行）构成一个独立转义 token，不向字符串值添加字符；
未跟在反斜杠后的普通空白仍是字符串内容。反引号字符串使用同一组
标量转义，但以 `` \` `` 代替 `\"`，并作为结构化连接表达式使用 `\{...}` 嵌入
表达式。raw String 不处理转义或插值；需要保留大量反斜杠或引号时应增加 `#`
delimiter，而不是发明新的 escape。

所有字符串字面量中的实际源码换行（LF、CRLF、单独 CR）统一产生 LF，
包括普通 String、raw String、插值字符串的文本片段以及 Bytes 字面量。
显式 `\r`、`\x0D`、`\u{D}` 仍产生 CR；规范化发生在文本片段解码时，
不对转义或插值产生的值再次替换。反斜杠续行接受上述三种源码换行。
因此源码检出格式不改变多行字符串的值；数据模块仍遵守各自格式的字符串规则。

```telora
let greeting = `hello \{name}`;
```

插值中的每个表达式都要求 `T: std/fmt::Display`，并在编译期降低为已选中
实现的 `display` 成员调用。String、Int、Float 由标准能力提供
显式实现；插值处需要已确定的类型及其 Display 实现，`Dyn` 必须先显式投影。具名 enum
不会因运行时使用 Atom 表示而自动获得 `Display`。String 保持原文本，Int 使用
十进制表示。Float
使用有限 binary64 的文本表示：与 Rust `f64` 的 `{}` 一致，选择能往返到同一
binary64 值的最短十进制文本，不受 locale 影响。该表示保留负零的符号，但不保留
整数值的小数点，例如 `3.0` 表示为 `3`，`-0.0` 表示为 `-0`。输出不保留字面量的
原始小数或指数拼写。

`std/fmt` 定义标准静态展示能力：

```telora
use std::fmt as fmt;

@fmt::display_by("{host}:{port}")
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
已生成的调用胶水执行同一个 closure，共用当前 session 的执行边界；报告宏使用实际
执行处的 rule，输入值保留自己的数据来源。它直接支持
String、Int、Float 以及嵌套的 `DisplayBy` struct；它不动态调用字段类型上的任意
显式 `Display` impl。
`Display::display` 返回 opaque `Fmt`，`fmt::display` 也返回 `Fmt`；显式调用使用
`fmt::render(fmt::Display::display(endpoint))` 得到 String。`fmt::concat(strings, items)`
按 `strings.len == items.len + 1` 组合常量文本和 `Fmt` fragment。插值使用同一
`Display -> Fmt` 路径，在 render 时物化字符串。Fmt 是 Guest 内不可变的格式数据，
不是 Host 对象；其计算受 Wasm fuel 与内存限制。debug repr、codec/JSON 与 Display
分别服务于诊断观察、数据交换和展示，不能互相隐式替代。

Bool 是闭合的内建 enum，其成员 `True` 和 `False` 由 prelude 提供。
这些名称直接确定 Bool 类型，条件、guard 和布尔操作符接受 Bool 值。
条件位置只接受 Bool，不进行 truthiness 转换。

Float 是有限的 IEEE 754 binary64 值。字面量可以写成 `digits.digits`、
`digits exponent` 或 `digits.digits exponent`；exponent 使用 `e` 或 `E`，并可带
`+`/`-` 号。舍入为 `NaN`、`+Inf` 或 `-Inf` 的形式不是合法字面量；语言也不提供
这些值的关键字或特殊拼写。正负零、正规数和次正规数都属于 Float。

## 3. 运行时值

每个进入执行图的值都有 MIR 确定的静态类型：标量、Tuple、Array(T)、Dict(T)、
具名 Struct/Enum、函数、元数据、不透明 native 类型或显式 Dyn。

Bool、Option(T)、Result(T, E) 和 Value 是具有确定身份的 enum。运行时选择哪个
variant，不改变表达式的静态类型。Atom/Tagged 是反射接口的值分类，不是独立的
源码类型；底层表示不授权跨类型比较、隐式转换或按形状赋予名义身份。

### 3.1 集合和积

Array 是有序同质序列：

```telora
let ids: Array(Int) = [1, 2, 3];
let second: Int = ids[1];
```

`array[index]` 对 `Array(A)` 使用零基 `Int` 索引并直接返回 `A`。负数或超出范围的
索引执行等价于 `fail!("OutOfRange", array, index)` 的结构化失败；需要把缺失作为值
处理时使用总操作 `array::get(array, index) -> Option(A)`。

无 expected item type 的 Array 字面量必须得到共同元素类型。`[1, "one"]` 会报错；
位置不同的异质数据可用 Tuple，领域中的不同形态应显式包装为 enum。`Never` 不贡献
可达元素类型。
同一 enum 类型族的成员合并泛型参数证据，例如 `[Ok("hi"), Err(2)]` 的类型为
`Array(Result(String, Int))`。不同类型族或冲突的具体参数会报错；缺少证据的参数
可用 `.ty!(Ty)`、`func@[Ty](...)` 或类型注解补全。
显式 `Array(T)` expected type 会向每个普通元素和 spread operand 下传 `T`。因此当
`T` 是 concrete family 实例时，多个记录构造元素中的 variant 构造、闭包、
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

记录字面量必须由全图上下文确定为具名 Struct 或 `Dict(T)`：

```telora
type User = struct {name: String, active: Bool};
let user: User = {name: "Ada", active: True};
let labels: Dict(String) = {region: "east", tier: "gold"};
```

Struct 的字段集合属于静态结构；`Dict(T)` 的 key 集合可以动态变化，所有 value 具有
同一静态类型。Dict 的无领域顺序观察和序列化采用规范顺序。

字段投影保留静态证据：已知 Struct 必须声明该字段，`Dict(T)` 的字段结果是
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

`std::dict::keys`、`values`、`pairs` 和 `merge` 使用 `Dict(A)` 泛型契约；values 与
pairs 保留元素类型 A，merge 的两个输入共享 A，同名键由右侧覆盖。

#### Struct 合并更新

`base <~ patch` 构造与 base 具有相同具名 Struct 类型的新值，包括其全部泛型实参。
左操作数提供名义身份和完整字段类型，右操作数提供更新字段。右侧可以是另一个
具名 Struct 值，也可以是由左侧约束的更新字面量：

```telora
type State = struct {count: Int, label: String};
type Label = struct {label: String};
def update: Fn(State, Label) -> State = fn(base, label) {
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

Option、Result、Bool 以及用户 enum 的成员与 payload 类型在 MIR 中确定，
静态模式检查据此判断穷尽性；运行时 tag 只标识实际选择的成员。

名称确定 enum 类型族，类型上下文和调用证据补全泛型参数。成员使用
`Event::Progress` 或模块限定的 `events::Event::Progress`；声明本身只绑定类型名。
`use Event::{Progress, Finished as Done};` 选择成员并引入本地名称；
`pub use Event::{Progress, Finished as Done};` 同时提供本地绑定与公开导出。
prelude 提供 Bool、Option 和 Result 的成员。payload 构造器可作为函数传递，
显式泛型特化使用 `Result::Ok@[Int, String]`，包含类型族的全部参数。
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
未完成推断的局部记录构造不会提前冻结为匿名类型；已确定的具名类型身份不能被更换。
函数调用通过共享的泛型类型变量传播同样的构造上下文。例如 `for(T) Fn(T, T)`
和 `for(T) Fn(Array(T), T)` 均可从一个参数获得名义类型证据，为另一个参数中的
字面量提供上下文；回调的返回值也可提供该证据。不同泛型变量不会因为解析后的
类型相同而共享上下文。`std::eq::equal` 遵循这套通用规则，不依赖函数名称或绑定别名。
复合值按结构比较，但两个具名值还必须
具有相同的 canonical TypeId；函数按不透明函数身份比较，而不是比较代码或闭包捕获
内容。enum 契约允许同一静态类型在运行时携带不同 variant，
同一 enum 的不同 variant 返回 False。不同名义类型不能进入普通相等比较；
若两侧是 Dyn，则先比较包内类型身份，不同身份返回 False。`!=` 是 `==` 的精确布尔补集。

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
`{}` 是空记录构造，由上下文确定空 Struct 或 Dict(T) 类型；`do {}` 明确表示 block。

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
if score >= 90 { Grade::Excellent }
else if score >= 60 { Grade::Pass }
else { Grade::Fail }
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
静态和运行时契约的一部分。尾位置的函数调用支持 proper tail call。

模块顶层的每个普通 `def` 都必须有显式完整类型，无论私有还是导出。完整契约不能
包含推导 hole，也不能由 initializer 或调用者补齐；显式绑定的 `for(T)` 参数不属于
hole。配对的 `decl name: Type; def name = ...;` 使用 decl 的签名。仅给 fn 内部参数
加注解不替代绑定签名。导出列表不改变要求，直接重导出使用原声明契约。

所有契约先在同一张 MIR 图中建立，再加入值证据。函数体和调用不满足契约时记录
失败约束，保留契约骨架和健康类型；错误 session 不能 seal 或执行。局部声明仍允许
推导；局部闭包可泛化，递归及相互递归定义在没有显式泛型契约时保持单态。

单个 binding 的 alias 只实例化其右侧泛型一次。它不会自动获得任意 let-polymorphism。
例如局部 `let identity = identity;` 遮蔽模板声明后，后续调用共享同一组类型实参槽；
可以由局部使用补齐，但 seal 前必须确定。需要多个实例时直接引用模板，或声明显式
`for(...)` 契约的模板绑定；不能在已物化的普通 alias 上重新选择类型实参。

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

不存在可独立求值的匿名 Record 类型。记录字面量提供字段构造证据，若整图求解
仍无法确定为具名 Struct 或 `Dict(T)`，静态检查失败。`<~` 右侧的更新字面量和
投影直接向结果 Struct 提供字段，不形成独立的匿名值。
直接 `struct` / `enum` 类型声明具有声明拥有的名义身份；
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
Fn(types::Input, Array(types::Item)) -> types::Output
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
记录构造实参同样在完整调用上下文中检查。若 generic callback 的结果确定了共享
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
普通值 alias 不会产生新的隐式 scheme。跨模块公开的模板声明在每个值域引用处独立
实例化，且其私有 bound identity 不泄漏到导入方。

局部 `let foo = foo;` 的右侧若引用模板声明，只产生一次实例化；左侧绑定的是函数值。
后续作用域中的 foo 引用共享该实例，可以继续为其补齐类型实参，但不能各自选择不同
实例。所有实参必须在可执行 MIR seal 前闭合。真正的 `use` / `pub use` 传递声明身份，
不等同于普通值 alias。

### 6.4 静态 Trait 与约束

Trait 声明定义一个由 provider module 和稳定 local slot 标识的 nominal capability：

```telora
trait Display {
    display: Fn(Self) -> Fmt,
};

impl Display for Endpoint {
    display: fn(value) {
        fmt::concat(
            ["", ":", ""],
            [fmt::from_string(value.host), fmt::from_int(value.port)],
        )
    },
};

impl(T: Property(DisplayBy)) Display for T {
    display: fn(value) {
        let property = type_property::evidence(T.type, DisplayBy.type);
        property.display(dyn::pack(T.type, value))
    },
};

def render: for(T: Display) Fn(T) -> String = fn(value) {
    fmt::render(Display::display(value))
};
```

`Self` 只在 trait member contract 中绑定。`impl(T: Bound) Trait for Target` 的参数
与约束属于该 impl 声明；`for(T) Fn(T) -> T` 中的 `for` 则属于值的多态类型。
Impl 是顶层静态声明，必须完整且精确地
实现 member contract；调用使用 `Trait::member(value)`，不进行 receiver method lookup。
泛型参数可用 `+` 声明多个约束。发布的 rank-1 `TypeScheme` 按 canonical `TraitId`
保留约束，`use`、alias、`pub use` 和 recovery 均复用同一身份。

编译器在泛型实例中记录已选 impl 和成员证据，codegen 生成对应函数调用。
运行时不重新构建类型约束或搜索 impl，
也没有 trait-object value kind。Coherence 拒绝重复的精确 impl 和无优先级的重叠
blanket impl；精确 impl 优先于满足约束的 property blanket impl。orphan boundary
要求 impl module 拥有 trait 或目标最外层 nominal constructor。

`Property(P)` 是内建约束：`T: Property(P)` 根据 provider 的返回类型证明精确的
`Ty(T, P)` typed property 存在，无需先求出 property 值。它可以驱动 blanket impl，
而普通反射查询仍返回 `Option(P)`。静态 evidence 引用待生成的 payload binding；
类型检查通过后才计算 property 值，初始化成功后才能对外发布 session 结果。
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
`std/type_desc` observer 接受 `Type` 值，并通过 kind、children 和 resolve 等操作
暴露规范 descriptor 视图。声明类型以 `TypeDescKind::Ref` 暴露，通过 `resolve` 获得其结构 descriptor；
property 由独立的 property registry 按目标与 property 类型查询。

类型构造及其显式元数据投影：

```telora
Array(String).type
(Fn(Int, Int) -> Int).type
Option(String).type
```

函数类型的 descriptor kind 是 `TypeDescKind::Func`。当前运行时反射把它视为叶节点，
`type-desc.children` 返回空数组；完整签名在 MIR 中存在，但此 API 不公开参数和返回类型。`dyn::kind` 对函数值返回 `ValueKind::Func`；
对装箱的元数据值返回 Type 分类，不能把“函数值”与“描述函数的元数据”混为一谈。`Function` 不具有语言保留意义，可以作为普通领域标识符使用。

`let` / `def` 绑定数据，`type` 绑定类型。类型角色由解析后的声明身份和模块接口决定，
不依赖大小写或运行时 `TypeOf` 内容。`pub use self::{T};` 保留类型身份；公开 `T.type`
得到的普通数据不能再用于静态类型位置。`.type` 是关键字后缀，不是字段查询，不能用于
普通数据，也不触发无关 property provider 求值。

普通函数可以传递、返回和组合元数据，但不能用其结果定义类型或类型 annotation。
`type T = build_type(...)`、`type T = metadata` 和 `type T = Int.type` 都被拒绝。
结构类型由静态求解器解析，整个类型求解过程不持有 VM。
普通 helper 的返回值不能决定类型骨架。Property 是带外信息，不改变此边界。

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
校验。泛型函数体的具体构造目标已在 MIR 实例中确定；运行时只执行对应构造和
校验代码，并保留规则与输入来源。

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
def next: Window = first <~ { offset: 10 };
```

`next` 保留 limit，更新后的整个 Window 再接受校验；`first <~ { limit: 0 }` 会被拒绝。
同时修改互相约束的字段时，应放在同一次更新中，避免要求中间值也满足最终约束。
需要外部授权、关系图或最终排序信息的规则仍由掌握这些信息的函数检查，不能仅凭
局部对象的字段完成。校验器返回 `Err(blame!(...))` 可保留具体字段的输入来源。

装饰器名称也遵循普通 resolve 规则。例如 `use std::type_property as property;`
会遮蔽 prelude 的 `property` 函数；需要同时使用时可显式导入
`use std::prelude as prelude;`，并写 `@prelude::property(...)`。

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
`type Event = enum { Progress(Int), Finished };`，`Event::Progress` 的类型是
`Fn(Int) -> Event`，`Event::Finished` 的类型是 Event。解析到的声明确定成员所属
enum；同形的另一个具名 enum 不改变该身份。泛型成员的契约保留所属类型族的
所有参数，可使用 `Result::Ok@[Int, String]` 显式指定。缺少载荷或结果上下文中
不能确定的泛型参数时，编译器要求补充类型证据。

模块命名空间与选择性导入的类型是不同的绑定。模块别名与导出类型可以同名，
限定链逐级解析，例如 `Model::Model::Data(1)` 中前两级分别表示模块和类型。

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
variant 完全相同，也不是同一个类型；alias、`use` 和 `pub use` 则保留原身份。普通
TypeMetadata 值、codec、`Dyn`、TypeDesc 和工具从同一份权威元数据图工作。

Decorator 只适用于没有类型参数的具名 Struct/Enum 声明及其直接字段/variant。
Property carrier 本身也必须是具名 Struct/Enum，并用内建 capability 标记：

```telora
@property(PropertyTarget::Type)
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

`@property` 接收 `PropertyTarget` 表达式。其签名和类型由 MIR 检查；
表达式的值在初始化执行时计算，不用于反向推导类型骨架。
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
use std::type_property as type_property;
let property: Option(DisplayBy) = type_property::get_type_prop(Endpoint.type, DisplayBy.type);
```

member 查询分别是 `get_field_prop(Owner.type, index, P.type)` 和
`get_variant_prop(Owner.type, index, P.type)`，其中 Owner 和 P 是类型声明。
查询使用已确定的键读取 property 需求结果，不深复制 payload。
在 `T: Property(P)` 约束范围内，编译器证明对应声明存在，生成代码读取同一结果；
约束外的显式查询仍保持上述 `Option(P)` API。

`std::type_desc::fields/variants` 使用相同 canonical index 枚举 member；
`std/dyn::get_field_value/get_variant_index/get_variant_payload` 根据 `Dyn` 携带的权威
descriptor 安全投影。错误 kind、越界和 variant mismatch 是有来源的运行错误；合法
unit variant 的 payload 是 `None`。

Interpreter、静态 trait implementation 和codegen 可以读取同一封闭
骨架和 property registry，并基于这两个稳定输入赋予 property 行为意义。

### 7.2 参数化类型族

参数化 `type` 声明创建可命名的静态类型族：

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

类型族的形参与成员契约由静态 Pass 解析，应用以声明身份和规范类型实参为键。
MIR 先登记 TypeId，再补齐有限的成员图；相同应用复用身份，不求值 Telora helper。

```telora
type Expr(A) = enum {
    Leaf(A),
    Call(Array(Expr(A))),
};
type Swap(A, B) = struct {next: Swap(B, A)};
```

同参自递归和参数置换都可构成有限图。跨声明的参数替换也可以闭合，例如：

```telora
type Left(A) = struct {next: Right(Array(A))};
type Right(B) = struct {next: Left(Int)};
```

这里常量 Int 截断了持续参数增长。相反，`Grow(A) -> Grow(Array(A))` 这样的
增长环会被拒绝。实例展开使用编译期深度/宽度保护，没有固定的实例总数上限。
触限说明本次分析超出边界，不声称已经证明程序无限递归；完整停机性仍不是语言承诺。

### 7.3 递归类型与元数据

递归类型以稳定 TypeId 和有限图回边表示，不构造无限树，也不等待 VM 初始化类型。
纯 alias 环缺少可建立的类型骨架，不能 seal。符号、类型和执行闭包的封闭条件
见 IMPLEMENTATION.md；不完整图可以供 query/LSP 观察，但不能成为执行输入。

TypeDesc 是这些已确定类型的运行时视图。反射可以读取成员和解析名义 Ref，
不能创建新的静态类型或改变已有骨架。遍历类型图的普通函数必须处理循环，
或者借助有限运行时值的结构提供递归进度。

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

`interpreter!(operand)` 在构造时求值 operand 并捕获所得函数；即使适配后的函数未被
调用，operand 的诊断或失败也在构造时发生。每次调用外层工厂产生普通的新闭包，
捕获同一个输入函数；调用内层闭包不会重新求值 operand，也不会修改外层环境。
不缓存 wrapper，不保证相同工厂和 witness 的重复调用产生相等的函数。
同一个已生成函数值的赋值、传递和引用保持普通闭包身份。

类型参数在 MIR 中封闭，codegen 机械生成参数包装；捕获输入函数不会深复制其环境。
需要延迟执行的准备逻辑应显式放在输入函数体内。语义修订见 RFC 0301。

### 7.5 静态约束与 Dyn 投影

以下边界具有不同语义，不能互相替代：

```telora
let empty = [].ty!(Array(Int));
let truth = ty!(True, Bool);

use std::dyn as dyn;
let exact = dyn::project_with(User.type, package); // Option(User)
let exact_sugar = dyn::project@[User](package);
```

`ty!(expr, T)` 与 `expr.ty!(T)` 是零运行时的静态 ascription。`T` 作为 expected type
向 `expr` 内部传递；无法证明时前端报错。它不改变 payload、名义 witness 或来源，
Dyn 中的值需要显式投影为具体类型。


`Dyn` 精确投影比较打包 descriptor 与目标的 canonical type identity，不走结构比较或
assignability。`dyn::project` 是标准库中的普通泛型函数；`project@[T](package)`
按其 for(A) 签名实例化，函数体调用 `project_with(A.type, package)`。它不依赖
namespace 拼写魔法，重命名导入遵循普通名称绑定和泛型实例化规则。

对具体的具名字段 struct 和单载荷 newtype，内置 `std::dyn::FromDynFields` 提供
从 `Array(Dyn)` 受检查地构造 `Self` 的能力。`dyn::construct@[T](items)`
通过该 trait 调用：先检查输入数量，再按 `type_desc::fields(T.type)` 的规范索引
逐项核对 Dyn 的精确类型身份；newtype 固定只有索引 0 的载荷。具名 struct 的
索引按字段名排序，不是源码声明顺序。数量错误返回 `StructFieldError::Count`，
类型错误返回携带索引、预期类型和实际类型的 `StructFieldError::Field`。
全部通过后才构造 `T`，并执行普通构造的 `@check`；检查失败遵循原有诊断语义。
enum、非名义结构和未封闭类型不能获得该 trait 证据，用户不能覆盖内置实现。
该位置数组是内部结构接口，不作为跨版本的外部数据格式。

## 8. 模块和封闭世界

模块通过静态路径组成不可变有向图。每个 crate 以 `src/lib.telora` 为根；`mod` 和
`data` 创建图边，`use` 只在已有图上建立静态名字绑定。全部依赖都在初始化前封闭，
程序不能根据运行时值决定模块依赖。

```telora
pub mod model;
mod validation;
pub data defaults = import(json) "defaults.json";

use std::array as array;
use crate::model::{User, make_user};
use self::validation::{validate};
```

`mod child;` 相对于声明模块的逻辑目录挂载 `child.telora`；`pub mod child;` 还将该
模块加入父模块公开接口。`use` 不读取文件，也不能使未挂载文件进入模块图。它支持
namespace、选择性绑定和 alias；所有形式观察同一个 public scheme，不能因拼写不同
而丢失泛型关系。

默认 prelude 是编译器提供的 fallback binding，不形成源码模块边，也不是不可遮蔽的
词法外层。本地 `let`、`def`、`type` 等 binding 正常遮蔽同名 prelude 项。若仍需访问
被遮蔽项，应显式绑定 alias：

```telora
use std::prelude::{ PropertyAttr as BuiltinPropertyAttr };
```

声明默认私有。`pub` 将顶层声明加入模块公开接口；`pub use` 在当前模块建立 alias，
并以同一个身份重新公开它：

```telora
use crate::types::{Plan as LocalPlan};
pub use self::{LocalPlan as Plan};
```

`pub` 只改变静态可见性，不执行用户代码。`pub use` 保留原 value identity、精确
TypeScheme、concrete/recursive TypeMetadata graph、type-family template、opaque
provider identity 和 provenance；它不包装、重求值或重建 binding。公开的 namespace
仍是语义 Module，不能退化为 Dict。

Module 顶层是声明空间，不是普通 expression block。顶层值只使用 `def`；`let` 只允许
出现在函数或 `do` 等普通 block 中，用于顺序求值、词法局部和 shadow。复杂模块值的
初始化把局部步骤封装在 initializer expression 中：

```telora
def answer: Int = do {
    let base = 40;
    base + 2
};
```

模块顶层不接受 `let`、裸表达式或 final expression。`pub` 不适用于局部 binding、
`let` 或 `impl`。

Telora crate 使用 `src/` 和 `tests/`。Host 从当前工作目录向上查找最近的
`telora-config.json`，并从 workspace 的 `telora-crate.json` 得到稳定 crate 身份、
直接依赖和 crate root。manifest 和 lock 都不列举模块。普通模块从 `src/lib.telora`
按需发现；工具对公开的名义类型和值进行验收。测试由 Host 以 `@test/<name>` 选择，
`telora test <name>` 使用同一身份。

逻辑 ID 到 crate 内物理位置的映射为：

```text
@src/lib        -> <crate>/src/lib.telora          -> my-crate
my-crate/x      -> <crate>/src/x.telora            -> my-crate/x
my-crate/a/x    -> <crate>/src/a/x.telora          -> my-crate/a/x
@test/x         -> <crate>/tests/x.telora          -> my-crate/tests/x
dependency/x    -> <dependency-crate>/src/x.telora -> dependency/x
```

Host 选择测试根时，先为当前 crate 的 `tests/` 建立受限访问边界，再从选中根发现
可达图。测试根可以位于子目录；顶层测试和辅助模块可通过 `@test/`、
`<crate>/tests/` 或测试内相对身份相互引用，并通过 crate 的公开模块树访问源码。
源码模块与其他 crate 不能访问测试模块；依赖只暴露由其根模块公开的模块树。

测试边界包含 Telora 和受支持的数据文件，不写入 manifest 或 lock，不扩展普通
production module tree。枚举顺序确定，符号链接（包括 tests 根）被拒绝，其他文件类型被
忽略。若 `tests/x.telora` 与已声明的 `src/tests/x.telora` 产生相同 canonical name，
测试准备阶段拒绝该冲突。边界成员不自动成为执行根，未引用文件不解析、不求值；
枚举或路径校验失败仍属于准备错误。固定访问边界不意味着 Host 提供原子文件快照。

静态路径首段选择当前 crate、直接依赖或内置 `std`。`crate`、`self` 和 `super` 只在
已挂载模块树内导航，不访问任意物理路径。上述解析均在模块初始化前完成。

模块图不允许初始化 cycle。递归函数和递归 TypeMetadata 在单个模块及
已建立的模块接口内由各自机制处理，不把 module cycle 当作递归定义机制。

Production module 使用显式命名导出：

```telora
pub def version: Int = 1;
pub def compile: Fn(Input) -> Option(Output) = fn(input) { ... };
pub use self::{ User, compile };
```

模块没有由“最后一个表达式”定义的公共默认结果。Source module 的公共接口由 `pub`
声明及 `pub use` 定义，Host 再按所选协议校验或选择其中的值。

### 8.1 模块身份和依赖

模块的语义身份由 crate 和逻辑路径组成的 canonical cname 决定，不等于偶然的物理绝对路径。
相同模块经不同相对边解析时应复用同一身份；resolver 拒绝词法或 symlink 越界。

workspace 根的 `telora-config.json` 为每个 crate name 选择唯一的 workspace member
或远程 tarball source，并可为远程 source 设置 workspace 内的开发 override。每个
crate 的 `telora-crate.json` 声明 canonical name 和直接依赖名称。
`telora-lock.json` 固定 workspace 唯一精确包图。
crate 名与逻辑路径共同形成 canonical cname，物理 workspace 与缓存路径不进入身份。

`telora lock` 是唯一创建或改写 lock 的命令。其他 crate-mode 命令和 LSP 校验 lock，
通过私有 Host 中的 IMOS store 物化缺失的远程
tarball，再把准备好的 immutable crate-root map 交给 resolver。resolver、module loader、
求值器和 VM 均不执行网络 I/O，也不改写 lock。

每个 crate 的 `src/lib.telora` 是固定模块根。resolver 只沿已解析的 `mod` / `data`
声明按需发现后继；`use` 不产生发现边。未挂载文件不属于程序，也不写入 manifest 或
lock。`tests/**` 属于测试调用的独立访问边界。

resolver 按顺序查询 vendor：builtin vendor 在先，当前发布 `std/*`；prepared
workspace 建立的 configured vendor 在后。vendor 以 crate 为选择颗粒；一旦
builtin vendor 提供 `std`，全部 `std/*` selector 都只在该 crate 内解析，后序同名 crate
整体被遮蔽，不能补充或覆盖 module。package graph 在模块图发现前完成；注册采用 first-win，
当前 crate 先于 dependencies，已登记的 crate name 及其 source 指向此后不可改变。
当前没有 registry、语义版本求解或同名多版本能力；远程 source 是确定的 HTTP(S)
tarball URL。

resolver 配置只来自 prepared workspace。运行时 Context 和服务输入契约由
实现 TransformService 的 MainService 类型导出表达。Telora 源文件没有独立的 `option` 声明；`option`
可以作为普通 identifier 使用。

Telora 的模块 cname 不含 `.telora`；resolver 在物理读取时映射到源码文件。模块是否
公开只由 `pub mod` 决定，不由文件名约定决定。内置标准库也遵循同一规则：
`std/lib.telora` 是根，其 `pub mod` 声明定义公开模块树。Rust 侧 embedded source 表只
负责把源码字节打包进 binary，不定义语义可达性。只有内置 `std` crate 可以声明
native symbol。

### 8.2 静态数据模块

JSON、TOML 和 YAML 与 Telora 文件共同进入模块图。它们不是运行时文件读取：Host
在静态阶段仅登记接口，类型闭合后才读取内容并交给 Guest 解析。每个静态数据文件都是只导出 `data: Value` 的
Module，不是一个 raw root：

```telora
data request = import(json) "./request.json";
data policy = import(yaml) "./policy.yaml";
data config = import(toml) "./config.toml";
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

`ScalarValue::None`、`ScalarValue::Bool(True)`、数值和字符串成员分别编码为 JSON null、boolean、number 和 string。
参数化查询使用 `Array(ScalarValue)` 表达 bindings；数据库 component 再把逻辑标量适配为
后端的物理参数类型。

每个递归子节点都携带同一个 canonical `Value` TypeId，因此可以用闭合 `match`
穷尽处理。它是规范化后的语义数据，不是 lossless AST：不保留注释、
原始标量拼写或 table 拼写，也不暴露 VM 的 meta/runtime layout。

当前格式行为包括：

- JSON 严格解析数字、字符串和重复 key；Int 越界或非有限 Float 失败；
- TOML 支持 1.0 的核心值与表结构，四种 date/time 类别规范化为独立 Value variant；
- YAML 使用固定的保守 schema：mapping key 必须是 String，拒绝 custom tag 和非有限
  Float；标准 `!!binary` 经过 canonical base64 校验后成为 Bytes；不支持anchor（`&name`）、
  alias（`*name`）和merge，遇到相关语法直接报错。未加引号的`<<` mapping key被拒绝，
  加引号的`"<<"`是普通字符串key；字符串或注释中的`&`、`*`、`<<`不作为引用或merge。
  旧式隐式 bool 和时间戳按 String 处理。

运行时文本解析使用同一边界：

```telora
json::parse(text)  # Result(Value, codec::BlameError)
yaml::parse(text)  # Result(Value, codec::BlameError)
toml::parse(text)  # Result(Value, codec::BlameError)
```

Typed model 与 Value 之间只通过 codec 重建数据图：

```telora
let model = codec.decode@[Model](request).unwrap!();
let value = codec.encode(model);
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

初始化消费已封闭的执行图；顶层值和 property 通过需求表处理依赖，成功后保留
初始化对象作为请求基线。类型骨架已由 MIR 确定，不依赖初始化是否成功。

check 可在内部保留成功和失败的独立需求结果；失败条目不是语言值。任何初始化
错误都阻止本次结果对外发布。正常服务请求从同一基线开始，结束后丢弃请求对象；
发生陷阱时重建实例并恢复内存基线。这里不涉及持久化 snapshot。

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

解码使用不可观察的 native `codec::BlameError`，保存消息和失败值的来源。
`codec.decode` 与 `json.decode` 返回 `Result(A, BlameError)`，失败本身不产生诊断。
错误保留当前解码 Value，调用方可以继续试探，或显式使用其来源产生诊断：

```telora
match codec.decode@[User](raw) {
    Ok(user) => user,
    Err(error) => raise!(error),
}
```

`string::parse` 返回 `Result(A, string::ParseError)`，错误的 value 为原始 String。
`dyn::field/fields/array_items/tuple_items/tag/payload` 返回带 `dyn::AccessError` 的
Result，错误的 value 为原始 Dyn。`type-desc.resolve` 使用 `ResolveError`，value
为输入 Type。这些错误都具有 message 字段，并直接保留原输入的来源。

被选中的 Entry 可以通过 `std/_rt` 将一次普通函数调用放入独立诊断
作用域：

```telora
rt.with_diagnostics:
    for(A, R) Fn(Fn(A) -> R)
        -> Fn(A) -> Result(Tuple([R, Array(rt::Diagnostic)]), Array(rt::Diagnostic))
```

`rt::Diagnostic` 是类型化快照，包含 `severity`、`message`、`labels` 和 `notes`。
每个标签包含 `location`、`message` 和 `primary`；位置记录源码名称以及起止位置。
`start/end` 均为 `SourcePoint { line: Int, offset: Int }`，每个分量的有效范围是 u32。
行号和行内 UTF-8 字节偏移从 0 开始，范围为 `[start,end)`，并非文件绝对字节偏移。
两个分量分别序列化为整数，不通过 JavaScript Number 传递打包的 u64。
CRLF、LF、单独 CR 都计作一次换行；面向用户显示时再将行号转换为从 1 开始。
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
失败不能改变 Telora 的值、失败、诊断、控制流或执行终止边界。
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

失败传播保留原始 rule、data sources 及其顺序，不生成替代诊断。

`check` 将多个导出项及所需 property 初始化作为多根求值。共享依赖只计算一次；
一个根失败后，可以继续其他根。读取已失败的依赖会传播同一个根因，不重复报告。
每个根内部采用顺序失败传播：函数调用、let initializer、block、闭包、容器构造或
逐项操作失败后，立即退出当前调用，不执行后续语句或 callback。
容器不保存可供后续计算使用的失败子节点，也不提供“健康投影”。

静态求解独立收集 Unresolved、Conflicted 和 Unknown 等诊断；类型未闭合不进入求值。
运行阶段不提供 best-effort。run（含服务模式）和 eval 初始化失败即停止，不启动入口。
serve 已有的单请求失败恢复属于显式请求边界，不改变普通函数的顺序中断规则。
资源耗尽等终止性错误中止整个 session，不尝试剩余根。

任何未消费的 error 都阻止本轮 session 成功发布，即使其他根能够算出健康结果。
为收集诊断继续初始化不代表可以发布部分结果；失败传播不制造派生类型错误。

### 9.5 失败类别

VM 区分可恢复的程序失败与终止整个 evaluation session 的资源/一致性失败。前者包括
类型不匹配、missing field、non-exhaustive dynamic match、panic 和 `fail!`；后者
包括取消、资源耗尽以及执行引擎 trap。

check 可以在一个初始化根失败后继续其他根，但不会把 failure 当作成功值。

## 10. 求值和资源语义

当前工具链使用 lossless CST、扁平 HIR/MIR、全图类型求解、Wasm 和 Wasmi 实现
语言。各层共同服从本文定义的可观察语义和来源映射，不建立彼此竞争的语言模型。
各阶段、运行时布局、模块骨架与初始化/请求生命周期的当前实现见
[`IMPLEMENTATION.md`](IMPLEMENTATION.md)；这些私有结构不是第二套语言语义。

### 10.1 一个静态阶段，两个执行阶段

模块、符号和类型求解是纯静态阶段，不执行语言代码。Tool stage 在静态闭合后执行
property provider 和顶层初始化；Program stage 使用显式 Host 输入执行普通应用函数。
两个执行阶段共用：

- Wasm 与函数调用规则；
- 不可变值和 heap 表示；
- 执行资源约束；
- runtime failure 与来源规则。

静态类型标注不在运行期重新检查。编译器始终保留执行所需类型和布局；
显式使用 T.type 时还会产生可供普通函数读取的元数据值。

`codec.decode@[Target](value)` 由静态类型参数选择 `Decode` 实现，解码返回
`Result(Target, codec::BlameError)`。`codec::encode(model)` 从 model 在 MIR 中确定的
具体类型选择 `Encode` 实现并直接返回 `Value`；失败产生诊断，并保留失败值和编码规则的
来源位置。Dyn 中的模型需先投影到具体类型。
复杂 concrete family 的定义模块应拥有一次完整实例化，并导出 concrete
alias 或 typed boundary function：

```telora
type Rejection = RejectionPayload(Entity, Dimension, Intent, Expr, Plan, Sql);

def encode_rejection: Fn(Rejection) -> Value = fn(value) {
    codec.encode(value)
};

pub use self::{ Rejection, encode_rejection };
```

下游调用 `encode_rejection(value)`，不重复 family 实参；函数契约仍严格检查 value，
其 canonical witness 随值跨模块传播。该模块/API 方案适用于包含封闭递归参数的
family，不引入隐式反射或新的表面语法。

### 10.2 时间检查与配额

资源边界的目的，是让持续循环、持续分配或无法取得进展的执行能够被终止，
不是精确核算成本或严格约束实际资源占用。阈值可以宽松，不构成 CPU 时间、
进程 RSS 或跨版本可消费操作数量的承诺。

Fuel 用于对抗执行能否收敛的不确定性，而不是对执行成本精确计费。这是既有的
求值语义（RFC 0010）：不要求逐指令、逐元素或逐字节核算，也不要求按 native
算法的内部操作次数扣减。指令融合、数据复制方式或库内部实现的变化，不应成为
重新定义 fuel 计费规则的理由。

初始化与单次服务请求分别采用 fuel 预算：`runtime.initializationFuel` 和
`runtime.requestFuel`，配置单位为百万 fuel，默认分别为 5000 和 1000。
`--initialization-fuel`、`--request-fuel` 以相同单位直接覆盖配置值；构建制品
保存覆盖后的预算。独立的 `telora-run` 可用同名参数覆盖制品内的预算。初始化与请求
各自获得独立预算，请求之间也独立。fuel 不换算为时间，也不承诺跨编译器版本
的精确计费。惰性 Wasmi 字节码翻译/验证不扣 Guest fuel，避免缓存状态改变
可用的执行预算。

同时使用 Wasm 引擎的内存增长和调用栈限制，不另建 Telora 操作、逻辑分配
或跨边界调用的统一 account。编译到 Wasm 的 Rust RT 与语言代码同受
引擎限制，超限终止当前执行，不能作为可恢复语言失败被吞掉。

Host 的解析、fixture 展开和外部 IO 不由 Wasm fuel 覆盖，仍需简单的输入规模、
结构深度、有限展开或超时/取消边界。Guest fuel 检查不能让任意 Host 调用
自动及时返回。验收验证持续循环与持续分配能被拦住，不验证精确扣费次数。

### 10.3 确定性

在相同源码、解析依赖、显式 Host 输入、native module 实现和预算下，执行结果应可
重放。无领域顺序的 Dict、模块身份、诊断、类型合并和输出使用稳定顺序。

确定性不意味着所有程序终止，也不保证不同编译器版本产生字节级相同的内部表示。
它要求当前语义版本内，内部 cache 命中、heap 地址、物理路径别名和无意义遍历顺序
不能改变可观察结果。

## 11. Host 边界

Telora 只进行数据求解，业务 Host 负责外部输入输出。模块发现和类型求解在 VM 之外完成，
内置 entry 与业务代码在同一 MIR 中静态封闭。

### 11.1 当前 CLI Host

当前 `telora` 二进制提供：

```text
telora [-C <context>] check <module>
telora [-C <context>] test <name>
telora [-C <context>] eval <module:export>
telora [-C <context>] build <module> -o <file.wasm>
telora [-C <context>] run <module> [--source <name>=<source>]... [--serve <URI>]
telora [-C <context>] query|q modules [-p <substring>]
telora [-C <context>] query|q exports <module> [-p <substring>]
telora [-C <context>] query|q at <module>[:<line>[:<column>]] [-p <substring>] [-k type,let,def,use]
telora [-C <context>] lock
telora lsp
```

Clap 拥有 help、version 和命令行参数校验，其输出是面向人的文本。命令通过参数校验后，
Telora Host 在 stdout 和 stderr 上只产生 JSON 或 JSONL。成功的 `eval` 输出一个 JSON
Value；`dbg!` 和命令错误在 stderr 输出 JSONL record。`check`、`test`、`query` 和 `run`
使用相应传输的响应协议，`lock` 输出生成的 lock path JSON String。进程退出码
独立表达命令是否成功。

`eval MODULE:NAME` 要求导出 Value。`run MODULE` 选择具体类型 MainService。
-C 指定 manifest discovery 的起点。服务契约见下文 TransformService 入口。

`test <name>` 选择 `tests/<name>.telora`，名称必须是无后缀的规范相对路径；不接受
绝对路径、parent traversal 或 export selector。当前只支持显式选择一个 Telora 根，
private 文件不能作为根。它先使用既有诊断恢复流程检查和初始化完整可达图；有错误时
不执行用例。随后按 UTF-8 公开名称执行入口直接公开的精确 `std::test::Test`，
普通 `use` 不执行依赖模块的 Test，显式 `pub use` 按公开别名执行，容器和 Dyn 不参与发现。
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
此命令不调度服务入口，不改变循环初始化
的拒绝规则。`check @test/...` 与显式测试 query 使用相同清单和模块身份。

其他命令的 `<module>` 是 `@src/...`、`@test/...`、依赖模块 ID，或
公开 `std/...` 模块 ID，不是物理文件名。`query exports std/string` 与
源码中的 `std::string` 静态路径选择同一个内置模块身份；不存在的 `std/...` 得到
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
`def`、`use`；坐标化的 `at` 不接受 `-p` 或 `-k`。空匹配成功且不输出记录。
`line` 从 1 开始，`column` 从 0 开始并按 UTF-8 byte 计数；column 必须落在 Unicode
scalar 边界。JSONL 中所有 `line/end_line` 同样从 1 开始，`column/end_column` 从 0
开始并固定为 UTF-8 byte offset，range 使用半开区间。LSP 仍按客户端协商的
UTF-8/UTF-16/UTF-32 encoding 输出其协议规定的 0-based position；终端诊断继续使用
面向人的字符位置。每条 query 记录显式区分 `authoritative`、`recovery` 或 `debug` 权威层级；
表达式级记录属于 `debug`，错误恢复所得记录的权威层级服从对应事实状态，而不是模块级
“完整性”状态。
Namespace `use` 的 definition record 以 `target` 给出目标模块的稳定 ID，并省略
普通值的 `type` 字段；其成员的精确公开 type/scheme 由 `query exports <target>`
记录定义。Selective `use` 仍在本地 definition record 中直接携带所选成员的精确
type/scheme。Namespace 保留模块接口中每个导出项的精确契约。

`check` 先完成静态求解，再对多个初始化根采用 best-effort。独立初始化根可以在失败后继续，以收集
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

静态 `diagnostic` record 的 `module` 是 primary 来源所属的规范模块 ID；
`session` 保留本次查询或检查的根选择（例如 `--lib`）。批量检查不会把同一条
错误复制给每个导入者。没有可归属的源码 primary 时，`module` 回退为本次根选择。
这一静态归属不代表运行时失败的触发导出项；规则位置与触发求值的根是不同信息。
显式契约失配时，secondary 的 `type contract declared here` 指向提供期望类型的
原始注解。该来源随具体使用关系保存；导入和重导出沿用原声明位置，成功的类型槽
合并不会把错误调用的来源改成某个无关的健康调用。延迟产生类型的表达式也保留
先前接收的契约来源。

初始化执行期间，Wasm 为诊断事件记录当前调度根的稳定身份。`check` 的对应 record
带有 `initialization: {node, module, symbol, name}`；具名全局值包含 symbol/name，
property 等其他需求根用 node/module 标识且 symbol/name 为 null。ID 仅在本次封闭图内
有意义。共享函数中的规则位置、输入值的来源和触发根分别表达；不能由 primary、
数据来源或闭包推导触发者。依赖者读取缓存失败时保留原事件，不重复报告或
改写根身份。初始化之外的运行时诊断不携带初始化根。

`query` 的 definition/export/reference/expression record 中，`state` 表达类型或解析
结果，`failed_constraints` 列出该语法子树产生的失败类型约束 ID（仅在本次图内有效），
可与静态 diagnostic record 的 `constraint_ids` 对应。
例如 `def value: Int = "wrong"` 可以保留 `Known`、`Int`，同时带有失败约束。
引用不继承被引用声明的所有失败；导出记录查询其原声明。空列表不代表程序或声明合法，
仍须结合 Unknown、resolve 结果、缺失契约及其他诊断判断，不能据此绕过 seal。

`query` 查询同一普通 Module 管线产生的全面证据图，包括 recoverable CST、部分语义事实和
诊断求值结果，因此存在错误或求值失败时仍可返回不受影响的事实。`query` 成功只表示查询
成功，不表示模块健康；恢复节点通过独立 fact state 表达未确定或失败的状态。

运行命令不接受 `--best-effort`。需要多根初始化诊断时使用 `check`；只检查静态事实时
使用 `check --only-types`。运行命令不为诊断而额外预执行用户代码。

### 服务传输与制品

`telora run MODULE --serve URI` 与 `telora-run app.wasm --serve URI` 共用传输层：
`stdio+jsonl://`、`http://IP:PORT`、`http+unix:///absolute/path.sock`。
HTTP 使用集合字段声明的 `@http::get/post` 路由，响应为 `telora.service/v1` envelope；
语言错误及资源陷阱仍通过 `error` 和诊断表达（HTTP 200）。
请求之间恢复初始化基线，执行串行。Unix socket 不会覆盖既有路径，
停机后由部署方清理。不传 `--serve` 时执行单次请求。

`telora build MODULE -o FILE` 在输入端归一化源码与静态数据的 EOL 为 LF，
编译并原子发布普通 Wasm，不执行初始化。`telora build MODULE --snapshot
--source NAME=FILE -o FILE` 还会初始化、压缩 service，并把状态嵌入
`telora.snapshot` custom section；原始初始化代码与数据 bundle 继续保留。
`telora-run FILE` 使用 wasmi，不依赖源码与编译器：无显式 source 时优先恢复快照，
提供 source 时忽略快照并重新初始化。制品格式为实验版本；不提供 Wasmtime 后端。

### TransformService 入口

服务入口是模块公开的 `@service::collection` MainService struct；每个字段的具体类型实现
`std::transform_service::TransformService`：

```telora
trait TransformService {
    init: Fn(Context) -> Self,
    transform: Fn(Self, Value) -> Value,
};
```

字段使用 `@service::slot("name")` 绑定静态适配器和请求方法，
可附加 `@http::get/post("/path")` 声明 HTTP 路由。Context 为 {sources: Dict(Value)}。
字段服务的 `@service::source("name")` 归并为稳定来源清单；Host 来源必须与清单一致。
所有类型参数和方法实例都在 MIR 中确定。内置 entry 包装调用，无运行时 trait 派发。

模块顶层值与 property 初始化完成后，Host 准备来源并初始化各字段；随后按路由调用 transform。
来源只读取一次，服务间隙 reset 到初始化后的确定基线。transform 不产生跨请求状态更新。
init 失败不发布实例。内置 with_diagnostics 捕获普通语言 failure 并保留完整诊断；
fuel/memory 耗尽由执行器结束当前请求，下一个请求仍从同一基线获得独立预算。
配额用于可停机，不是精确计费，reset 和页级计量方式不构成语言契约。

run 从 stdin 读取一个 `{method: String, input: Value}` JSON，成功输出一个 JSON Value；`run --serve stdio+jsonl://` 读取 JSONL，
每条输入对应 {ok, error, diagnostics} 响应，按输入顺序处理。diagnostics 包含 severity、
message、labels、notes；已捕获诊断不重复输出。初始化 source 不接受 stdin；JSONL 和单次 run 使用 stdin 作为请求通道，HTTP 使用请求体。--source name=path.json 或 file+FORMAT://path 使用已有格式验证和来源管线。
初始化来源使用 @service/name；逐次请求输入不注册规范来源路径，物理路径不进入来源身份。

服务不获得环境、进程、网络或任意文件能力；需要的业务输入由 Host 显式转成 Value。
旧 eval-with、entry.Eval/Run/Serve、应用 EES 与 reducer 协议均已删除。
包管理的 IMOS 能力只在私有 Host 中使用。详细用法见 [执行模式](../../guide/EXEC-MODE.md)。

数据模块、--source 和 fixture 的文件读取由 Host 负责；JSON/YAML/TOML 解析和
语言 Value 构造在 Guest 内完成。源码类型求解只使用数据模块的固定接口，不读取
数据内容。普通字符串 parse 同样使用共享数据解析器，但保留输入字符串的来源。

解析分为 parse-0 和 parse-1：前者建立 span 结构并检查资源边界，后者完成必要解码、
数值校验、重复键检测和数据树整理。原文可借用时只保存范围，需要转换的文本集中
保存。全部验证成功后才物化 Value；资源超限立即停止，其他可恢复的数据错误可以
收集多条诊断。实现不构造递归 Owned AST。

Host 限量读取原始文件；Guest 的解析和物化同时受 Wasm fuel、线性内存与 DataLimits
约束。DataLimits 包括原文大小、节点数、结构深度、容器大小、单值字符串/字节长度
与累计解码载荷。具体计量和两阶段布局见 IMPLEMENTATION.md。
文件来源登记名称和 BOLs，节点保存 src/start/end；临时字符串解析沿用输入来源，
附加内容内字节范围，不为每次 parse 建立新的行索引。



每次服务请求在独立执行边界内处理，结果发布后或请求失败后 reset。

`check`、`test`、`query` 和 `lsp` 当前仍是 Host 固定命令路径，尚未通过 run Entry ABI。它们
把目标当作 module。`check` 给出严格 module load/compile verdict，但不等价于一次
`run`：它不选择 application output，也不承诺执行期成功。`query` 和 LSP 可以使用
recovery snapshot 展示仍有证据的语义事实。

`telora build` 只编译 Wasm，不是领域 build plan 执行器；CLI 不提供领域专用 exec adapter。Exec plan、build plan、SQL plan 等
都是普通应用值；是否解释它们以及是否产生现实效果，由显式的外部 Host 决定。

## 12. Workspace 和语言工具

严格检查与编辑器分析共享 parser、HIR、模块/符号/类型 Pass 和 MIR 查询。
query/LSP 不创建 VM，不求值 property 或顶层导出；可以保留并展示错误周围仍有
依据的静态事实，不能把分析不完整伪装成成功执行。

符号结果区分 Bound、Unresolved、Conflicted，以及交由类型阶段处理的成员约束。
类型槽区分 Unknown、ProxyTo、Structure、Known、Conflicted；完成求解后执行
要求所需槽具有归一化 TypeId。诊断保留冲突证据，Unknown 不等于 Dyn。
内部槽状态与 CLI/LSP 呈现格式是不同层次，工具不从运行时失败补猜静态类型。

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
、typed property、Dyn observer、文本 parse/display 等。
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
- runtime code generation、`eval`、动态模块装载或通用 macro system；
- trait object、interface、subtyping、associated type、higher-rank/HKT；
- 任意 binding 的无限制 polymorphic generalization；
- 全局 termination proof；
- 通用 package registry、获取和版本求解；
- 生产级领域 effect executor；
- 对外部生态或未来版本的长期兼容性保证。

此外，下列能力具有明确的当前限制：

- `interpreter!` 只提升直接 A 参数的消费型解释器，不能适配高阶或返回 A 的位置；
- 反射只观察封闭类型，不能在运行时产生新的泛型实例；Func 描述符当前不公开子类型；
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
variant 构造、不同 closure 或记录构造时，仅凭元素字面量可能
无法主动找到预期的封闭 enum、函数契约或参数化 family。无法闭合时报告冲突或未
解决约束，并要求各元素满足共同类型。使用 `.ty!(Ty)` 或 `@[Ty]` 可以提供完整
上下文；领域中的多种数据形态通过显式 enum 定义。

在最小公共边界给 Array 提供具体契约即可把同一个 family 实例下传到记录元素：

```telora
type Entry(Id, Value) = struct {
    id: Id,
    value: Option(Value),
};

type EntryId = enum {First, Second};
type IntEntry = Entry(EntryId, Int);

let entries: Array(IntEntry) = [
    {id: EntryId::First, value: Some(1)},
    {id: EntryId::Second, value: None},
];
```

该标注提供检查目标，并授权这些字面量在构造点获得 `IntEntry` 的声明身份。若记录需要在数组
之外分别构造，应给完整记录或具名构建函数标注 `IntEntry`。若完整泛型调用仍有
歧义，可以进一步使用 `@[...]` 显式提供无法由值参数唯一确定的类型实参。

### 14.2 Enum payload 的具名类型要求

Enum variant 的 payload 是静态类型表达式。`struct { ... }` 只允许作为
直接 `type` 初始化器，不是可嵌入的类型表达式，因此匿名 Struct 不能直接写在
payload 位置；应先声明具名 Struct：

```telora
# 非法：struct 初始化器不能嵌入 enum payload
# type Expr = enum {Column(struct {alias: String, column: String})};

type ColumnRef = struct {alias: String, column: String};
type Expr = enum {Column(ColumnRef)};
```

该限制只属于类型声明。构造 tagged value 时，payload 的记录字面量仍会按具名 Struct
契约检查，因此下式合法：

```telora
let expr: Expr = Expr::Column({alias: "orders", column: "id"});
```

### 14.3 Family 与递归类型

递归类型保持声明身份和有限回边，可以用于函数契约、类型族与模块接口。
同一声明经不同导入方式仍是同一类型，另一个同结构声明是不同类型。
具体递归和参数增长边界见第 7.2、7.3 节；普通函数不能计算静态类型。

### 14.4 泛型声明与函数值

具有完整 for 契约的声明可以保留绑定类型参数并接受静态检查。每处模板引用独立
取得实例化证据；进入 SealedExecutable 的函数值必须具有完整具体签名和实例 ID。
普通局部别名仅实例化一次，后续调用不能把同一个值重新实例化为另一种类型。

泛型函数体可以使用所属契约绑定的类型参数，包括局部标注：
`def identity: for(T) Fn(T) -> T = fn(value) { let copy: T = value; copy };`
类型参数不是 Unknown；执行时使用 MIR 已封闭的具体实例。

## 15. 规范性总结

一个符合当前 Telora 模型的程序，应能被理解为以下组合：

1. 静态解析的 source/module graph；
2. 对不可变小型值模型的普通函数计算；
3. MIR 静态求解的类型骨架，以及普通代码可读取的类型元数据与 property；
4. 明确受限的 `Dyn`、诊断和 Host bridge；
5. 具有执行终止边界、成功后原子发布的一次执行；
6. 最终仍需 Host 赋予意义的普通输出值。

Telora 的核心承诺不是“所有错误都能静态发现”，也不是“所有程序都会终止”。它的
承诺是：可影响一次执行的世界是明确的；执行在有限边界内产生值或结构化失败；类型、
来源和诊断尽量共享一个权威语义模型；任何现实效果都位于普通程序之外的 Host 授权
边界。
