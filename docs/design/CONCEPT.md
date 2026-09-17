# Telora 核心概念

本文档定义讨论 Telora 时使用的稳定词汇，是 [`LANGUAGE.md`](LANGUAGE.md) 的概念
配套文档，而不是语法或实现 struct 的清单。
文档的权威关系与维护规则见 [`../README.md`](../README.md)。

这些词汇同时定义语义边界和所有权。领域库可以增加自己的概念，但不能悄悄改变
核心术语的含义。

## MRT

**MRT** 是 **Modelling、Representation、Transformation（建模、表示与转换）**。

- **Modelling** 定义概念、关系、不变量、策略和扩展点。
- **Representation** 让模型知识和程序状态拥有类型化、可检查、可交换且来源可
  追踪的形式。
- **Transformation** 在保留必要证据的前提下解析、验证并 lowering 一种表示到
  另一种表示。

在非平凡系统中，三者不能分离。不能表达模型的 representation 会丢失意义；不能
指出规则的 transformation 会丢失可追责性；不能执行或检查的 model 也无法可靠
约束结果。

## DSL、GPL 与 eDSL

### DSL

**DSL（Domain-Specific Language）**是用特定领域词汇表达模型、规则或意图的
语言表面。DSL 缩小了作者需要操作的概念范围，但其解析、检查、求值、转换和诊断
仍必须由更一般的实现机制完成。

### GPL

**GPL（General-Purpose Language）**提供足以实现或承载 DSL 的通用计算与抽象
机制。这里的“通用”不等于必须拥有环境 IO 或任意效果。Telora 的通用性集中在
封闭、纯粹的 MRT 计算。

### eDSL

**eDSL（Embedded DSL）**是由承载语言的普通类型、值、函数和模块表达的 DSL。
它复用承载语言的检查、执行、来源、诊断和工具模型，而不是另外实现一套完整编译器。

一个 eDSL 仍然可以拥有清楚的领域边界和 authoring contract。判断它是否成功，
应看领域知识是否集中、类型关系是否保留、错误能否指向领域 subject，而不是看它
是否拥有专用语法。

## Intent、Evidence 与 Artifact

### Intent

**Intent（意图）**是用应用或领域库拥有的 vocabulary 表达的高层请求。它说明想
得到什么，而不要求作者手工构造低层结果。

Intent 不是内建语法类别，也不一定来自自然语言。它是普通的类型化 Telora 数据
或代码。Build request、desired deployment、data query、configuration objective
或生成程序都可以成为 intent。

### Evidence

**Evidence（证据）**是证明某次转换或发布决定成立所需的信息，例如已解析的
capability、通过校验的值、依赖结果、策略决定或分类后的关系。

Evidence 同时具有值和来源。缺少强制 evidence 会阻止发布；独立 evidence 仍可以
继续计算，使诊断不必停在第一个无关错误。

### Lowering

**Lowering** 是从高层表示到更具体表示的语义保持转换，通常同时完成解析和验证。

Lowering 不等于编译器代码生成。它可以只是领域库中的普通 Telora 函数；其结果
仍然可以是等待下一层继续 lowering 的数据。

### Artifact

**Artifact（制品）**是 lowering 得到的完整、显式输出，可以由 Host 校验和解释。
Plan、生成文件集合、规范化配置、schema 或 migration description 都可以是 artifact。

Artifact 是普通不可变数据。其类型不授予执行效果的权限。

### Plan

**Plan（计划）**是一种描述潜在未来 action 或 output 的 artifact。Plan vocabulary
与校验协议属于相应 Host 或应用；Telora 不定义通用 action 或 plan type。

## Language、Library、Application 与 Experiment

### Language Core

**Language core（语言核心）**包括通用 syntax、static semantics、runtime semantics、
module semantics、source/provenance 行为、diagnostic 与有界求值机制。它不知道任何
特定领域。

### Generic Standard Library

**Generic standard library（通用标准库）**包含契约具有广泛意义的确定操作。它
建立在语言机制之上，并且不依赖任何应用实验就能解释和使用。

`std/fmt` 的 `Fmt` 是标准库拥有的 opaque、不可变延迟展示树。`Display` evidence
把一个静态类型的值转换成 `Fmt`，`concat` 组合 fragment，`render` 才产生最终
String。Fmt 的 Host payload 和最终物化结果都受 allocation quota 约束；最终结果在
分配前按共享节点 memoize 测量，payload 在复制前预扣，复用 fragment 不会绕过
展开后的字节核算，也不会让拒绝路径按展开结果做指数计算。它不是通用动态值、
codec 表示或 Host debug repr。

### Domain Library 与 Method Library

**Domain library（领域库）**引入领域 vocabulary；**method library（方法库）**
拥有一套可复用的领域转换或校验方法。二者都是位于通用标准库之上的普通 Telora
库，也可以共同构成 eDSL。

Ontology eDSL、analytics compiler、build policy 或 deployment model 默认属于
这一层，除非实验另外证明了一个可以中性表述的通用机制缺口。

### Application

**Application（应用）**提供私有事实、选定策略、具体 model type 和 Host-facing
protocol。不能仅为了缩短一个 fixture，就把 application knowledge 移进通用层。

### Experiment

**Experiment（实验）**提供关于语言和库的可用性或边界的证据。实验结果可以推动
中性 RFC，但实验 vocabulary 和 workaround 不会自动成为语言或标准库设计。

依赖方向固定为：

```text
language core
  -> generic standard library
  -> domain/method library 与 eDSL
  -> application model 与 authored intent

experiment -> 观察并检验这些层次
Host       -> 从封闭计算之外包围这些层次
```

任何通用标准库模块都不能依赖 ontology 或其他实验才能解释自己的契约。

## World 与 Authority

### Closed World

**Closed world（封闭世界）**是一次计算所处的世界：其代码、静态数据、依赖图和
显式输入在程序阶段求值前固定且可枚举。它可以包含真实 runtime request，但运行
期间不能获取任意新代码或 ambient data。

### Open World

**Open world（开放世界）**是包含 filesystem、process、network、clock、mutable
service、credential 和其他效果的外部环境。只有 Host 与它交互。

### Host

**Host** 嵌入 Telora。它选择输入和预算、选择工具 adapter、初始化 closed world、
校验输出、拥有诊断展示，并决定 artifact 是否可以影响 open world。

Host 是权限边界，不只是 foreign-function interface。

### Main

**Application export** 是一次 Host 调用所选择的普通封闭 module graph 中的公开值。除非 Host 在冻结前把信息
显式准备成输入值或静态数据模块，否则 Main 不能观察 Host facility。

### Entry 与 TransformService

服务入口是模块导出的具体类型 MainService，必须实现 std/transform-service.TransformService：

```telora
trait TransformService {
    init: Fn(Context) -> Self,
    transform: Fn(Self, Value) -> Value,
};
```

Context 为 {sources: Dict(Value)}。@service.source("name") 是普通类型 property，
多次声明归并为稳定来源清单，重复声明报错。Host 提供的来源必须与清单一致。
MainService 可以是正常类型别名或重导出，所有类型参数和方法实例都在 MIR 中确定。
内置 entry 包装方法调用，无运行时 trait 派发，也不按模块成员 shape 猜测入口。

模块顶层值与 property 初始化完成后，Host 准备来源并调用 init；随后每次调用 transform。
来源只读取一次，服务间隙 reset 到初始化后的确定基线。transform 不产生跨请求状态更新。
init 失败不发布实例。内置 with_diagnostics 捕获普通语言 failure 并保留完整诊断；
fuel/memory 耗尽由执行器结束当前请求，下一个请求仍从同一基线获得独立预算。
配额用于可停机，不是精确计费，reset 和页级计量方式不构成语言契约。

run 从 stdin 读取一个 JSON，成功输出一个 JSON Value；serve --bind stdio+jsonl:// 读取 JSONL，
每条输入对应 {ok, error, diagnostics} 响应，按输入顺序处理。diagnostics 包含 severity、
message、labels、notes；已捕获诊断不重复输出。两者都保留 stdin 给请求，不接受 stdin
初始化 source。--source name=path.json 或 file+FORMAT://path 使用已有格式验证和来源管线。
初始化来源使用 @service/name，请求来源使用 @request；物理路径不进入来源身份。

服务不获得环境、进程、网络或任意文件能力；需要的业务输入由 Host 显式转成 Value。
旧 eval-with、entry.Eval/Run/Serve、应用 EES 与 reducer 协议均已删除。
包管理的 IMOS 能力只在私有 Host 中使用。详细用法见 [执行模式](../../guide/EXEC-MODE.md)。

### Pure Eval

`eval` 读取一个 Value 导出，不构造服务实例。

### Freeze 与 Publication

**Freeze（冻结）**关闭本次求值的输入和模块世界。**Publication（发布）**使成功
初始化的值、module export、type graph 或 workspace snapshot 成为权威持久结果。

临时、失败、取消、过期或超配额的工作不能发布。对外暴露的 artifact 必须原子发布。

## Source、Value 与 Identity

### Source

**Source（源码）**是具有稳定身份和位置的 Telora、JSON、TOML 或 YAML 文档，参与
解析、依赖分析、provenance 和 diagnostic。

**Canonical source path（规范来源路径）**是 Source 对语言值和诊断公开的稳定名字。
它与 Host 用于读取数据的物理 locator 分离，也不必是 module identity。运行上下文中的
服务初始化来源使用 `@service/<key>`，逐次输入使用 `@request`。
源码文档。它保留格式特定的 syntax behavior 和字段级 origin。

### Value

**Value（值）**是不可变 runtime datum。值可以携带用于诊断的隐藏 source location
与 metadata，而不改变普通语言 equality 或 serialized payload。

### Provenance

**Provenance（来源链）**记录值、规则或转换从何而来。它穿过计算，使后续失败可以
把被拒绝的 subject 与 authored requirement 联系起来。

### Module Identity

**Module identity（模块身份）**是 dependency、interface、cache 和 diagnostic 使用
的规范语义身份。完成解析后，它不能依赖偶然的物理路径拼写。

**Crate vendor（crate 来源）**在模块图发现前把 crate name 映射到不可变 source。
resolver 按 vendor 顺序注册 crate，并以 crate 为颗粒采用 first-win；builtin vendor
先提供 `std`，当前 crate 先于 dependencies。后序同名来源不能补充或覆盖该 crate。

**Workspace config** 为 workspace 中的每个 crate name 选择唯一 source：workspace
member 或确定的远程 tarball。**Crate manifest** 声明 crate 的 canonical name、权威
普通 module catalog 和直接 dependency names。**Workspace lock** 固定完整精确 package
graph；除显式 lock 操作外，Host 只验证和消费它。

**Package preparation** 是 resolver 之前的 Host 阶段。它验证 config 与 lock、通过内嵌
`telora-ees` 的 IMOS component 物化远程 source、校验物化 manifest，并产生一次命令
生命周期内不变的 crate-name 到 root 映射。Package source 和物理 root 不进入 module
identity。

包管理 Host 使用私有 IMOS 服务物化依赖；业务 TransformService 不获得外部 effect 能力。
run/serve 共用静态方法协议，差异仅在单次输入和持续请求流。

只有模块图节点拥有 module identity 和 `ModuleId`。Telora module 与 static data module
的 canonical source path 通常等于其 module identity；运行上下文 source 等非模块输入
只有 canonical source path。

## 类型概念

### TypeMetadata

**TypeMetadata** 是类型的规范不可变数据表示。类型声明和 metadata constructor
在工具阶段计算它。Static checking、runtime validation、codec、schema、formatting
capability 和用户态 interpreter 可以共享同一份 TypeMetadata。

表面语言用 `T.type` 单向取得元数据数据，结果为 `TypeOf(T)`。类型由声明、结构
构造器和参数化类型族产生；普通函数计算的元数据不能反向成为静态类型。`let` / `def`
绑定数据，`type` 绑定类型；模块接口保留这一身份，不以大小写或元数据内容判定。

### `Type`

`Type` 是有效 TypeMetadata 值的静态 metatype。它证明一个值是有效元数据，但不
保留该元数据描述哪一种 instance type。

### `TypeOf(A)`

`TypeOf(A)` 是静态 metadata witness：其值描述 `A` 的 instance。它可赋给 `Type`，
并在运行时擦除；它不是 dependent function type 或 runtime generic parameter。

### `TypeDesc`

`TypeDesc` 是通用检查 TypeMetadata 时使用的公开擦除视图。它为递归 graph 提供
有限表示，但自身不能恢复一个静态 bound instance type。

### Type Scheme 与 Bound Type

**Type scheme** 是 `for(A) Fn(A) -> A` 这样的 rank-1 contract。**Bound type** 是
检查该 contract 时 `A` 的刚性含义。Scheme 不是普通 `Type` 值。
Bound 身份只在所属 scheme 内有意义，不能按内部编号跨 scheme 比较。模块接口独立
保留权威 scheme；每次调用从 scheme 新鲜实例化，Bound 的关系在所属 scheme 内保留。

### TypeMetadata Family

**TypeMetadata family（类型元数据族）**是由参数化 `type` 声明建立的、可命名的
rank-1 metadata witness 关系。例如 `type Box(A) = ...` 使 `Box(A)` 可以出现在
contract 中，并通过 `Box(A).type` 取得 `TypeOf(Box(A))`。内部仍保留参数化 witness
scheme，但不能将类型族作为接收元数据的普通函数。newtype 的值构造器是独立的可调用能力。

Family 声明以刚性 Bound 参数求值一次并发布符号模板；application 只替换模板中的
Bound，不按 concrete 参数重跑声明 body。这个限制使泛型 contract 与值级结果保持
一致，也让工具能够发布完整、可诊断的 scheme。符号模板保留规范 TypeMetadata；
application 产生的规则节点保留 authored call-site provenance。
Partial recovery 对独立有效的 family 同样发布精确 scheme。

Family 不是任意 metadata function、associated type、trait implementation、
higher-kinded type parameter 或 nominal constructor。它不参与实例搜索，不能作为 type parameter 传递，
也不引入新的 runtime kind 或求值语言。

名义 Struct/Enum family 可以在自己的有限符号模板中以完全相同的 Bound 参数建立直接
回边。参数变换、mutual family recursion 和无生产 alias 不属于该能力；它们需要一般
递归 type-function 归一化，当前不支持。

### `Dyn`

`Dyn` 是狭窄的 existential package，保留值、权威类型关系和 provenance。投影
需要 type witness，并执行权威校验。`Dyn` 不是 unchecked cast，也不是
polymorphism 的通用替代品。

### `Never`

`Never` 是公开契约中可见的无居住者（bottom）静态类型，表示一条路径不产生普通值。
`return`、`fail!` 和 `panic!` 等终止路径可以得到 `Never`，使 directional checking
不必为不可达结果伪造类型，并抑制连锁错误。用户不能把 `Never` 构造成普通数据；
初始化缓存中的失败状态也不是 `Never` 的源码可观察实例。

### Enum variant 的值物化

enum variant 的身份属于类型域。在值表达式位置，它物化为无载荷值或有载荷构造器，
值来源从这处表达式开始；类型定义、导入和重导出身份不形成值来源链。
普通绑定已经持有值，其后续引用、传参、返回保留原来源。构造器调用产生的 enum
外层继承构造器来源，payload 保持输入来源。定义导航与值来源是两种独立关系。

### Decorator 与 Typed Property

**Decorator** 是在 tool stage 为一个已经封闭的具名类型或其 member 计算 typed
property 的函数。目标的 TypeMetadata、TypeId 和 canonical member index 在 provider
运行前已经封闭；provider 从只读 context 计算并返回 property value。类型骨架和
property registry 是两个独立的数据域，协议与执行顺序保证目标的结构和身份稳定。
Property carrier 必须是由
`@property(PropertyTarget.Type)` 这类标记修饰的具体具名类型；参数是具名 enum
`PropertyTarget` 的值，其成员为 `Type`、`StructType`、`EnumType`、`Member`、
`Field` 和 `Variant`，多个标记按位合并。参数在工具阶段求值并检查类型身份。

系统使用 `Ty(target, property)`、`Field(target, canonical_index, property)` 或
`Variant(target, canonical_index, property)` 作为键并发布到 MainWorld。相同 key 的
provider 接受 `Option(previous)` 并按词法顺序 fold。字段/variant provider 先运行，
type provider 后运行并可查询完整 member-property snapshot。Interpreter 只通过
TypeId 和 member index 查询，不使用字符串属性名。

当前 decorator 只适用于无类型参数的具名 Struct/Enum 声明及其直接 member；alias、
结构类型和 type family template 不接受 decorator。

Interpreter、静态 trait implementation 和未来的 quote/codegen 都可以把封闭类型
骨架与独立的 typed property registry 作为稳定输入。

### Trait 与 Static Evidence

**Trait** 是由 canonical `TraitId` 标识的 nominal 静态 capability。Trait member
给出以 `Self` 表示接收类型的函数 contract；**impl** 为具体类型或带静态约束的类型
模式提供完整 dictionary。Coherence 和 orphan boundary 使封闭模块图中的候选唯一。

**Static evidence** 是编译器已经证明某个类型满足 trait 或 `Property(P)` 的事实及其
实现数据。受约束的 rank-1 scheme 保留 canonical identity，dictionary elaboration
把 evidence 作为隐藏参数传给普通函数。Evidence 不产生 runtime trait object；VM
执行普通 closure、Dict 和 property payload，也不进行运行期 implementation search。

`Property(P)` 把成功发布的 `Ty(T, P)` property 提升为静态约束。普通反射仍返回
`Option(P)`；约束只证明 property 存在，不根据 payload 内容选择 implementation。

### Interpreter

**Interpreter** 是消费 `TypeDesc` 和 typed property 并实现类型导向操作的普通代码。
受控的 typed bridge 可以把它连接到 `TypeOf(A)`，但不能允许它任意构造或提取 `A`。
工具阶段的 interpreter 可以产生捕获 canonical member index、静态 evidence 和普通常量
的普通 closure，并把 closure 作为 typed property payload 发布。运行期消费该 payload
不需要重新查询 property registry。

`interpreter!` 是受信任的高阶适配器：构造时求值并捕获输入函数，以封闭类型生成参数
包装。工厂每次调用产生普通闭包，不缓存 wrapper、不回写原环境；函数身份及环境共享
遵循普通闭包规则。见 RFC 0301。

## Stage 与 Execution

### Tool Stage

**Tool stage（工具阶段）**计算发现 type、metadata、contract、decorator、module
interface 和 semantic fact 所需的闭合计算。它按依赖范围执行，并支持保守 recovery。

### Program Stage

**Program stage（程序阶段）**使用封闭 module graph 和显式 Host input，执行普通
应用转换。

两个阶段共享求值语义。“工具阶段”不是另一门 type-level language。

### Fuel 与 Quota

**Fuel** 限制语义执行进度，尤其是 call 和实际执行的 back edge。**Quota** 还限制
stack、call depth 与 allocation。它们使 Host 获得有限终止边界，而不要求每个程序
都是 total function。

Fuel 的定位是对抗执行能否收敛的不确定性，防止递归和重复控制流失控，不是精确
计费。它不衡量 CPU 时间、指令条数、复制字节数或 native 实现内部的操作次数。
内存与输入规模等风险由独立配额约束，不能用 fuel 代替。

### Persistent World 与 Temporary World

**Temporary world（临时世界）**保存推测性或调用局部工作；**persistent world
（持久世界）**保存成功发布的 module value 和 metadata。二者之间的 promotion
受控且原子；持久值不能引用已丢弃的临时状态。

## Feedback

### Diagnostic

**Diagnostic（诊断）**是面向人、Agent 或 Host 的来源可追踪观察，具有 severity、
message、location、label 和 provenance。它不只是 String，也不一定是程序返回值。

### Blame

**Blame** 把被拒绝的 subject 与拒绝它的 rule 或 contract 关联起来，并跨 boundary
failure 保留 authored origin 和 transformed origin。

### Recovery

**Recovery** 在损坏之后继续分析或独立计算，以保留仍有依据的 fact 和 diagnostic。
Recovery 不会把失败结果变成可发布的成功结果。

### Semantic Fact

**Semantic fact（语义事实）**是与 revision 绑定的工具观察，描述 source entity 或
expression。其状态至少区分：

- known information；
- unknown information；
- conflicting constraint；
- dependency blocking；
- tool-stage incomputability。

不能仅为了简化 completion 或 display 就合并这些状态。

### `Option`、`Result` 与 Host-Observed Diagnostic

这些机制具有不同含义和所有者：

| 机制 | 含义 | 处理者 |
| --- | --- | --- |
| `Option(T)` | 预期内的缺失或可选证据 | 普通 Telora 代码 |
| `Result(T, E)` | 显式 value-level boundary outcome | 普通 Telora caller |
| `warn!`、`ok_or_warn!` | Warning，返回 Option(T)；失败分支为 None | VM 记录、Host 观察 |
| `raise!`、`unwrap!`、`fail!` | 当前结果不能产生；保留结构化原因和显式 subject 来源 | VM 与 Host |
| `panic!` | 实现不变量破坏，不是普通领域拒绝 | VM 与 Host |
| `dbg!` | 不影响值与资源核算的 Host-only observation | Host observer |
| `rt.with_diagnostics` | Entry 对一次调用建立可恢复诊断作用域 | Entry orchestration |

结构化 failure diagnostic 的核心是 `rule + data_sources`。rule 包含拒绝消息与规则
应用位置；data sources 是显式 subjects 的有序来源位置。函数边界内触发的 contextual
failure 把 rule 归因到最外层 authored caller，内部 `fail!` 位置只保留为实现 trace。
Host 如何把这些位置显示为 primary/secondary 属于呈现策略。失败在初始化依赖之间
传播时继续引用原 root diagnostic，不增加新的根因。

`rt.with_diagnostics` 把一次调用的成功值与 Warning 作为 `Ok((value, diagnostics))`
返回，把可恢复 failure 作为 `Err(diagnostics)` 返回并消费这些诊断。资源耗尽、取消与
其他终止性 runtime failure 仍向外传播。

Best-effort 是 check 在多个初始化根之间采用的策略，单个根失败立即中断，不是
源码中的“报告后继续” intrinsic。它可以帮助 Host 一次观察更多根因，但任何 Error
仍阻止 candidate artifact 和 effect 发布。完整表面与传播规则见
[`LANGUAGE.md`](LANGUAGE.md#9-来源失败和诊断)。

## Agent

### Agent

**Agent** 是外部作者或修复参与者，可以生成 Telora source、static data 或 intent。
它在语言内部没有特殊语义地位。

### Harness

**Harness** 是由 Host 控制的过程：限制 Agent 输入，调用 Telora 分析或执行，返回
公开诊断，校验 artifact，并控制下一步或真实效果。

Harness 拥有 observation、persistence、retry、approval 和整个 loop 的预算；
Telora 只负责其中一次封闭、确定的计算。

### Repair Loop

**Repair loop（修复闭环）**中，作者收到来源可追踪的诊断，并修改 intent 或代码。
它由 Host 控制；稳定的输入、诊断、源码和输出身份使整个过程可以审计。

## 必须区分的概念

| 不应混淆 | 区别 |
| --- | --- |
| Intent 与 artifact | Intent 提出请求；artifact 是完整的 lowering 结果。 |
| Artifact 与 effect | Artifact 是数据；只有 Host 可以执行效果。 |
| Validation 与 lowering | 两者概念不同，但经常必须在同一个过程完成。 |
| `Type` 与 `TypeOf(A)` | 前者证明元数据有效；后者保留它描述什么。 |
| 类型与 unknown | 类型描述值的契约；unknown 描述分析尚未取得依据的 fact state。 |
| `Dyn` 与 cast | `Dyn` 保留经过检查的 existential evidence；cast 会绕过它。 |
| Diagnostic 与 `Result` | Diagnostic 是 Host-observed feedback；`Result` 是普通值协议。 |
| Recovery 与 success | Recovery 保留信息，绝不自动授权发布。 |
| Entry 与 Main | Entry 属于 Host 边界；Main 是封闭的纯程序。 |
| Domain library 与 standard library | 领域 vocabulary 可复用，不等于语言通用。 |
| Experiment 与 product semantics | 实验提供证据，不定义语言核心。 |

这些区别约束未来 RFC。每项提案都应指出它修改了哪个概念、该概念属于哪一层，
并证明现有层次为什么无法忠实表达需求。
