# Telora 最小实用教程

本文面向第一次接触 Telora、但需要独立编写类型化库和应用的读者。它描述稳定的
公开语言表面，不要求阅读编译器源码、RFC 或仓库示例。

Telora 是一门封闭、纯粹、确定且来源可追踪的语言。程序对不可变数据进行计算，
Host 决定输出值是否具有外部效果。本文重点介绍数据建模、高阶函数、可编程类型
元数据、模块和诊断；这些能力足以实现一门库形式的 embedded DSL。

## 1. 文件、注释和基本命令

Telora 源文件扩展名是 `.telora`。单行注释以 `#` 开始：

```telora
# This is a comment.
let answer = 42;
```

常用 Host 命令是：

```text
telora check path/to/main.telora
telora run path/to/main.telora
telora show path/to/main.telora
```

`check` 做模块解析和严格类型检查；`run` 执行模块并显示命名导出；`show` 用于观察
模块接口。执行有 fuel、栈、深度和分配预算。

在 clean-room 实验中，生成代码的 Agent 不直接运行这些命令。Host 运行并返回
带源码位置的诊断。

## 2. 值与字面量

### 数字和 String

```telora
let count: Int = 42;
let ratio: Float = 0.75;
let name: String = "telora";
```

普通 String 支持转义。反斜线后连续 whitespace 会被忽略，适合拆分长字符串。
raw string 使用 Rust 风格 delimiter：

```telora
let pattern = r"\w+";
let quoted = r#"a "quoted" value"#;
```

插值不是普通 String，而是 concat expression：

```telora
let message = `hello \{name}`;
```

### Bool 是 tag

Telora 的 Bool 值写作 `'True` 和 `'False`，不能省略单引号：

```telora
let enabled: Bool = 'True;

if enabled {
    "on"
} else {
    "off"
}
```

### Array、record 和 Dict

```telora
let numbers: Array(Int) = [1, 2, 3];
let second: Int = numbers[1];
let user = {name: "Ada", active: 'True};
let labels: Dict(String) = {region: "east", tier: "gold"};
```

Array 是同质集合。record 字段集合是静态结构。`Dict(T)` 是 String key 到同一值
类型 `T` 的动态映射。`numbers[1]` 直接得到元素，越界会以 `OutOfRange` blame
失败；需要可分支的缺失值时使用 `array.get(numbers, index)`。

Tuple 用字面量位置选择成员，例如 `(1, "one").0` 的值是 `1`，类型是 `Int`。

## 3. 绑定和函数

`let` 定义局部值，`def` 定义模块级函数或值：

```telora
let local = 1;

def add: Fn(Int, Int) -> Int = fn(left, right) {
    left + right
};
```

函数体最后一个 expression 是返回值。`return` 可用于提前返回：

泛型调用通常推断类型参数，也可以用 `@[...]` 显式应用：

```telora
identity@[Int](1)
pair@[Int, _](1, "text")
```

`_` 由调用上下文推断。未标记的 `value[index]` 只表示 Array 索引。

```telora
def absolute: Fn(Int) -> Int = fn(value) {
    if value >= 0 {
        return value;
    };
    0 - value
};
```

函数是一等值，可以作为参数和返回值：

```telora
def apply_twice: for(A) Fn(Fn(A) -> A, A) -> A = fn(f, value) {
    f(f(value))
};
```

显式多态使用 `for(...)`。定义的 body 必须对所有类型参数成立。未标注的局部闭包
可以获得保守的 rank-1 推导，但公共库接口应写完整契约。

## 4. Struct、enum 和类型别名

### Struct

```telora
@struct type User = {
    id: Int,
    name: String,
    labels: Array(String),
};

let user: User = {id: 1, name: "Ada", labels: ["admin"]};
let id = user.id;
```

field shorthand 可避免重复：

```telora
def make_user: Fn(Int, String) -> User = fn(id, name) {
    {id, name, labels: []}
};
```

### Enum 与 Tagged 值

```telora
@enum type Status = {
    Pending: 'None,
    Failed: String,
};

let pending: Status = 'Pending;
let failed: Status = 'Failed("timeout");
```

零 payload variant 是一个值；有 payload 的 variant constructor 在应用后才得到
enum 值。

### Type alias

```telora
type UserNames = Array(String);
```

alias 不创建名义上不同的新类型。

### 参数化 TypeMetadata family

需要让一个可复用的 metadata 结果出现在泛型契约中时，使用参数化 `type` 声明：

```telora
@struct
type Box(Item) = {
    value: Item,
};

def wrap: for(Item) Fn(Item) -> Box(Item) = fn(value) {
    {value}
};

type StringBox = Box(String);
let boxed: StringBox = wrap("ready");
```

`Box(Item)` 是契约中的类型，而 `Box` 也是一个普通 callable metadata 值，其类型为
`for(Item) Fn(TypeOf(Item)) -> TypeOf(Box(Item))`。声明 body 以符号参数求值一次；
`Box(String)` 只替换已发布模板，不会用 `String` 再执行 body。

Family 可以组合其他无环 family，也可以像其他 export 一样跨模块使用：

```telora
# containers.telora
@struct type Box(Item) = {value: Item};
export { Box };

# main.telora
import "./containers.telora" { Box };
type IntBox = Box(Int);
```

调用必须一次提供全部 TypeMetadata 参数。重复参数、参数数量错误、无效 metadata 和
直接或相互递归 family 都会在声明或调用位置报告。当前版本不允许 family 依赖同一
模块中普通的非参数化 `type` 或 helper；可以直接使用内建 metadata 构造器、imported
metadata 能力和其他无环本地 family。

## 5. Option、Result 和模式匹配

```telora
def first_or_zero: Fn(Option(Int)) -> Int = fn(value) {
    match value {
        'Some(found) => found,
        'None => 0,
    }
};
```

`Result(T, E)` 使用 `'Ok(value)` 和 `'Err(error)`。`?` 对 Option 或 Result 做同类
提前传播：

```telora
def parse_both: Fn(String, String) -> Result(Array(Int), BlameError) =
    fn(left, right) {
        let a = parse_int(left)?;
        let b = parse_int(right)?;
        'Ok([a, b])
    };
```

`if let` 和 `let ... else` 可处理局部模式：

```telora
if let 'Some(value) = candidate {
    use_value(value)
} else {
    fallback
}
```

对“尝试失败是正常分支”的 API 使用 Option。对需要由调用方显式处理的 boundary
错误可以使用 Result。领域规则明确知道输入非法时，通常应产生诊断并返回 None，
而不是用假的 fallback 继续发布结果。

## 6. Array 组合子

标准模块按路径导入：

```telora
import "std/array" as array;
```

常用函数：

```telora
array.map(values, fn(value) { transform(value) })
array.filter(values, fn(value) { predicate(value) })
array.find(values, fn(value) { predicate(value) })
array.any(values, fn(value) { predicate(value) })
array.all(values, fn(value) { predicate(value) })
array.flat_map(values, fn(value) { children(value) })
array.fold(values, initial, fn(state, value) { next(state, value) })
array.concat([left, right])
array.push(values, value)
array.length(values)
```

复杂泛型调用有时无法仅凭外层 record field 推导 callback 的结果。不要改成 Any；
提取一个完整签名的 typed adapter：

```telora
@struct type NodeSet = { nodes: Array(Node) };

def nodes_of: Fn(NodeSet) -> Array(Node) = fn(value) { value.nodes };

let all_nodes: Array(Node) = array.flat_map(groups, nodes_of);
```

同样，公共高阶 API 的 selector 通常应有完整类型，或由调用者传入 typed closure。

## 7. 模块、依赖和导出

### 导入

```telora
import "std/array" as array;
import "./local.telora" { User, make_user };
import "some-package/path/to/module.telora" as module;
```

路径优先 import 的形式是：

```telora
import "path";              # open module for symbol resolution
import "path" as name;      # module namespace
import "path" { item };     # selected exports
import "path" as name, { item };
```

### 导出

模块没有隐式返回值。显式导出值、函数或类型：

```telora
export let version = 1;
export def compile: Fn(Input) -> Option(Output) = fn(input) { ... };
export { User, make_user };
```

### Package dependency

crate 根目录使用 `telora-deps.json`：

```json
{
  "dependencies": {
    "ontology-edsl": {"path": "../ontology-edsl"}
  }
}
```

依赖 key 是 import 的首段。相对路径不会越过 crate 边界。Host 在加载前固定依赖，
运行时不能动态 import。

文件访问等级由 resolver 控制：普通 `.telora` 可作为公开模块；`.priv.telora` 仅包内
可见；`.native.telora` 承载 Host 注册的 native 声明并具有更严格边界。用户态 eDSL
通常只需要普通 `.telora`。

## 8. 诊断和 best-effort recovery

Telora 诊断会携带 source span，并能同时指向输入和规则位置。

### 构造错误

`blame!` 是 contextual intrinsic，不是普通函数：

```telora
let error = blame!("invalid relationship", authored_edge, policy);
```

### 报告并继续

```telora
let ignored = emit_warn!("deprecated field", field);
let ignored = emit_error!("unsupported capability", requested);
```

`emit_error!` 产生 Host diagnostic event，并返回 BlameError。领域 lowerer 可以在报告
后返回 `'None`，使其他独立元素继续计算：

```telora
def reject: Fn(Request) -> Option(Plan) = fn(request) {
    let ignored = emit_error!("request cannot be lowered", request);
    'None
};
```

Host recovery 可以收集多个独立错误。普通生产执行可能只显示阻止发布的首条错误；
完整诊断集的验收应使用 Host recovery，而不能只解析 CLI 的第一行。

### 立即失败

```telora
fail!("fatal contract violation", value)
```

或对已有 BlameError 使用 `raise!(error)`。只有确实不能继续获得独立信息时才立即
失败。

### 来源

把用户写下的 enum/record/field 作为 `emit_error!` 的 subject 参数。不要只传经过
转换后新构造的 String，否则诊断会远离意图来源。

## 9. 类型也是元数据

Telora 类型声明会求值得到规范化元数据。`TypeOf(A)` 表示“描述值类型 A 的元数据
见证”：

```telora
def Maybe:
    for(A) Fn(TypeOf(A)) -> TypeOf(Option(A)) =
    fn(Item) { Option(Item) };

type MaybeInt = Maybe(Int);
```

### 用户定义类型构造器

`struct`、`enum`、`Fn`、`Array` 和 `Option` 都可参与元数据计算：

```telora
def Capability:
    for(Id, Input, Output)
    Fn(TypeOf(Id), TypeOf(Input), TypeOf(Output)) -> Type =
    fn(Id, Input, Output) {
        struct('None, {
            id: Id,
            lower: Fn(Id, Input) -> Option(Output),
        })
    };

type UserCapability = Capability(UserId, Context, UserPlan);
```

调用 metadata function 的具体 type declaration 会保留完整结构。当前用户定义 family
的泛型返回通常只能写 `Type`，不能在另一个泛型签名中精确命名为 `TypeOf(F(A))`。

因此，一门 eDSL 常采用两种方式保持精度：

1. 企业侧用 metadata family 产生自己的具体类型；
2. 共享高阶函数对 Capability、Output、Node、Plan 等分别量化，并接受 typed
   selector/lowerer/builder callback。

不要为了缩短接口把闭合企业类型变成 `Any`、`Dyn` 或 String id。

### Continuation 构造结果

泛型函数无法命名调用者生成的 record family 时，可以让调用者提供 builder：

```telora
def classify:
    for(Node, Edge, Result)
    Fn(
        Array(Edge),
        Fn(Edge) -> Node,
        Fn(Array(Edge), Array(Node)) -> Result,
    ) -> Result =
    fn(edges, endpoint, build) {
        let selected = ...;
        let missing = ...;
        build(selected, missing)
    };
```

这比返回 Dict(Any) 更长，但保留了调用者的精确 Result 类型。

## 10. Decorator 是元数据函数

Decorator 不是特殊 attribute bag；它是作用于后续声明元数据的函数。普通 eDSL
不一定需要 decorator。优先用普通 metadata function 和高阶函数；只有声明式附加
信息确实更清楚时才定义 decorator。

不要假设存在 trait、interface、associated type、动态 code generation 或通用 quote。
Telora 的反射边界以 TypeMetadata、受限 Dyn observer 和显式 interpreter 为主。

## 11. 设计 embedded DSL 的建议

一门库形式 eDSL 通常由三层组成：

```text
metadata families
    定义调用者必须提供的 typed role records

higher-order rules
    实现一次维护的查找、组合、验证和 lowering invariant

application model
    提供封闭 vocabulary、事实、policy callback 和 final builder
```

### 应留在共享 eDSL 的内容

- 与具体企业名字无关的 capability lookup；
- 独立请求的 lowering 与完整性统计；
- 通用关系分类或规则组合；
- provenance-preserving error policy；
- candidate evaluation 与 atomic publication 顺序。

### 应留给应用的内容

- 闭合 id/entity enum；
- 具体事实、公式和物理 mapping；
- 真正不同的组合或授权策略；
- final typed plan shape 和 builder。

### 停止抽象的信号

- 应用必须传大量无意义 dummy value；
- 精确类型被迫退化为 Any/Dyn/String；
- 诊断只指向共享库而不再指向 authored intent；
- 一个应用的物理概念进入所谓通用层；
- 应用仍需在共享函数外重新实现相同阶段顺序。

机械 selector 是明确的 ergonomics cost，但在缺少结构约束/associated family 时可以
接受。文档应诚实区分“包含领域知识的 callback”和“只做字段 forwarding 的 adapter”。

## 12. 一个最小 typed lowering 示例

下面的示例展示模式，不规定任何 ontology 设计：

```telora
import "std/array" as array;

@enum type RuleId = { Enabled: 'None, Missing: 'None };
@struct type Rule = { id: RuleId, lower: Fn(RuleId) -> Option(String) };

def find_rule: Fn(Array(Rule), RuleId) -> Option(Rule) = fn(rules, requested) {
    array.find(rules, fn(rule) { rule.id == requested })
};

def lower_one: Fn(Array(Rule), RuleId) -> Option(String) = fn(rules, requested) {
    match find_rule(rules, requested) {
        'Some(rule) => rule.lower(requested),
        'None => {
            let ignored = emit_error!(
                `no rule is defined for \{requested}`,
                requested,
            );
            'None
        },
    }
};

def enabled: Rule = {
    id: 'Enabled,
    lower: fn(requested) { 'Some("enabled-plan") },
};

export let output = lower_one([enabled], 'Enabled);
```

真实 eDSL 应进一步保存所有独立结果、验证完整性，并只在证据完整时调用 final
builder。不要从第一个成功元素猜测缺失请求的含义。

## 13. 编写交付物前的检查表

在不能自行执行代码的条件下，提交前逐项检查：

- Bool 是否全部写为 `'True` / `'False`；
- tag 和 enum variant 是否使用单引号；
- 每个 module-level `def` / `export` 是否以分号结束；
- 公共函数是否有完整 `Fn` / `for` 契约；
- 泛型 `flat_map`、空数组和 selector 是否需要 typed adapter；
- 所有失败路径是否返回正确的 Option/Result 类型；
- independent lowerer 是否避免过早 fail；
- 最终 plan 是否只在完整证据下发布；
- 诊断 subject 是否保留 authored value；
- 模块是否显式导出调用者需要的类型和函数；
- `telora-deps.json` 的依赖 key 是否与 import 首段一致；
- eDSL 是否没有偷偷包含某个企业的 vocabulary 或物理 mapping。

这份教程刻意不提供完整 ontology eDSL。读者应使用上述语言机制，自行定义其角色
类型、extension points、验证规则和 lowering protocol。
