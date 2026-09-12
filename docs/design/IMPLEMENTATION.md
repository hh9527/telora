# Telora 当前实现架构

本文描述当前 main 的编译器、运行时、模块系统与 Host。语言可观察语义以
[LANGUAGE.md](LANGUAGE.md) 为准，术语以 [CONCEPT.md](CONCEPT.md) 为准。
Rust 类型名、文件名和内存布局是实现事实，不构成公开 ABI 承诺。

RFC 0280 的整图 MIR 流水线已通过 #176 合入 main（`a408c68`），批量检查随后在
`cb0e23b` 落地。历史迁移过程和测量保留在 RFC 中；本文只描述当前路径。

## 1. 总体管线

所有编译入口共用同一个 session MIR：

```text
workspace/package 清单 + 所选根模块
  -> module-resolve：模块图、可达 CST、扁平 HIR
  -> symbol-resolve：符号与引用槽闭合
  -> type-resolve：全图类型槽、泛型实例与静态证据闭合
  -> SealedMir
  -> codegen：bytecode + TypeImage + ExecutionGraph
  -> 链接 native 与数据模块
  -> VM：安装 TypeImage，注入数据
  -> 一个 Initialize WorkWorld：求值全部初始化根
  -> 一次发布到 MainWorld
  -> 新 WorkWorld：eval 调用、Test 或 Entry 调度
```

前三个 Pass 不持有 VM 或运行时 heap，不执行 Telora 代码。Query/LSP 可以读取未成功
seal 的 MIR；执行入口必须通过 seal。后续阶段直接使用静态结果，不重新 resolve 名称、
推断类型或通过求值补全类型骨架。

主要源码入口如下，core 路径相对于 `crates/telora-core/src/`：

| 层次 | 当前实现 |
| --- | --- |
| grammar、CST、parser | `syntax/telora/`、`parser.rs`、`ast.rs` |
| session 图与 HIR lowering | `mir.rs`、`mir/lower.rs`、`module-resolve.rs` |
| 符号、类型求解 | `symbol-resolve.rs`、`type-resolve.rs` 及其子目录 |
| 封闭与只读查询 | `mir/seal.rs`、`mir_query.rs` |
| 代码与运行时静态数据 | `codegen.rs`、`bytecode.rs`、`type_image.rs`、`execution_graph.rs` |
| 链接 | `execution_link.rs` |
| 初始化、需求求值与发布 | `vm/solved-check.rs`、`vm/demand.rs`、`heap/publish.rs` |
| 运行时、值与复制 | `vm.rs`、`heap.rs`、`heap/value.rs`、`heap/copy.rs` |
| package 与 Host 契约 | `package.rs`、`runtime_host.rs` |
| CLI 静态输入与编辑器快照 | `crates/telora/src/static_input.rs`、`crates/telora/src/mir_workspace.rs` |
| CLI 命令消费者 | `crates/telora/src/{static_cli,eval_cli,test_cli,main}.rs` |

codegen 直接消费 SealedMir 并生成寄存器 bytecode。仓库中仍有 `lir.rs`，但当前主流程
不经过旧的 elaborated AST / compiler 管线。

## 2. Frontend 与静态诊断

parser 保留 lossless CST、恢复后的语法和诊断。module Pass 将可达源码挂入 MIR，
记录源码有效性，并分配扁平 HIR 节点及相应 resolve/type 槽。源码不完整也能产生可查询图，
但不能因此获得执行资格。

模块状态包括 `Unloaded`、`Source`、`Data` 和 `Unavailable`。符号求解结果包括
`Bound`、`Unresolved` 和 `Conflicted`；冲突区分重复定义、多个 import 候选等。
`ResolveState::Member` 表示已交给类型阶段的成员约束，不是遗留的词法名称查找。

类型求解中的槽状态为：

```text
Unknown
ProxyTo(TypeSlotId)
Structure(TypeTermId)
Known(TypeId)
Conflicted(TypeConflictId)
```

冲突发现时记录证据与诊断，最终归一化后记录仍未确定的必需槽。Unresolved/Conflicted
是前一阶段的权威结果；后续阶段继续处理独立信息，不回退到另一套解析或推导器。
静态阶段的诊断积累不采用 VM 的失败恢复语义。

源码位置从 CST/HIR 保留到 bytecode 和运行时值。逻辑模块名用于诊断，物理路径由 Host
单独保存。CLI JSONL 使用 1-based line、0-based UTF-8 byte column；LSP 根据客户端协商
编码转换位置。

## 3. 模块图、骨架和静态身份

CLI 的 `package_host` 先准备 `ResolvedWorkspace`：发现 workspace、校验 lock 与 crate
manifest，并完成需要的 package 安装。解析器和 VM 不执行 package acquisition，也不
隐式重写 lock。package preparation 与应用 EES service 是两个独立的 Host 生命周期。

`static_input::Inventory` 从所有可用模块名称建立清单。module Pass 先排序清单并分配
ModuleId，再从所选根逐级读取可达源码。共享依赖只读入并解析一次，未到达模块保持
Unloaded。完整 inventory 的身份分配与源码读取、初始化顺序无关。

`telora-crate.json` 的 modules 是源码与静态数据模块的权威清单。未声明文件只能产生
warning，不能成为隐式 import 候选。测试选择额外递归建立当前 crate 的 `tests/` 清单，
拒绝 symlink；测试模块可相互导入，普通源码不能反向导入测试。

数据模块在静态阶段只有编译器生成的接口：

```telora
import "std/value" { Value };
decl data: Value;
export { data };
```

数据内容在静态阶段不读取、不解析；因此类型检查成功不代表 JSON/YAML/TOML 内容有效。
实际内容在链接和 VM 数据注入阶段接受格式与 DataLimits 检查。

symbol Pass 先索引模块的声明、导出和作用域，再闭合引用。import * 建立搜索范围，
具体引用才选择绑定；显式绑定与遮蔽按普通名称解析规则处理。内置类型的特殊身份来自
native 声明的 NativeTypeId，不能根据 Int、Array 等拼写识别。默认的
`import "std/prelude" *;` 提供普通名称；`@property` 等装饰器也遵循这些绑定规则。

MIR 的模块、符号和类型身份都是本次完整构建中的索引。不要把旧 module/package API
的 ID 编码或预留区间套用到 MIR 的 ID，也不承诺源码改变后数字保持不变。

## 4. 全图类型求解与 SealedMir

MIR 拥有 HIR、resolve_slots、ty_slots、结构类型项、最终类型表及各种证据表。源码节点
在 lowering 时得到槽位；泛型实例等辅助槽在求解时按需增加。求解器通过相等、适配、
调用和成员等约束填空，合并代理根，再完成结构类型归一化。分支不复制整份模块类型环境。

`TypeTerm` 的参数仍可指向未知或代理槽；最终 `ResolvedType` 的参数都是 TypeId。
名义实例保留声明身份及类型实参。递归、泛型、部分应用和隐式类型实参的证据也进入图，
不能把尚未闭合的泛型调用留给 codegen 猜测。

`Mir::seal` 检查必需类型槽、泛型实例、成员选择、类型布局、构造检查、property 与 bound
证据是否完整。成功返回只读借用 `SealedMir` 和独立的 TypeImage；seal 不重新编号。
失败保留原 MIR 和诊断，供 query/LSP 使用。

入口执行另有 `Mir::seal_export` 发布的 `SealedExecutable`：它保留 TypeImage，
并封闭所选导出的值依赖、具体实例及元数据初始化集合。未实例化模板仅属于静态
声明图；进入执行集合的类型必须具体化。native 入口和模块检查都消费这一发布
能力，分别由 `seal_export` 和 `seal_modules` 确定执行范围，不在 codegen 中重建
依赖闭包。默认 bytecode 入口仍使用原来的 `SealedMir` 接口。

相同完整输入应产生确定的 MIR ID、TypeImage、bytecode 和执行图，不受 inventory
枚举顺序影响。这是完整构建的确定性，不是跨版本或增量编辑的永久 ID 保证。

TypeImage 是扁平只读数组，保存类型、已应用布局和类型定义。其索引对应静态 Pass
分配的 TypeId。VM 安装该图后，TypeDesc、codec、Dyn 与类型元数据操作查询已有类型
和布局；它们可以物化元数据值，但不能分配新的推导槽或求值生成类型骨架。

`T` 是静态类型，`T.type` 生成带精确见证的元数据值。普通函数返回的元数据不能反向
成为静态类型声明。Fn 和 tuple 的语法 lowering 与普通内置名称绑定保持区分；
`()` 复用空 Tuple，Unit 是其别名。

## 5. codegen、构造校验与运行时表示

codegen 的公开编译入口接受 SealedMir。表达式类型、泛型实例、模式构造器和成员选择
来自已完成的证据表；生成闭包、调用和构造代码不再启动类型推导。

`@check` 的静态阶段确定校验器签名与构造目标；校验函数在 VM 中执行。具名字段 struct
接收 Unchecked(T)，newtype/带载荷 enum variant 接收载荷，返回 Result((), BlameError)。
普通构造拒绝产生运行时失败，codec 解码拒绝返回 Err。读取或复制已完成的值不会重新
执行构造校验；新构造与 `<~` 更新会检查其结果。

Struct 暂仍复用 Dict 的字段表示。merge-update 根据左侧已解出的具名类型和字段契约
生成代码，新建外层容器并复用保留字段的 Val；projection 同样消费静态字段证据。
它们保留嵌套值的身份和来源。当前没有将全部 field access 降为固定偏移量。

寄存器和对象字段使用 32-byte Val，包含来源、表示标签、类型标记和 payload。
静态 TypeId 与底层表示不同：具名 newtype 使用单元素 Tuple，enum 使用内部 Atom/Tagged
表示。内部表示标签不重新成为公开的表面类型。

类型元数据可以用 `SolvedType(TypeId)` 表示。复制器验证其属于同一个 session TypeImage，
直接复用身份，不递归重建类型描述符。复合数据和闭包仍由 scoped handle 指向 heap；
不能据此宣称所有运行时值都已消除深复制或引用计数。

typed equality 使用类型身份及对应值表示，来源位置不参与相等；Dyn 的投影与 codec
通过已确定的见证检查契约。动态值检查属于运行时行为，不是重新推断表达式类型。

## 6. Property、MainWorld 与 WorkWorld

静态 property 记录说明某个 owner/member 是否具有特定 carrier，以及 provider 的
签名和来源。判断 HasProperty 不需要执行 provider。property 内容则是运行时值，
不属于类型骨架。

ExecutionGraph 从 SealedMir 建立全局值、泛型实例、property fold 和构造检查等任务。
property 使用 TypeId、carrier TypeId 和 member/site 身份建立键。同一键的 provider
按既定顺序 fold，最终只有一个有效结果；不同成员仍是不同键。

VM 持有可变求值状态表，任务经历 Pending、Running、Ready 或 Failed。需求指令读取
已完成的 Val，或启动目标任务；再次请求 Running 节点报告依赖环，Failed 传播已有失败。
codegen 只生成利用这张表的代码，不与 VM 共用可变推导状态。

初始化时先安装 TypeImage 和执行图，再注入数据。整张可达图使用一个 Initialize
WorkWorld，主动完成全部初始化根；顶层值与 property 的相互依赖通过需求求值处理。
初始化不调用普通函数的函数体，除非某个初始化计算实际调用它。

完成后 `freeze_initialized_world` 验证所有初始化根 Ready，再用一次多根复制把全局值与
property 结果发布到 MainWorld。共用 forwarding map 保留跨根共享、闭包引用及来源，
Main 对象不能引用即将释放的 Work storage。失败或未完成任务不能作为成功初始化发布。

之后创建新的 WorkWorld。执行读取 Main 中的已完成结果，不在新的 WorkWorld 中重新
启动初始化。Work-to-Work 的值迁移也使用根驱动复制；已有 Main 引用可直接保留。
外层结构仍会分配，普通值图复制的成本没有在本次架构改造中全部消除。

## 7. 诊断、失败与发布

静态诊断由三个 Pass 和 seal 产生。无静态执行，所以 Unknown/Conflicted 不等于一次
运行失败，也无需通过重建 VM 或重跑旧求解器恢复。存在静态错误时不进入初始化。

运行时失败保留规则位置和数据来源；`raise!` 产生 Never，`warn!` 产生值为 None 的
Option(T)。`blame!` 构造错误数据，规则归因由调用/构造边界与 VM 诊断逻辑共同保留。
`@check` 的 Err 路径不会把失败候选发布成合法 T。

初始化的 best-effort 可以继续独立任务，失败依赖传播已有错误；资源、取消或一致性等
终止错误仍中止 session。没有任何 error 才能把 session 当作成功对外输出。这个保证
针对 session 结果，不是外部 Host 已执行 effect 的事务回滚。

源码名与字节范围保留在 VM debug origins 和值来源中。fixture、eval/run 输入等来源
由 Host 注册，物理文件定位不授予语言额外文件访问权限。

## 8. 资源与 Host 边界

VM 的 QuotaAccount 核算 fuel、栈与分配，并携带诊断和取消上下文。静态类型求解不
消耗 VM fuel；这不表示解析、求解或编辑器请求没有资源与取消约束。

Fuel 是约束失控执行的机制，不是精确计费器。实现应在调用和实际执行的回边等
动态扩展点保留检查，不必为每条指令、每次复制或 native 库内部的每一步建立账目。
验收重点是递归、循环及 callback 重入不能绕过预算，耗尽后正确传播终止失败并
保留来源。Native 操作自身的终止性、输入限制和内存配额需要独立保证；不应仅为
获得精确 fuel 计数而重写已有库算法。

数据入口单独使用 DataLimits 检查文件大小、节点数、深度、单容器成员数和 payload
大小。通过 admission 后才物化 Value。运行时 codec/parse 仍受 VM 配额约束。
错误消息和来源应保留，但配额的具体数值是 Host 配置，不是语言语法。

`telora-ees` 组合 IMOS 与 sqlite-query 等 native actor components。package preparation
使用自己的 Service，run/serve 根据应用配置另建 Service；core 仅依赖 component-neutral
Host ABI。实际文件、环境、stdin 与 EES 调用由 Host 执行，纯 Telora 代码不能直接访问。

run/serve 的生成 adapter 与应用在同一 MIR 中求解，wrapper family 和具体泛型实参
在静态阶段闭合。初始化完成后才进入资源协商和 Entry 调度；Host 根据声明的 capabilities
读取输入并检查 effects。eval/eval-with 不启动 reducer loop 或应用 EES service。

## 9. CLI 与 LSP 的阶段边界

| 命令 | 消费边界 |
| --- | --- |
| query modules | inventory 清单 |
| query exports / at、LSP 语义查询 | 三个静态 Pass 后的 MIR，可保留错误和未知事实 |
| check --only-types | 三个 Pass 与 seal，不读取数据内容或执行 Telora 代码 |
| check | seal、codegen、链接、数据注入及整图初始化 |
| eval | 初始化后取得选中 Value 导出 |
| eval-with | 初始化后调用选中 entry.Eval |
| test NAME | 初始化后执行该测试模块直接导出的 Test |
| run / serve | 初始化后按 Entry 策略调度 |

`check MODULE_ID` 选择一个根。`check --lib` 选择当前 crate 清单里的全部模块，包括
私有模块和数据模块；`check --tests` 递归选择当前 crate 的 tests/ 模块。两个开关
可以组合，与显式 selector 互斥。依赖按导入加入同一张图；不会对每个根重启编译器。
空集合成功，未声明文件仍不进入图。

批量 check 输出一份 `telora.check/v1` summary，roots 列出所选根。独立静态问题可
一起报告，但任一静态错误都会阻止整图初始化，不提供逐模块独立成功/失败 session。
`--tests` 不执行 Test thunk，也不因 Test 描述而读取 fixture。

summary 中 catalog_seconds 是清单准备时间，static_seconds 包含三个 Pass 与 seal，
execution_seconds 包含 codegen、链接与 VM 初始化；only-types、静态失败或空根集合时
execution_seconds 为零。check_seconds 是 static 与 execution 之和，不含 catalog。
这些是阶段观测，不自动构成不同版本的性能比较。

test 的每个 thunk/factory 在运行时检查自己的结果；可恢复预期失败由测试断言消费，
终止错误中止 runner。Host 负责 fixture 定位和限量读取，测试结果使用
`telora.test/v2`。成功的 check 不代表测试断言已运行。

LSP 的 `mir_workspace` 把文档覆盖内容和磁盘清单送入同一静态流水线，快照拥有 MIR。
查询通过 MirQuery 返回确定的信息与诊断，不从 CLI 文本反推语义，也不持有 VM。
取消或过期快照不能发布成最新结果。当前不承诺每次编辑只重算最小依赖子图。

## 10. 维护不变量与验证入口

- 名称解析只做一次，类型阶段接受其 Bound/Unresolved/Conflicted 结论。
- 类型推导只在静态阶段完成；codegen 与 VM 消费完整证据，不补猜类型。
- seal 不隐藏未知、冲突或遗漏的泛型/构造证据，也不重新编号。
- native 特殊身份来自声明的稳定标识，普通名称受 import、遮蔽和作用域规则约束。
- 类型骨架不依赖 property 值，数据内容不进入静态求解。
- 初始化覆盖整图并统一发布；共享和来源跨 World 复制后保持正确。
- 构造校验覆盖新的合法值边界，不能用跳过检查换取性能。
- query/LSP 可观察失败图，执行入口只能接受成功 seal 的图。

验证入口包括三个 resolve 模块的单元测试、`codegen/tests/` 的确定性与执行测试、
`vm/tests/demand.rs` 的初始化/共享测试，以及 `crates/telora/tests/cli.rs` 和
`tests/language/`。语言规则与诊断回归优先使用 Telora 用例，检查成功、拒绝、来源、
泛型实例以及构造/解码/更新边界。

完整构建确定性测试比较不同 inventory 顺序下的 MIR dump、TypeImage、bytecode、
native links 和 ExecutionGraph。运行时布局分离、字段偏移量 codegen、native backend
和进一步减少值复制仍是后续工作，不能描述为本轮已经实现。
