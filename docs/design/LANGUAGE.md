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

这里的“纯”需要精确定义：普通值计算没有外部效果；调试输出和诊断报告是由 Host
观察的两个受控通道。特别是 `report`/`emit_*` 会产生诊断事件，并可能使最终结果
不可发布。它们不修改 Telora 值，但属于求值的可观察行为。

## 2. 源文件和词法表面

Telora 源文件通常以 `.telora` 结尾。`#` 引入行注释；文件开头可以包含 shebang。
空白和注释不影响求值，但 lossless parser 会保留它们以供工具使用。

基础字面量包括：

```telora
42                         # Int
0.75                       # Float
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

插值当前支持具有稳定文本表示的标量类别，不是隐式调用任意用户 `Display`。

Bool 没有独立运行时类别。它是闭合的 Atom 类型，其值为 `'True` 和 `'False`。
条件位置只接受 Bool，不进行 truthiness 转换。

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
之间没有隐式转换。Int 使用有符号数值顺序。Float 使用 IEEE/Rust primitive 比较：
涉及 NaN 的四种顺序比较均为 false，NaN 与任何值（包括自身）的 `==` 为 false、
`!=` 为 true，正负零相等。

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
*  /  +  -
<  >  <=  >=  ==  !=
&&  ||
|>
```

`!` 只接受 Bool，求值一次并返回相反的 canonical Bool。它与数值负号同属前缀一元
优先级，结合强于比较和布尔中缀运算。`&&` 和 `||` 只接受 Bool 并短路。
`left |> right` 统一降低为 `right(left)`。

Tuple 使用非负十进制字面量做位置投影，例如 `(left, right).0`。已知 Tuple 的位置在
分析期检查并得到精确成员类型；通过 `Any` 边界的 Tuple 在运行期检查类型和范围。

`if` 必须有 `else`，两个分支产生可合并的类型：

```telora
if enabled { "on" } else { "off" }
```

`return expression;` 从最近的函数返回。它不是模块导出机制。

### 4.1 模式匹配

Pattern 可以匹配字面量、Atom、Tagged payload、Tuple 和 Struct 字段：

```telora
match result {
    'Ok(value) => value,
    'Err(error) => raise!(error),
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
def parse_pair: Fn(String, String) -> Result(Tuple([Int, Int]), BlameError) =
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
`Never` 表示不产生值的路径，例如 `return`、`raise!` 和 `panic!`；它作为 bottom
参与方向性检查，避免根失败制造级联类型错误。

### 6.1 函数契约和 rank-1 多态

函数契约使用专用的 `Fn(P1, ..., Pn) -> R` 记法。它精确降解为普通元数据构造
`Func([P1, ..., Pn], R)`；`Fn` 不是值环境中的 callable，`Func` 才是构造函数元数据的
普通内建 callable。参数和结果位置都递归接受完整契约记法，例如：

```telora
Fn(A) -> Tuple([B, C])
Fn(Fn(A) -> B) -> Array(Tuple([A, B]))
```

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
可以组合本模块中的其他 family，也可以跨完整、选择性、alias 和 open import 保留
精确 scheme。

当前 family 声明不能依赖同模块的普通 helper 或非参数化 `type` binding；
可以依赖内建 metadata 构造器、imported metadata 能力及其他无环本地 family。该限制
防止尚未完成调度的 concrete/recursive placeholder 进入符号模板。

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
def show_dyn: Fn(Dyn) -> Result(String, BlameError) = ...;

def show:
    for(A) Fn(TypeOf(A)) -> Fn(A) -> Result(String, BlameError) =
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

Host 选择的根入口位于 crate 的 `bin-src/` 下时，它不属于 `src/` 模块层级，不能
使用 `./` 或 `../` 导入 crate source；crate source 必须写成 `@src/<path>`，并从
当前 crate 的 `src/` 根解析：

```telora
# bin-src/report.telora
import "@src/model/report.telora" { compile };
```

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
export record 定义，Host adapter 再从该 record 选择协议规定的 entry。

### 8.1 模块身份和依赖

模块的语义身份由 authority、crate 和逻辑路径决定，不等于偶然的物理绝对路径。
相同模块经不同相对边解析时应复用同一身份；resolver 拒绝词法或 symlink 越界。

Path crate 的依赖由根目录 `telora-deps.json` 固定。依赖名是 package import 的首段。
当前没有通用 registry 获取、版本求解或运行时 package acquisition。

根模块可以使用静态 `option` 声明 Host/resolver 配置。Option 必须位于 import 解析
之前的有效位置，嵌套依赖不能借此修改根 Host 的策略。

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

### 9.2 Blame 和立即终止

`BlameError` 当前是结构类型：

```telora
{data: Any, message: String, rule: Any}
```

`blame!(message, subjects...)` 构造该值，并用调用位置及 subject origin 建立诊断所需的
关系。`raise!(error)` 要求一个 BlameError，以 `Never` 终止当前计算并将控制权交还
VM/Host：

```telora
match validate(User, raw) {
    'Ok(user) => user,
    'Err(error) => raise!(error),
}
```

`fail!(message, subjects...)` 是 `raise!(blame!(...))` 的便利形式。`panic!(message)`
产生无结构的程序失败，应表示实现错误或无法恢复的不变量破坏，而不是可预期的领域
拒绝。

### 9.3 Host-observed diagnostic

Prelude 提供：

```telora
report: Fn(Severity, BlameError) -> BlameError
```

它记录 Info、Warn 或 Error 事件并返回原 BlameError。便利形式：

```telora
emit_info!(message, subjects...)
emit_warn!(message, subjects...)
emit_error!(message, subjects...)
```

分别降低为 `report(severity, blame!(...))`。Info/Warn 不阻止成功；Error 允许当前普通
控制流继续，使独立检查能够报告更多根因，但 session 在发布前会失败：

```telora
def reject: Fn(Intent) -> Option(Plan) = fn(intent) {
    let ignored = emit_error!("intent cannot be lowered", intent);
    'None
};
```

诊断集合属于 evaluation account，而不是隐含的普通 Array 返回值。调用次数和顺序
可被 Host 观察，因此 `report` 是一项窄的诊断 effect；实现和优化必须保留它。

当前 BlameError 没有稳定 category、stage、expected/actual、cause 或 repair 字段。
多个 subject 可以贡献 primary 和 related location；其载体表示不构成公开的诊断
序列化协议。

### 9.4 失败类别

VM 区分可恢复的程序失败与终止整个 evaluation session 的资源/一致性失败。前者包括
类型不匹配、missing field、non-exhaustive dynamic match、panic、raised blame 和
reported Error；后者包括取消、fuel/分配/栈/调用深度耗尽以及无效 bytecode。

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
Host 选择根模块和 entry 协议
  -> 固定依赖与 source snapshot
  -> 注册 typed native/virtual module 和显式输入
  -> 初始化并冻结 Main world
  -> 取得命名 export
  -> 用协议 TypeMetadata 校验、调用或投影
  -> 检查诊断和结果
  -> 授权、解释或拒绝普通值
```

Host virtual input 属于一次调用，在 Main 执行前冻结。Main 不能反向 import Host entry
runtime，也不能请求 Host 在执行途中打开新的观察窗口。

Plan 没有语言级权限。一个值即使静态类型为应用定义的 `ExecPlan`，也只是一段数据；
只有相应 Host adapter 可以解释它。

### 11.1 当前 CLI Host

当前 `telora` 二进制提供：

```text
telora check <module>
telora run <module> [--input <json|->]
telora types <module>
telora show <module> [at <source> <line> <column>]
telora exec --dry-run <module> [-- <arguments>...]
telora build --dry-run <module>
telora lsp
```

`run` 从显式 export record 选择 `output`。外部 JSON 输入以显式 `input` binding 进入。
`exec` 和 `build` 是具体协议，不是通用 action ABI：前者通过受信 entry module 调用
应用 `exec`，后者调用 `build`，随后校验并输出规范 JSON。

当前 `exec` 和 `build` 强制 `--dry-run`。它们不会下载、启动进程或写出构建文件；
现实效果执行尚未由这个 CLI 实现。

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
与 schema、TypeMetadata attribute、Dyn observer、文本 parse/display、debug 等。
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
- 生产级 exec/build effect executor；
- 对外部生态或未来版本的长期兼容性保证。

此外，下列能力具有明确的当前限制：

- 参数化 TypeMetadata family 不能参数化递归，也不能调用同模块普通 helper；
- `interpreter!` 只提升直接 A 参数的消费型解释器，不能适配高阶或返回 A 的位置；
- Func 在公开 Type descriptor 解释中是受限的 opaque leaf；
- 普通 CLI 严格失败输出不保证一次展示 recovery 已收集的全部独立诊断；
- Host-observed diagnostic 具有 severity/message/location/labels，但还没有稳定的领域
  category、cause graph 或 repair schema；
- 工具可以保守丢失精度，不能以猜测填补缺失语法或失败依赖。

这些限制是当前规范的一部分。应用不能依赖规范之外的推断、反射或 Host fallback。

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
