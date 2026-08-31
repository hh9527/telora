# Telora 当前实现架构

本文档描述当前语言设计如何落到编译器、运行时、模块系统和 Host 中。它是实现架构的
SSOT，帮助维护者在不回看 RFC 的情况下建立当前实现模型。

语言可观察语义以 [`LANGUAGE.md`](LANGUAGE.md) 为准，稳定术语以
[`CONCEPT.md`](CONCEPT.md) 为准。本文出现的 Rust 类型名、文件名、数字布局和处理阶段
是当前实现事实，不自动构成公开兼容性承诺；如果实现改变但语言语义不变，应更新本文，
而不是把私有结构提升为语言概念。

## 1. 总体管线

当前实现只有一门 Telora 语言，但为严格执行和工具 recovery 保留不同的消费路径：

```text
SourceDatabase 中的 revisioned source
  -> lossless CST + syntax diagnostics
  -> 严格 AST / recovered program
  -> HIR name resolution
  -> type analysis + semantic facts
  -> elaborated AST
  -> register-oriented LIR
  -> bytecode
  -> register VM + QuotaAccount
  -> WorkWorld
  -> 校验后原子晋升到 MainWorld，或整体丢弃
```

源码位置从 parser 一直保留到 bytecode debug origin、运行时值和诊断。工具查询使用
workspace snapshot 中的 source、定义、引用、类型图和诊断，不从 CLI 文本输出反向解析
语义。

`SourceDatabase.name` 保存 canonical source path，而不是物理文件名。workspace module
同时在 Host 私有结构中保留 resolver path，LSP 由 source id 反查该结构后生成 file URI。
运行上下文 source 以 `@run-ctx/<percent-encoded-key>` 注册；CLI Host 另存从这个公开名字
到文件或 stdin locator 的私有映射。该 source 不进入 module graph，不分配 `ModuleId`，
也不参与 import resolution。文件读取错误、provenance 和普通诊断只公开 canonical
source path。`eval-with` 使用相同机制，但 canonical 前缀为 `@eval-ctx/`；它在目标执行
共享的 `SourceDatabase` 中完成格式验证和 Value 物化。

主要实现入口是：

| 层次 | 当前实现 |
| --- | --- |
| Telora grammar/lexer/CST | `syntax/telora/grammar.llw`、`syntax/telora/` |
| CST 到 AST/recovery | `parser.rs`、`ast.rs` |
| 名字解析 | `hir.rs` |
| 类型分析与 partial facts | `types.rs`、`semantic.rs` |
| elaboration、LIR、bytecode | `elaboration.rs`、`lir.rs`、`compiler.rs`、`bytecode.rs` |
| VM、配额、失败 | `vm.rs`、`evaluation.rs` |
| 值、heap、复制 | `heap.rs`、`value.rs` |
| typed property registry / 反射 bridge | `property.rs`、`heap/`、`types/dependency.rs` |
| workspace/package model | `package.rs` |
| 模块解析与 Host 生命周期 | `module_id.rs`、`module.rs` |
| canonical runtime types | `type_store.rs` |
| CLI/LSP package Host | `crates/telora/src/package_host.rs` |
| CLI Host | `crates/telora/src/main.rs` |

以上路径均相对于 `crates/telora-core/src/`，除非另有说明。

## 2. Frontend 与 recovery

`parse_registered` 总是产生 CST、recovered program 和有序诊断；只有
不存在 frontend diagnostic 时才产生严格 `Program`。因此 CST 可用于损坏文档上的
定位和编辑，而严格编译不会把 recovered node 当成有效程序。

HIR 给 definition 和 reference 建立稳定的分析期身份，并区分本地 definition、external
binding 与 unresolved name。完整分析服务严格编译；partial analysis 则按定义依赖保留
仍有依据的事实。当前公开事实状态为：

```text
Known
Unknown(MissingSyntax | InvalidSyntax | UnresolvedName |
        BlockedBy(fact) | UnavailableDependency)
Conflicted(DuplicateDefinition | IncompatibleContract)
Incomputable(QuotaExceeded | RuntimeOnly | UnsupportedOperation |
             CyclicEvaluation | Cancelled)
```

显式 `Any` 是一个已知静态类型，不是 `Unknown`。Recovery 也不制造“部分成功”的语言
值或特殊 Module 类型：Module 只有源码层面的 `Available` / `Unavailable`，细粒度状态
属于 definition、expression 和 type fact。

严格 AST 经类型分析后先 elaboration，再降低为寄存器 LIR，并组装为 bytecode。静态
annotation 和仅用于元数据计算的 helper 可以在 program bytecode 中擦除；被普通运行时
值引用的 TypeMetadata、函数和 closure 则必须保留。

## 3. 模块图、骨架和静态身份

crate mode 在构造 `ModuleResolver` 前执行 package preparation：向上发现
`telora-config.json`，严格校验 `telora-lock.json`，将远程 tarball 转成确定的 IMOS plan，
通过内嵌 `telora-ees` facade 提交 `InstallShared` 并取得 immutable installation root，
再校验每个 `telora-crate.json`。开发 override 只在 baseline package 与 lock 一致后替换
effective root。准备结果是 `ResolvedWorkspace`，同时供 CLI 和 LSP 使用；resolver、
module loader 和 VM 不执行 package acquisition，也不改写 lock。

Cargo workspace 中的 `telora-ees` 是 Native Actor Components 的组合根，依赖 `imos`
和 `sqlite-query` components。强类型 manifest 在 Service 启动前绑定逻辑 actor name 与
物理构造参数；通用 `Call {id, actor, operation, input}` 只能调度已构造 actor。
`telora` package Host 和应用 RunHost 都依赖该 facade；
`telora-core` 只拥有 component-neutral `EesCall/EesReply` Entry ABI，不依赖 EES、IMOS
或 SQLite 实现。

Package preparation 构造私有 `telora-packages` IMOS Service。`run/serve` 从选中 export
的 `entry.Ees` 单独构造应用 Service。CLI 校验并替换 `--ees-var`，component adapter 把 `user-*:` locator
解析为 XDG/HOME 物理路径；解析结果不进入 Telora World。RunHost 异步 dispatch call，
把终态转为一个关联 `EesReply` event。Engine 与 RunHost 都按 `SystemCaps.ees` 校验 actor name。

`telora-crate.json` 的 `modules` 是 `src/` module 的权威清单。清单项在准备阶段映射并
canonicalize 到物理文件；未列出的文件不会进入 catalog。`tests/*` 只在 Host 选择测试
根时加入当前图。`telora check` 在准备后扫描 `src/`，为未声明文件输出 warning，但不
改变 resolver 输入。

`ModuleResolver` 消费已经准备好的 crate source 清单。`builtin_list()` 先登记 builtin
vendor 的 crate，resolver 随后登记当前 crate 和 manifest dependencies；
同名登记使用 first-win，已选 source 不再改变。import 先按 selector 首段选择 crate，
再只在该 crate 中解析 module，因此 configured `std` 不能补充 builtin `std`。

模块加载不是逐条执行 `import`。Host 先解析根及其全部静态依赖，完成 canonical module
name 解析和图发现，再建立模块骨架：

1. 扫描 Telora module 的 import、显式 export、顶层 `decl` / `def` 和具名 Struct/Enum
   type constructor；JSON、TOML、YAML 与 builtin opaque module 也占据图中节点。
2. 默认 prelude 作为每个非 prelude module 的 open-import edge 加入图。
3. 按 canonical module name 的 UTF-8 bytes 排序，为完整图分配 `ModuleId`。
4. 为顶层递归函数和名义 type constructor 分配确定的模块内 slot。
5. 校验实际加载时看到的 import graph 和 skeleton 没有在扫描后变化。

动态 `ModuleId` 从 16 开始；模块内动态 `FuncId` 和 `TypeConstructorId` slot 从 1024
开始。较低范围保留给匿名或稳定 Host contract 身份；builtin module 与其他图节点一起
按 cname 排序并从 16 分配。具体数字是 Host 与 native contract 当前使用的稳定实现
边界，但用户代码应通过名字而不是数字引用普通模块定义。

`FuncId` 和 `TypeConstructorId` 都是 `(ModuleId, local slot)`。`decl f: ...; def f = ...;`
在骨架阶段建立一个函数 slot，编译后的 `FuncRef` 只携带这个静态身份；定义求值时再
seal 对应函数。普通值没有开放 slot，因而不能用 `decl` 建立任意值循环。

Import edge 指向已发现的模块身份。一个依赖模块只初始化一次，完成的 module export
root 保存在 MainWorld；菱形依赖中的后续 import 复用同一个持久 root，再向使用方建立
binding。当前拒绝模块初始化 cycle，模块内函数和 TypeMetadata 的递归由专用 slot
机制闭合。

普通 Telora module 与 static data module 的 canonical source path 等于 canonical module
name，例如 `my-crate/model`、`my-crate/bin/main` 或 `standalone/main`。嵌入式
builtin ABI source 同样使用 `std/...` canonical name，不通过 synthetic source name
取得额外权限。`@run-ctx/config` 等非模块 Source 不能转换为 module name。

## 4. 分析期类型与运行时类型

实现有三个必须区分的层次：

| 层次 | 用途 | 身份范围 |
| --- | --- | --- |
| `TypeDescriptor` / analysis type graph | 推断、scheme、Bound、错误恢复和接口检查 | 分析期 |
| TypeMetadata value | Telora 中可计算、可来源化的规范类型描述 | tool/program stage 的值图 |
| `TypeId` / `TypeStore` | 已封闭 concrete type 的运行时 canonical identity | MainWorld 构建期与持久 witness |

Bound parameter、inference variable 和 unresolved named type 不能直接 canonicalize 为
`TypeId`。具体类型跨越运行时边界前必须已经消除这些分析期占位符；需要富结构时从
`TypeStore` 或权威 TypeMetadata 读取，不把 analysis ID 当作运行时相等依据。

内建 canonical `TypeId` 当前固定为：

```text
Any = 1, Never = 2, Type = 3, Dyn = 4,
Int = 5, Float = 6, String = 7, Bytes = 8
```

动态 `TypeId` 从 1024 开始。名义实例的 intern key 是
`(TypeConstructorId, Array(TypeId))`；因此同一 constructor 用相同 type arguments
应用任意多次都得到同一个 `TypeId`。结构类型按规范 `TypeShape` intern。递归名义类型
先 reserve identity，再 seal body；若构造失败则 abort pending slot。这使 body 可以
回指已经确定、但尚未封闭的本类型，同时阻止未封闭引用越过发布边界。

参数化 TypeMetadata family 的模板包含 Bound，而不是 concrete `TypeId`。应用 family
时用 concrete arguments 替换模板中的 Bound、重建受影响子图，并通过上述 constructor
key 取得 canonical identity；不会把源码名字当作 intern key，也不会把 family body
作为任意运行时 type function 重复求值。

## 5. `Val`、对象和 heap

VM 寄存器和 heap object 字段统一保存 32-byte `Val`：

```text
PackedLoc { source: u32, start: u32, end: u32 }  12 bytes
Meta { flat kind, heap sub-kind, traits, provenance } 4 bytes
ty: u32                                           4 bytes
narrow: u32                                       4 bytes（保留）
raw payload / scoped handle                       8 bytes
```

`meta` 描述运行时表示，不等于静态/名义 `ty`。例如 Int 和名义 wrapper 可以共享 Int
表示而携带不同 `TypeId`。`narrow` 已预留但当前没有公开 trait/interface narrowing
语义。

Int、Float、短 String/Atom、内建 Atom、native type identity 和静态 `FuncRef` 可以
直接编码在 `Val` 中。Bytes、Array、Tuple、Tagged、Dict、closure、Dyn、Module、
Declared/Symbolic TypeMetadata 和 opaque value 使用 scoped handle 指向 heap object。
handle 的 work bit 让复制器无需间接查询即可区分 Main 与 Work 引用。

Heap 是按 storage scope 管理的对象、text、shape、静态函数、类型 witness 和 typed
property 集合。
String/Atom 与 Dict shape 分别 intern；复合值不可变。Host 对值的观察通过借用式
`ValueRef` 和受控转换完成，不存在一份与 VM 图竞争的 legacy/owned Host value model。

Main heap 的 property registry 使用有序的 `PropertyKey`：`Ty(TypeId, TypeId)`、
`Field(TypeId, u32, TypeId)`、`Variant(TypeId, u32, TypeId)`，值是 MainWorld `Val`。
Tool stage 先封闭具名 Struct/Enum 的 TypeMetadata、TypeId 和 canonical member index，
再执行 decorator provider。provider 从只读 context 计算 property value，结果写入
独立的 property registry；这个单向数据流保持目标 descriptor 稳定。provider 的静态
结果、运行时 `Val.ty` 与 property carrier TypeId 必须一致。
carrier 的 owner 能力由
`Ty(Carrier, PropertyAttr) -> PropertyAttr { bits }` 记录；`PropertyAttr` 自举自己的
TypeId，内部 capability 使用 `u32` 位集。

同 key provider 以 `Fn(Ctx, Option(P)) -> P` 逐个 fold，只保留成功的最终 head。
member property 完成并暂存后才运行 type property，因此 type provider 可读取完整
member snapshot。整个声明的 effective heads 在失败检查后一次复制和提交；失败 fold
不会发布部分结果。`std/type-property` 的 `get_type_prop`、`get_field_prop` 和
`get_variant_prop` 只读取 registry，并返回引用同一 Main 值的 `Option(P)`。

运行时相等先服从 [`LANGUAGE.md`](LANGUAGE.md#33-相等性和顺序) 的 typed equality：
名义值需要 canonical `TypeId` 一致，再按表示递归比较；循环图使用 visited pair 防止
无限递归；函数和 opaque value 使用各自的不透明身份规则。来源位置不参与相等。

`Dyn.project_with` 对目标 witness 和 package descriptor 直接执行
`TypeGraph::decode_persistent + canonicalize`。它先为 declared node 建立 canonical
TypeId，再闭合递归边；不能先降成扁平 `TypeDescriptor`，否则 `Option(Node)` 一类
递归复合 witness 会丢失名义回边并退化成不可 canonicalize 的结构递归。

## 6. MainWorld、WorkWorld 与原子晋升

构建期 `MainWorld` 持有本次封闭模块图的 persistent heap、module skeleton、canonical
`TypeStore`、typed property registry，并在 best-effort 路径持有稳定 failure arena。
模块依赖和静态根成功晋升
后，当前严格运行路径把 persistent heap 封装为只读 `FrozenMainWorld`；运行期所需的
canonical witness 已经在该 heap 内闭合，构建用 `TypeStore` 本身不进入 Frozen API。
所选根模块的执行结果、Entry transition、pure eval 调用和普通调用仍位于 Work heap，并把冻结 Main
作为只读 background。

Work 到 Main、Work 到新 Work 都使用根驱动的 copy collector：

1. 从 module export root、调用结果或显式迁移 roots 开始扫描可达图；
2. 用 forwarding map 为每个 source object、text 和 shape 只分配一个 target identity；
3. 保留已经属于 Main 的 uplink，重定位 Work handle；
4. 复制并 seal 可达静态函数、递归类型 slot 和 canonical type witness；
5. 完整验证 pending graph 后一次 commit。

Forwarding 使共享结构和循环保持共享/循环，也让多 root 复制不会重复对象。复制失败时
pending allocation 不进入目标 heap。普通 Host publication 还会拒绝任何可达 Fail；
模块内部的 best-effort 固化可以保留失败 export，以便下游独立诊断，但只要本轮存在
任何 error root，最终结果和 effect 都不能发布。

Entry reducer 每处理一个 event 后，新的 `(State, effects)` 会连同 reducer closure
一起迁移到新的 WorkWorld，旧 WorkWorld 随后释放。这避免长期状态引用已经丢弃的临时
heap；当前实现每轮都迁移一次，没有可观察的 mutable heap。

## 7. 严格求值与 best-effort

严格执行遇到未处理的 recoverable failure 就终止当前结果；资源、一致性或取消失败
终止整个 session。Best-effort 使用同一 VM 指令和普通值语义，但在 evaluator 中把
可独立的 module binding / computation 建成 evaluation units，并把失败保存在 MainWorld
的稳定 failure arena。

失败 arena 区分 root failure 与 propagation node。复合 diagnostic value 可以暂存
Fail child，以保留形状并继续健康的独立分支；Fail 不是用户可构造或匹配的普通值。
依赖失败的 unit 不执行，独立 unit 继续。传播节点保留有界 lineage，但不产生新的
“类型不对”根因。

`RuntimeError` 为 contextual failure 保存一个 rule location、有序去重的 data source
locations，以及可选的 intrinsic implementation location。执行帧同时携带最外层
authored rule boundary；普通调用继承该边界，第一次调用建立边界，tail call 显式搬运
边界，native continuation 回调使用其 authored call site。`Raise` 读取 opaque
`BlameError` carrier 后一次建立 root diagnostic。failure arena 的传播节点只保存 root
failure id，因此 strict 与 best-effort 不会产生两套归因路径。

Entry runtime 的 `with_diagnostics` 使用 native continuation 在同一 WorkWorld 中调用
目标 closure。continuation 记录 `QuotaAccount.diagnostics` 的起点；成功或可恢复失败
时只取出并消费该区间。可恢复 `Raise` 尽量沿用原 `BlameError` carrier，其他可恢复
runtime error 按相同的 `rule + data_sources` 结构重建。terminal failure 不进入该
continuation 的 catch 路径。

Best-effort 不是另一套成功语义。没有 error 时，它与严格执行同属成功并产生相同
可观察值；存在任意 root error 时，即使某个最终表达式可算出，也没有可发布结果。
`check` 使用这条恢复管线；`run --best-effort` 在 Entry 启动前做诊断求值。

## 8. 资源核算

VM 的 `QuotaAccount` 同时核算 fuel、stack slots 和 requested allocation bytes，并携带
诊断与 cancellation query context。当前 VM 另有固定最大 call depth 1024 和 stack
slots 1048576；Host 提供的 quota 可以进一步收紧边界。Module/tool 初始化与 session
执行使用独立 account，debug sink 不消耗 Telora fuel 或 allocation。

静态数据和 Entry 声明的数据源不按可递归 VM 计算核算 fuel。它们在解析/规范化前后
按独立 `DataLimits` admission：原文件大小、逻辑 node 总数、深度、单容器成员数、
单 Bytes 长度、单 String/key/temporal UTF-8 长度，以及全部 decoded payload bytes。
通过 admission 后才把规范 Value 图物化到相应 World；运行时 codec/parse 仍属于普通
VM 计算并按 VM allocation 核算。

## 9. Entry wrapper 与 CLI Host 生命周期

普通模块通过 `std/entry` 构造 `Eval`、`Run(State)` 或 `Serve(State)` 名义值。CLI 解析
`MODULE:EXPORT` 并检查 wrapper family；resolver 不赋予目标模块额外执行身份。内置工具
Entry 实现以下私有 ABI：

```text
config: Fn(Env, MainType) -> Tuple([SystemCaps, Initializer])
Initializer: Fn(SystemResources, MainType) -> Tuple([State, Reducer])
Reducer: Fn(State, SystemEvent) -> Tuple([State, Array(SystemEffect)])
```

文件 stem 以 `_` 开头的模块由 resolver 统一控制，只允许同 crate 模块访问。内置工具
Entry 属于 `std` crate，可以访问 `std/_...` 协议模块。只有内置 `std` crate 的模块编译
时具有 native authority。

当前 Host 顺序是：

1. 发现并执行普通 module，按工具要求选择 export 并验证 `Eval` / `Run(State)` /
   `Serve(State)` 名义 family；
2. 解开 wrapper payload，在准备 WorkWorld 中检查并执行内置 `config(env, main)`；
3. 解析 `SystemCaps`，由 `RunHost.configure` 一次性确认 data/env/stdin 与 EES 诉求；
4. Host 按 caps 读取并校验资源，由私有 runtime bridge 在 Entry WorkWorld 中构造
   `SystemResources`，再与 wrapper payload 一起传给 Initializer；
5. 发送 `'Initialize` 及后续 stdin/EES event，每次调用 reducer；
6. Host 先完整解析和审计一批 SystemEffect，再执行第一个 effect。

应用不直接读取 open world。wrapper 声明 capabilities；实际文件、环境、stdin 和 EES
调用由 CLI `RunHost` 执行。module graph 在应用求值前封闭。

CLI 的公开命令有 `eval`、`eval-with`、`run`、`serve`、`lock`、`check`、`query`（别名
`q`）和 `lsp`。`eval` 选择一个 module 的 `Value` 导出；`eval-with` 选择一个
`entry.Eval`。两条 pure eval 路径直接执行 module 与普通调用，不初始化 reducer loop、
RunHost 或应用 EES。

`run` 固定选择 `std/_entry-default`，`serve --bind stdio://` 固定选择
`std/_entry-serve`。目标 export 分别是 `entry.Run(State)` 与 `entry.Serve(State)`。
wrapper 的初始化函数返回具体 State 和 reducer；标准 Entry 边界将 State 擦除为 Dyn，
并保存一个接受 `(Dyn, Event)` 的 reducer wrapper。Entry 将 application
`EesCall` 映射成 component-neutral SystemEffect，将相关 Host reply 映射回 `EesReply`；
是否声明 EES model 不改变 reducer 接口。
`check`、`query` 和 `lsp` 当前是
Host 固定工具路径，不通过用户 Entry ABI。

## 10. 维护不变量与验证入口

修改当前实现时至少保持以下不变量：

- CST recovery 不得把未知或冲突伪装成 `Any` 或成功值；
- module identity 和静态 slot 不依赖文件发现、HashMap 或求值顺序；
- concrete nominal identity 只由 constructor identity 与 type arguments 决定；
- Main 中的持久对象不能引用已释放的 Work storage；
- copy/promotion 对共享、循环、closure、type witness 和 provenance 保持闭合且原子；
- Fail 传播不产生二次根因，任何 error 都阻止最终 publication；
- Entry capability negotiation 发生在 effect 执行前，Main 仍是封闭纯计算；
- actor transition 显式返回完整 State，Event 与 Effect 不包含 callback 或 continuation；
- pure eval 不创建 Entry、RunHost 或 application EES；
- `query` / JSONL 的位置默认是 1-based line、0-based UTF-8 byte column；LSP 单独按
  客户端协商的 position encoding 转换。

主要可执行证据位于各实现模块的单元测试和 `crates/telora/tests/cli.rs`。其中
`module.rs` 覆盖模块图、类型 family、静态数据、Entry 和 recovery；`heap.rs` 覆盖
值布局、相等、循环复制与 promotion；`types.rs` 覆盖推断、scheme 和 partial fact；
`evaluation.rs` 覆盖 best-effort unit 与 Fail 传播；CLI 集成测试覆盖命令、JSONL、
退出状态和位置协议。设计变更应同时更新相应 SSOT 和至少一处可执行证据。
