# Telora 语言设计

本文档是 Telora 当前完整语言设计的稳定基线。它描述整体架构与语义边界，不要求
读者重放 RFC 历史。[`CONCEPT.md`](CONCEPT.md) 定义本文使用的术语，
[`../MOTIVATION.md`](../MOTIVATION.md) 定义问题和目标。
文档的权威关系与维护规则见 [`../README.md`](../README.md)。

本文不是语法参考或教程。具体写法与编程指导属于
[`../../tutorial.md`](../../tutorial.md)；单项决策的动机、备选方案、验收条件与
实现证据仍由 RFC 保存。

## 设计不变量

Telora 面向 MRT（Modelling、Representation、Transformation），并坚持以下
不变量：

1. **封闭。** 影响一次计算的代码、静态数据、依赖和显式输入，在程序阶段求值前
   可以枚举。
2. **纯粹。** 普通 Telora 代码不能执行外部效果，值不可变。
3. **确定。** 相同源码、依赖图、Host 输入和预算产生相同的值或结构化失败。
4. **有界。** 允许递归，但每次求值都有有限资源配额与协作式取消点。
5. **来源可追踪。** 来源能够穿过解析、导入、转换、校验、元数据解释和诊断。
6. **由 Host 授权。** 只有 Host 可以观察开放世界或赋予输出外部意义。
7. **机制优先。** 领域政策应写成库；只有一般语言模型无法忠实表达的能力，才是
   核心机制候选。

这些是不允许功能提案绕过的约束，不只是实现偏好。

## 作为 eDSL 承载语言

任何 DSL 最终都需要一门 GPL 或等价的通用实现机制。Telora 选择成为一门范围
明确的 GPL：它提供封闭、纯粹的数据计算能力，并把领域特征留给普通库表达。

一个 Telora eDSL 由以下普通构件组成：

- 类型化的领域数据与 intent vocabulary；
- 模块和显式依赖；
- 实现领域不变量、组合和 lowering 的函数；
- 由 TypeMetadata 承载的结构、属性和可解释能力；
- 保留 authored subject 与规则位置的诊断；
- 由应用或 Host 定义的最终 artifact 协议。

eDSL 没有特殊执行模式。它复用语言的检查器、VM、模块系统、来源模型和工具链。
领域抽象的成功标准不是“像新语法”，而是领域知识能否集中在库中、调用者能否只
提供自己拥有的事实，并且错误仍能指向领域输入和规则。

## 程序和值

Telora 程序使用 expression、词法绑定、函数、闭包、调用、条件、模式匹配、递归
和模块计算不可变值。合并、校验、规范化、codec 派生、plan 构造和图遍历等领域
操作都是函数与数据，不是特殊求值模式。

紧凑的运行时值模型由以下类别组成：

```text
Int, Float, String, Bytes, Atom, Tuple, Array, Dict, Func
```

Record 与同质 `Dict` 共用运行时 `Dict` 表示，但保留不同的静态元数据。Tagged
tuple 表达 sum 和协议值：

```telora
'None
'Some(value)
'Ok(value)
'Err(error)
```

条件只接受 `'True` 和 `'False`，不存在通用 truthiness 转换。语言使用者观察到的
集合是持久且不可变的。实现可以在分配、intern 和发布结构内部使用不可观察的
mutation，但失败工作不能泄漏到持久世界。

## 函数与多态

函数是一等值。公共契约可以声明单态或显式 rank-1 多态：

```telora
def identity: for(A) Fn(A) -> A = fn(value) { value };
```

显式泛型定义的 body 必须对每一个 bound type parameter 刚性成立。调用可以依赖
推断，也可以使用完整或部分显式类型应用；部分应用留下的参数必须能够从上下文
推断。

局部闭包 generalization 是保守且有界的。它只适用于合格的无环绑定，保留未解决
obligation，也不会把递归组或不稳定的数值、callable 约束随意泛化。Alias 不会
产生无限制的 let-polymorphism。公共 API 应优先写出完整契约。

Telora 当前提供 rank-1 多态，而不是 higher-rank type、trait、subtyping 或通用
constraint solver。新增抽象前，必须证明现有普通函数和显式 typed callback 无法
清楚表达某个一般性的类型关系。

## 类型即数据

类型声明会求值一个 expression，得到规范的 TypeMetadata。TypeMetadata 是普通、
不可变的 Telora 数据，由程序代码所使用的同一套 VM 语义求值。编译器解释求值
结果，而不会在隐藏的类型语言中重新实现用户的元数据函数。

以下三个概念必须严格区分：

```text
Type             有效 TypeMetadata 值的静态类型
TypeOf(A)        描述 A 类型值的元数据见证
TypeDesc         元数据解释器使用的擦除后公开视图
```

`TypeOf(A)` 可以赋给 `Type`，并在运行时擦除。它保留了校验、codec 和类型化
interpreter 边界所需的静态关系：

```telora
validate: for(A) Fn(TypeOf(A), Any) -> Result(A, BlameError)
```

Primitive 和内建元数据构造器具有精确的 witness type。用户函数也可以在工具阶段
构造并返回元数据。Decorator 是普通的 metadata-to-metadata 函数；attribute 是
附着在规范 descriptor 上的数据。

参数化 `type` 声明定义可命名的 TypeMetadata family：

```telora
@struct
type Box(A) = {value: A};

def wrap: for(A) Fn(A) -> Box(A) = fn(value) { {value} };
```

声明时，每个参数被绑定为刚性的 Bound TypeMetadata，decorated body 只求值一次，
结果形成规范的符号模板。`Box` 同时具有以下两个一致表面：

```text
类型位置    Box(A)
值位置      Box: for(A) Fn(TypeOf(A)) -> TypeOf(Box(A))
```

应用 family 时只对模板做避免捕获的参数替换，不会针对 concrete type 重新执行 body。
因此通过 `TypeDesc` 观察参数所得的分支在声明时已经固定。Family 可以无环地组合
其他本地 family，并通过完整模块、选择性、open 或 alias import 保留精确 scheme；
参数个数、TypeMetadata 有效性、重复参数与递归 component 都有明确诊断。
替换以规范 TypeMetadata 值为模板，保留 `WithAttributes`、codec 规则和 application
来源。严格分析因其他错误失败时，partial recovery 仍为独立有效的 family 发布精确
scheme，不得退化为 `Any` 或不可计算事实。

TypeMetadata family 是 rank-1 witness 关系，不是任意函数调用进入类型位置，也不是
trait、associated type、higher-kinded parameter 或名义类型构造器。它不支持 partial
application 和参数化递归。当前实现也拒绝依赖本模块的非参数化 `type` 或普通 helper，
避免尚未完成调度的 concrete/recursive placeholder 污染符号模板；imported metadata
能力和内建构造器不受此限制。

递归 TypeMetadata 在内部使用有限图身份，并提供安全的公开 reference traversal。
构造成功后，元数据只发布一次到持久世界；失败或部分构造的值不能成为权威类型。

`Dyn` 是狭窄的 existential 边界，携带值、权威的运行时类型关系和来源；它不是
unchecked cast。`interpreter!(...)` 在静态 witness API 与擦除后的用户态
`TypeDesc` interpreter 之间建立受控桥梁，不能凭空制造任意 bound type 的值。

`Any` 表示显式丢失静态精度，与源码不完整造成的 unknown fact 不同。内部
`Never` 表示根错误之后无法返回值的路径，避免一个根问题产生误导性的连锁类型
错误。

## 一个求值器，两个阶段

Telora 区分**工具阶段**和**程序阶段**，但不定义两门求值语言：

- 工具阶段计算分析所需的闭合元数据、契约、模块接口、decorator 和派生计划；
- 程序阶段根据显式输入计算应用值；
- 两个阶段共用值模型、函数行为、bytecode VM、配额和失败语义；
- 静态 witness 与 annotation 默认擦除，除非程序显式把元数据当作值使用。

工作区分析中的工具阶段求值按依赖范围执行，并允许 best-effort recovery。一个损坏
组件不应抹去仍然可以独立确定的事实；但任何严格发布仍要求对应结果的全部强制
证据完整。

Telora 不提供无限制 macro expansion、runtime `eval` 或任意代码生成。Surface
elaboration 可以把便利语法降低到较小核心，但必须保留来源位置，也不能引入第二套
领域语义。

## 模块与封闭世界

模块组成静态可解析的不可变图。模块显式导出命名 binding，没有隐式最终结果。
Import 可以绑定完整模块，也可以从已声明接口选择名称。
完整模块、选择性、alias 和 open import 必须保留同一个权威 Type Scheme。Scheme
与供浅层观察使用的擦除后函数形状分别存储；导入方的单态事实不得包含被导出
scheme 私有的 Bound 身份。

Crate 依赖在求值前声明，并解析为规范模块身份。物理路径是解析输入，不是程序所
观察的语义身份。Dynamic import 和由程序自行选择的 package acquisition 不属于
语言模型。

Telora、JSON、TOML 和 YAML 文件都可以成为同一图中的 source module。静态数据
模块拥有各自格式的解析规则，但在后续分析中表现为不可变、带位置的值。字段级
来源继续参与校验和诊断。

模块加载器区分临时工作与持久发布。权威 export 和递归 TypeMetadata root 只有在
初始化成功后才进入持久 main heap。失败、取消、过期或超配额的工作被原子丢弃。

## 来源、诊断与失败

来源是语义数据流的一部分。值可以携带源码 expression 或静态数据字段的 origin，
并使其穿过 import、转换、元数据、codec 规范化和 Host 边界。

一个诊断可以关联多个位置：

```text
subject source     被拒绝的数据或 intent
rule source        施加要求的 contract 或 authored rule
transformation     相关中间过程的 provenance
```

不同通道表达不同含义：

- `Option(T)` 表达预期内的缺失或可选证据；
- `Result(T, E)` 表达必须由调用者处理的 value-level boundary outcome；
- `raise!` 使用结构化 blame 终止当前计算；
- `emit_error!` 报告 Host 可观察的错误，同时允许无关普通控制流继续；fatal
  diagnostic 仍然使发布无效。

诊断积累不是通用 effect，也不是隐藏的 value-level array。普通函数、`Option` 和
Host-observed event 支持 best-effort checking，Host 则拥有最终发布策略。

编译器和工作区 recovery 区分 known fact、显式 `Any`、unknown、conflict、
dependency blocking 与 tool-stage incomputability。Recovery 必须保留仍有依据的
独立事实，但不能虚构值、字段或类型。

## 确定的资源边界

Telora 允许递归，不要求程序必然规范化。每次求值 session 都有独立的 fuel、栈、
调用深度和分配限制，并支持协作式取消。

Fuel 在 call 和实际执行的 control-flow back edge 等语义扩展点扣减。它是确定的
终止边界，不是 CPU 时间模拟；无害的 straight-line compiler layout 改动不应
意外改变程序能否落在预算内。

配额耗尽是结构化、来源可追踪的失败，不能发布部分 module、部分 metadata graph
或过期 workspace snapshot。在遍历顺序没有领域意义时，表示、输出、推断和诊断
都必须使用规范顺序。

## Host 边界与 Entry

普通 Telora 代码没有外部权限。Host 拥有 IO、环境观察、时间、持久化、权限、
重试、事务和真实效果。

边界如下：

```text
开放的 Host 世界
  -> 选择 trusted entry 与显式 input snapshot
  -> 准备 pending module handle 和 typed virtual module
  -> 初始化并冻结封闭的 Main 世界
  -> 获取并校验命名 export
  -> 授权、解释或拒绝该值
  -> 可选的外部效果
```

Main 不能 import entry runtime。Host 提供的值和 virtual module 属于一次调用，
并在 Main 求值前冻结。Host 先通过 existential boundary 观察 export，再使用
权威的 `TypeOf(A)` 协议投影，之后才能调用或发布。

`telora run`、`telora exec` 和 `telora build` 是具体 Host adapter。它们的 entry
名称和 plan type 不构成通用语言 effect ABI。Plan 是普通数据；构造 plan 不会
执行它。

## 标准库边界

依赖方向固定为：

```text
语言与 VM 机制
  -> 通用标准库
  -> 可复用领域库或方法库
  -> 应用模型与 intent
```

Host adapter 从纯计算世界之外依赖语言与应用协议。实验可以观察和检验每一层，
但不能反转上述依赖方向。

通用标准库可以包含数据结构、combinator、元数据观察、codec、format、path、hash
等具有广泛意义的确定操作。没有独立的一般性证据和中性契约，它不能引入 ontology、
analytics model、build system、Agent workflow 或其他实验中的概念。

放置新行为时依次询问：

1. 它能否只是普通 application function？
2. 它是否属于可复用的 domain/method library policy？
3. 它是否是通用、确定的 standard-library operation？
4. 只有前三层都无法忠实表达时，才讨论缺少哪项最小语言或 Host 机制。

## 工具模型

Parser、HIR、类型分析、工具阶段求值、workspace snapshot、CLI query 和 LSP 共享
权威语义事实。工具不能各自从文本反向猜测运行时行为。

不完整源码是正常工具状态。Syntax recovery 保留稳定位置；semantic recovery 只
发布当前 revision 能证明的事实；异步工作不能覆盖更新的 snapshot。取消与配额
贯穿整个分析过程。

Strict command 与 editor query 可以展示不同的事实子集，但对双方共同展示的事实，
其意义和确定程度必须一致。

## 当前设计边界

当前设计有意不包含：

- 语言级 effect 或 ambient IO；
- runtime code generation、dynamic import 或通用 macro system；
- trait、interface、associated type、subtyping 或 higher-kinded type；
- 无限制 polymorphism 或所有 binding 的自动 generalization；
- 通用 package acquisition 或 action protocol；
- 领域专用的 ontology、analytics、deployment 或 Agent 语义；
- 脱离 Host 配额的全局终止保证。

未来 RFC 只有在陈述一般问题、保持设计不变量，并在语言变化被接受后同步更新本文档
时，才可以改变这份基线。
