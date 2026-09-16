
本文描述当前源码的编译器、运行时、模块系统与 Host。语言可观察语义以
[LANGUAGE.md](LANGUAGE.md) 为准，术语以 [CONCEPT.md](CONCEPT.md) 为准。
Rust 类型名、文件名和内存布局是实现事实，不构成公开 ABI 承诺。

整图 MIR 流水线延续 RFC 0280；RFC 0292 将执行统一为 Wasm。历史迁移过程和
测量保留在 RFC 中；本文只描述当前路径，最终验收状态见 RFC 0292。

## 1. 总体管线

所有编译入口共用同一个 session MIR：

```text
workspace/package 清单 + 所选根模块
  -> module-resolve：模块图、可达 CST、扁平 HIR
  -> symbol-resolve：符号与引用槽闭合
  -> type-resolve：全图类型槽、泛型实例与静态证据闭合
  -> SealedMir / SealedExecutable：类型镜像与所选执行闭包
  -> Wasm codegen：在预链接 Rust RT 模板上纯内存追加代码和数据
  -> Wasmi Session：加载内存模块、注入已验证的数据
  -> 求值初始化根，冻结 main 区
  -> work 区：eval 调用、Test 或 Entry 调度，安全边界精确回收
```

前三个 Pass 不持有 VM 或运行时 heap，不执行 Telora 代码。Query/LSP 可以读取未成功
seal 的 MIR；执行入口必须通过 seal。后续阶段直接使用静态结果，不重新 resolve 名称、
推断类型或通过求值补全类型骨架。

主要源码入口如下，core 路径相对于 `crates/telora-core/src/`：

| 层次 | 当前实现 |
| --- | --- |
| grammar、CST、parser、借用语法视图 | `syntax/telora/` |
| 共享数据 parser、source 与 document | `crates/telora-data/src/` |
| session 图与 HIR lowering | `mir.rs`、`hir_lower/`、`module_resolve.rs` |
| 符号、类型求解 | `symbol_resolve.rs`、`type_resolve.rs` 及其子目录 |
| 封闭与只读查询 | `mir/seal.rs`、`mir_query.rs` |
| 静态执行闭包与类型镜像 | `mir/executable.rs`、`type_image.rs` |
| Wasm 生成与内存组装 | `crates/telora-wasm/src/{codegen,compose,template}.rs` |
| 初始化、需求求值 | `crates/telora-wasm/src/{entry,properties,session}.rs` |
| Rust RT、值与复制回收 | `crates/telora-wasm/rt/`、`rt/collect.rs`、`rt/collect_trace.rs` |
| RT 共享 ABI、JSON 文本原语 | `crates/telora-wasm-shared/src/` |
| package 与 Host 契约 | `package.rs`、`runtime_host.rs` |
| CLI 静态输入与编辑器快照 | `crates/telora/src/static_input.rs`、`crates/telora/src/mir_workspace.rs` |
| CLI 命令消费者 | `crates/telora/src/{static_cli,eval_cli,test_cli,main}.rs` |

codegen 消费 SealedExecutable，生成 Wasm 指令和类型确定的胶水。Rust RT 在 Cargo
构建期间预编译、预链接并嵌入 Telora；用户执行期间无需外部 linker，不写临时代码文件。
旧 bytecode/LIR/VM 与直接 Cranelift 后端已删除，无后端选择开关或产物 CLI。

## 2. Frontend 与静态诊断

`.telora` 使用 Tree-sitter 单一路径。语法位于 `tree-sitter-telora/grammar.js`，
生成的 C parser 与手写 external scanner 一同编译；修改语法后在子模块运行
`tree-sitter generate` 并提交生成结果，不手工修改生成代码。
core 的 `syntax/telora/tree_sitter/` 负责分块输入、token 分类与验证；
`tree_sitter.rs` 迭代投影到独立的 `cst.rs` 平坦语义 CST。节点分类使用数值 ID 映射，
遍历携带父上下文，避免反复从根查找父节点。补全直接消费已保存的 CST token。

JSON 使用 `telora-data/src/json/` 中的 Logos 局部词法识别与显式状态栈，分为
parse-0 / parse-1：前者建立只有原文范围的扁平结构并检查资源配额，后者验证数字、
解码转义并按实际文本排序、检查重复 key。两个阶段复用等宽节点数组，不构造 CST
或递归 Owned AST。普通文本仍指向原文，所有需要解码的文本共用一个缓冲区；
节点以 Source / Decoded Span 区分两者，不持有 String 或 Arc。
YAML 使用 `telora-data/src/yaml/` 中的 Logos 局部词法识别、行索引、block 任务栈与 flow
容器栈；同样分为 parse-0 / parse-1，不构造 CST，不支持 anchor、alias 和 merge。
parse-0 只保留源码 span、扁平节点及块字符串的折叠/chomping 片段描述，并检查最终载荷配额；
不生成解码字符串或 Bytes。parse-1 转换数字、解码文本/base64，并按实际 key 内容排序、
累积重复 key 和数值错误；两个阶段复用等宽节点数组。普通文本及无转义的引号字符串
直接引用源码，转换文本共用一个 String，二进制共用一个 Vec<u8>，节点只保存范围。
行索引识别 LF/CRLF/CR；mapping、注释、flow 边界与字符串扫描共用外置引号状态。
TOML 使用 `telora-data/src/toml/` 中的 Logos 局部词法识别与显式任务栈，也采用两阶段。
parse-0 保留 `Table { header, items }` 语法段：表头是名称 span 列表，数组表有独立标记，
顶层赋值属于隐式段；dotted key 保留路径，不在此阶段合并表或判定重定义。四种字符串
模式外置，字符串、数字与日期时间仅保存原文范围，计量载荷但不分配解码文本。
parse-1 将转换文本写入同一缓冲区，冻结文本后以借用的键建立身份并组装数据树；
处理 dotted key、隐式/显式表、inline table 封闭与数组表，累积重复定义和标量诊断。
源码大小、语法嵌套、原始值数量和字面量长度在 parse-0 限制；最终节点数、深度、
容器大小及 key/value 累计载荷在 parse-1 构建时检查，任一资源超限立即停止。
成功的数据树以迭代方式计算后序编号并原地置换、重连边，不复制文本或递归遍历。
数据解析不再依赖 Lelwel 或 parser 生成器。
生成的状态机不受手写源文件大小限制，手写逻辑和测试使用正常子模块划分。

源码 parser 保留 lossless CST、恢复后的语法和诊断。module Pass 将可达源码挂入 MIR，
记录源码有效性，并分配扁平 HIR 节点及相应 resolve/type 槽。源码不完整也能产生可查询图，
但不能因此获得执行资格。

解析支持合作式取消：源码分块读取、Tree-sitter 进度回调、token/CST 遍历和结构诊断
均检查取消。取消后不发布部分 CST/MIR，也不回退旧解析器。编辑器的 QueryContext
将取消及版本过期传入模块构图；单次节点操作和后续静态 Pass 仍有各自的检查粒度，
这一机制不等同于资源总量配额或抢占式调度。

CST 是语法数据的唯一所有者；`syntax/telora/ast.rs` 提供借用视图，不构造完整 Owned AST。
`hir_lower` 用显式任务栈读取这些视图，直接向 HIR arena 写入节点和 Id 边。
源码节点记录 `HirOrigin::Source`，脱糖节点记录 `HirOrigin::Desugared`，两者都引用所属模块
CST 中的节点；这不是一对一映射，同一处语法可以产生多个语义节点。
字符串等字面量在 lowering 时解码，后续阶段消费语义载荷，不重新解析源码。

语法恢复由 parser 决定，CST 保存恢复结果，借用视图允许必要子节点缺失。
HIR 保留仍有意义的操作和绑定，以 `Missing` 表示没有语法证据的必要位置；
缺失子节点不会把父节点变成错误节点。lowering 不重新扫描错误子树，也不按诊断文本
拼接或替换 parser 的结论。后续静态 Pass 可以继续求解已有信息，但含 `Missing` 的图不能 seal。

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

### 静态 mini pass 调度

MIR 承载待填槽位和查询结论，任务只指定目标 Id 与局部操作，不递归调度子任务。
符号索引使用 `(HirId, ScopeId, IndexPass)` 显式工作表，保持源码顺序和稳定的符号分配。
符号、引用、命名空间、构造器分类使用 `ResolveTask`；查询方法只读取 MIR 的结果，
缺少输入时返回依赖任务，由外层循环登记等待并调度生产者。结果发布后只唤醒其消费者，
入队去重，不以反复扫描全部符号推进引用链。

`resolution_facts` 保存命名空间、构造器分类和等待边。尚未查询/仍在等待，与明确的
否定结论分开。队列耗尽时才分析无法推进的依赖环，发布无依据的引用或分类结论，
其余消费者继续正常求解。符号诊断在进入类型阶段前按来源排序；类型阶段消费最终
绑定、未解析或冲突结果，不自行重做名字查找。

类型阶段保留既有约束工作表及 revision 固定点轮次。结构相等与字面量兼容检查
共用一个局部约束队列，兼容处理不重新调用相等求解。匹配、参数检查和模式 occurs
使用显式工作表，访问去重保留 binder 上下文。已解析类型的替换把参数映射、子结果槽位
和结果放在 MIR 的 `type_substitution` 中，通过 Visit/Finish 工作项完成；同一上下文的
共享子类型只求解一次。结果进入规范类型表后，这组工作槽位可供下一次替换复用。

MIR dump 包含上述查询结论、等待原因和当前替换槽位，读取不触发求解。
module 图遍历、引用闭合、类型辅助遍历及 seal 的图检查不依赖输入深度递归调度。
诊断类型文本的渲染仍有显式上限（深度 32、节点预算 128），不随输入无限增长。
这不对生成 parser 或运行时的栈行为作出承诺。

源码位置从 CST/HIR 保留到 Wasm debug origins 和运行时值。逻辑模块名用于诊断，物理路径由 Host
单独保存。CLI JSONL 使用 1-based line、0-based UTF-8 byte column；LSP 根据客户端协商
编码转换位置。

## 3. 模块图、骨架和静态身份

let 的普通绑定、解构绑定和 let-else 共用语法前缀。解析完初始化表达式后，才根据
紧接的 `else` 或 `;` 确定绑定形式；插值和 if/else 的嵌套由表达式语法消费，不用
额外的括号计数前瞻扫描。CST 仍保留三种绑定节点，供 lowering 和诊断使用。

CLI 的 `package_host` 先准备 `ResolvedWorkspace`：发现 workspace、校验 lock 与 crate
manifest，并完成需要的 package 安装。解析器和 VM 不执行 package acquisition，也不
隐式重写 lock。package preparation 与业务服务初始化分离。

`static_input::Inventory` 从所有可用模块名称建立清单。module Pass 先排序清单并分配
ModuleId，再从所选根逐级读取可达源码。共享依赖只读入并解析一次，未到达模块保持
Unloaded。完整 inventory 的身份分配与源码读取、初始化顺序无关。

源码访问边界以规范化 cname 为键，逻辑路径始终使用 `/`。Host 根据
`telora-config.json`、`telora-crate.json` 和 `telora-lock.json` 建立资源地图，
负责物理路径规范化、目录包含性检查和文件读取；传入 module Pass 的只有逻辑模块清单、
入口 cname 和按 cname 读取文本的回调。物理路径不进入 MIR 的模块身份。
入口直接从逻辑清单选择，不构造假文件路径或 `<pending>` 模块。内置源码、磁盘源码和
编辑器文档使用同一逻辑身份边界。旧的基于物理路径的 `ModuleResolver` 已删除。

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

Host 的数据模块导入与 Wasm RT 的 `json.parse`、`toml.parse`、`yaml.parse` 共用
`telora-data`。共享库使用 `no_std + alloc`，输出带来源位置的扁平数据图。
JSON 的词法模式、容器栈与配额计数显式保存；字符串按
QStart/QEnd/Text/EscChar/EscUtf16 消费，不接受 `\x`。parse-0 只计量解码长度，
不分配解码文本；文件大小、深度、节点数、容器宽度、解码字符串长度与累计 payload
在构建时准入，资源超限立即停止。可恢复的多余逗号、重复 key 和数字范围问题
可以产生多条带原文位置的诊断；错误输入不发布值计划。parse-1 只在需要转换时
向单个解码缓冲区追加文本。YAML 同样在 parse-0 检查这些限制，
base64 只计量 Bytes 长度，block scalar 按 folding/chomping 后的实际载荷计数；
可恢复的 flow 多余逗号、重复 key 与数值错误可一起报告，资源错误仍立即停止。
JSON/YAML 节点天然子节点优先；TOML 在 parse-1 完成后序整理。三个格式的 RT 导出均直接消费 span。
运行时产生的 Telora 值沿用输入字符串的来源位置。

代码来源保留可编辑的 Rope；数据来源直接接管读入的连续 String。Host 将原始数据字节传入 Guest，
静态数据模块、注入来源及测试 fixture 均在 Guest 完成 JSON/YAML/TOML 解析和语言值构造，
不在 Host 构造并复制数据树。内部实验数据包携带原始文本及来源元数据，加载时也走同一 Guest 解析路径。
内置 `json.parse` / `yaml.parse` / `toml.parse` 借用 VM
中的输入字符串并直接导出 Span，所有解码文本共用一份 VM 生命周期的缓冲区，
不创建临时 Rope 或逐字符串的 owned-plan。

Host 配置、产物元数据和 EES 协议的 JSON 文本也先由同一 JSON 状态机校验，再由可选的
`json_serde` 适配器转换为 Rust 结构。Serde 不参与这些入口的文本解析；JSON 输出仍可
使用 serde_json 序列化。LSP 协议保留原有 serde/serde_json 实现。
实际内容在执行准备阶段接受格式与 DataLimits 检查，全部有效后才注入 Wasm。

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

类型声明直接链接其定义表达式的类型身份，不经过普通值的适配队列。类型位置的调用
显式登记结果槽：尚未求出的类型构造结果不等于自由推导变量，使用点的 Fit 必须等待
该结果确定形状，才能检查或适配字面量。这样跨模块的多个使用点不会抢先决定共享声明
的类型；无法求出的结果保留 Unknown/Conflicted，不在求解停机时放行适配约束。

`TypeTerm` 的参数仍可指向未知或代理槽；最终 `ResolvedType` 的参数都是 TypeId。
名义实例保留声明身份及类型实参。递归、泛型、部分应用和隐式类型实参的证据也进入图，
不能把尚未闭合的泛型调用留给 codegen 猜测。

泛型实例以声明 SymbolId 和按参数 SymbolId 排序的归一化类型绑定为键，先登记 ID
再展开，递归引用复用已登记的节点。实例图没有固定总数上限，也没有对应的配额参数。
源码义务和具体实例新产生的义务共用 MIR 的 evidence 图，以 (subject TypeId, bound
TypeId) 去重，先登记 ID，再展开候选实现及其依赖。增量游标只处理新节点；依赖展开
结束后，用反向依赖与待满足计数驱动证明队列。已有节点的依赖不再变化，后续批次直接
消费其结果。没有外部证明的循环不能自证，闭合后保留 Rejected；不使用失败回滚。

实例代入时，所有绑定到引用节点的泛型约束都会请求具体 evidence，不仅处理直接的
trait member 调用。调用所选实现继续产生具体 impl 实例，代码生成消费实例中的
implementation ID，不重新选择 trait。重复调用共享 evidence 和实现实例身份。

旧的保守参数流增长拒绝及其符号匹配近似已经移除；不再仅因“无法证明有限展开”而
拒绝实例。nominal 成员原有的确定性增长检测仍提供类型布局诊断。一般实例增长由
`expansion_limits` 兜底：初始限制为归一化类型结构深度 256（叶节点为 1）、
Tuple 单元数 1024、其他类型节点参数数 4096。这些是编译期结构限制，不是实例
总量限制，也不使用 Wasm fuel。名义类型的成员布局不作为该名义类型的子参数递归
计算；布局自身的结构节点仍受检查。每个已解析类型在子类型之后进入数组，深度按
TypeId 增量记录，每个节点只计算一次。

这三个默认值通过 workspace 配置的 `compiler.maxTypeDepth`、`compiler.maxTupleItems`
和 `compiler.maxTypeArguments` 覆盖，没有对应 CLI 参数。Inventory 将配置传入核心
类型求解器，CLI 与 LSP 共用入口；配置不参与 package lock，依赖不能覆盖 session 设置。

检查覆盖首次类型归一化之后，以及 evidence、名义布局和函数实例队列继续展开之前。
一次对有限模板的替换仍可能先生成有限的一批新节点，检查在下一次展开前拦截，不是
逐次堆分配的硬限额。触限报告资源诊断，保留已有类型求解结果并停止后续展开，禁止
seal；它不证明程序无限展开，也不承诺严格的编译时间或内存上界。此阶段停止整个
后续展开，而不是按失败依赖分量继续处理的 best-effort 调度。evidence 因触限而未完成
时保留 Unresolved，不派生虚假的“缺少实现”诊断。

这个实现仍不声称已经完成所有静态分析路径的停机性证明；保护覆盖已解析类型的展开，
不能替代对前序约束生成、归一化和调度自身的审计。

`Mir::seal` 检查必需类型槽、泛型实例、成员选择、类型布局、构造检查、property 与 bound
证据是否完整。成功返回只读借用 `SealedMir` 和独立的 TypeImage；seal 不重新编号。
失败保留原 MIR 和诊断，供 query/LSP 使用。

返回 `Never` 的函数值可用于参数类型完全相同、返回类型不同的函数签名位置。
类型求解在使用节点记录目标签名和 `value_adjustments`，保留原函数的声明类型；
泛型实例物化时同步替换适配中的类型参数。seal 检查所需适配证据，Wasm 仅消费
已确定的签名来调整函数值的类型标记，无需返回值转换或后端重新推断。

入口执行另有 `Mir::seal_export` 发布的 `SealedExecutable`：它保留 TypeImage，
并封闭所选导出的值依赖、具体实例及元数据初始化集合。未实例化模板仅属于静态
声明图；进入执行集合的类型必须具体化。Wasm 入口和模块检查都消费这一发布
能力，分别由 `seal_export` 和 `seal_modules` 确定执行范围，不在 codegen 中重建
依赖闭包。

相同完整输入应产生确定的 MIR ID、TypeImage 和执行闭包，不受 inventory
枚举顺序影响。这是完整构建的确定性，不是跨版本或增量编辑的永久 ID 保证。

TypeImage 是扁平只读数组，保存类型、已应用布局和类型定义。其索引对应静态 Pass
分配的 TypeId。VM 安装该图后，TypeDesc、codec、Dyn 与类型元数据操作查询已有类型
和布局；它们可以物化元数据值，但不能分配新的推导槽或求值生成类型骨架。

`T` 是静态类型，`T.type` 生成带精确见证的元数据值。普通函数返回的元数据不能反向
成为静态类型声明。Fn 和 tuple 的语法 lowering 与普通内置名称绑定保持区分；
`()` 复用空 Tuple，Unit 是其别名。

## 5. codegen、构造校验与运行时表示

codegen 的公开编译入口接受 SealedExecutable。表达式类型、泛型实例、模式构造器和成员选择
来自已完成的证据表；生成闭包、调用和构造代码不再启动类型推导。

`@check` 的静态阶段确定校验器签名与构造目标；校验函数在 VM 中执行。具名字段 struct
接收 Unchecked(T)，newtype/带载荷 enum variant 接收载荷，返回 Result((), BlameError)。
普通构造拒绝产生运行时失败，codec 解码拒绝返回 Err。读取或复制已完成的值不会重新
执行构造校验；新构造与 `<~` 更新会检查其结果。

运行时值头包含 12 字节 Loc（src/start/end 三个 u32）和 4 字节 TypeId（共 16 字节），
后接由静态布局决定的 payload。标量值为 24 字节；函数保存函数表索引与闭包环境。
String、Array、Record 等对象位于各自 typed table；Tuple/Record 共用 Record table。
Dict 使用有序 keys/values，字段操作与构造胶水消费已闭合的布局证据。
具体尺寸与表示以 `telora-wasm-shared/src/abi.rs` 和生成器为准，不构成发布 ABI。

MIR 的 value_materializations 按表达式记录 enum/Bool 与 newtype 构造事实，类型
来自该节点或封闭泛型实例，Loc 来自该节点。seal 检查身份别名边界、variant 与签名；
codegen 不沿 def/let initializer 追溯构造器。类型域别名不作为初始化 demand。
pattern 使用单独的 member selection 事实，不执行值物化。

enum 构造器代码按封闭签名和 variant 复用。其函数值的 environment 为 0，invoke
将函数值地址作为第一个参数交给构造器胶水，用于复制 12 字节 Loc；payload
仍按原布局搬运，不重写其来源。普通闭包使用非零环境句柄，调用约定不变。
内部 ABI 版本为 19；旧 Wasm 制品需重新生成，不能混用旧值布局。

全零 Loc 表示无来源；其余位置直接保存实际输入中的 UTF-8 字节范围 `[start,end)`。
不维护逐位置登记表。静态源码的来源名称和 BOLs 随 Wasm 生成；data-source 在 Guest
解析时建立 BOLs，随来源记录保存和回收。CRLF、LF、CR 都计作一次换行。
`with_diagnostics` 在 Guest 内查询来源行索引，生成 `SourcePoint {line, offset}`。
普通字符串解析继承输入 Loc；临时解析错误只附加输入内的 start/end 字节范围，
不建立 BOLs、不计算临时行号。LF/CRLF 制品字节一致性留待未来发布机制处理。
原始文本片段、UTF-16 列和终端宽度的转换由 Host 负责。
详细布局见 [RFC 0300](../../rfc/0300-host-guest-abi-and-location-ids.md)。

类型元数据复用静态 TypeId，不递归重建类型描述符。语言值和闭包留在 Wasm 内存，
Host 通过带类型的 session 句柄传递根；仅输入、输出、资源和诊断跨 Host 边界。

typed equality 使用类型身份及对应值表示，来源位置不参与相等；Dyn 的投影与 codec
通过已确定的见证检查契约。动态值检查属于运行时行为，不是重新推断表达式类型。

## 6. Property、MainWorld 与 WorkWorld

静态 property 记录说明某个 owner/member 是否具有特定 carrier，以及 provider 的
签名和来源。判断 HasProperty 不需要执行 provider。property 内容则是运行时值，
不属于类型骨架。

SealedExecutable 确定全局值、具体函数实例与 property 初始化集合。property 使用
TypeId、carrier TypeId 和 member/site 身份建立键。同一键的 provider 按既定顺序
fold，最终只有一个有效结果；不同成员仍是不同键。

生成代码访问 Wasm 内的需求状态表：Pending、Running、Ready、Failed。读取已完成值
直接复用结果；再次请求 Running 节点报告依赖环，Failed 传播已有失败。
codegen 不与运行时共用可变推导状态。

数据注入后主动完成初始化根，顶层值与 property 的相互依赖由需求求值处理。
初始化不调用普通函数体，除非某个初始化计算实际调用它。成功后冻结线性内存的 main
边界，后续分配属于 work；执行阶段不重新启动初始化。

服务事件及测试边界进行精确 work copy-collect。根包括跨事件状态、闭包、待执行
测试描述与必要缓存；遍历依据闭合类型布局，更新所有移动句柄，保留共享与环。
main 引用保持稳定。线性内存允许保留高水位，但固定存活状态应复用 work 空间，
不能以重建 session 或丢弃状态实现回收。

运行期数据的来源记录也参与回收：值、内联 enum payload、闭包及 Blame 的来源
标记决定哪些记录仍存活。静态/初始化来源固定保留；动态来源从 RT 和 Host manifest
同步移除后，Host 才能清空并复用其专属 SourceDatabase 槽位。MIR 的源码槽不复用。
来源注册只读取数据计划实际引用的文件，不能重新注册已释放的历史输入。

## 7. 诊断、失败与发布

静态诊断由三个 Pass 和 seal 产生。无静态执行，所以 Unknown/Conflicted 不等于一次
运行失败，也无需通过重建 VM 或重跑旧求解器恢复。存在静态错误时不进入初始化。
类型统一冲突使用中立表述 `type mismatch between A and B`；两侧顺序不表示
actual/expected，也不为统一诊断文案而调整求解器的合并方向。

运行时失败保留规则位置和数据来源；`raise!` 产生 Never，`warn!` 产生值为 None 的
Option(T)。`blame!` 构造错误数据，规则归因由调用/构造边界与 VM 诊断逻辑共同保留。
`@check` 的 Err 路径不会把失败候选发布成合法 T。

check 的初始化代码继续独立 demand，失败依赖读取缓存中的原错误，不重复诊断；
所有 demand 完成后若 session 失败则不 freeze、不发布。运行命令的初始化代码在
首个失败 demand 返回，函数内部始终在失败结果处立即返回。资源、取消或一致性等
终止错误仍中止 session。没有任何 error 才能把 session 当作成功对外输出。这个保证
针对 session 结果，不是外部 Host 已执行 effect 的事务回滚。

源码名与字节范围保留在 VM debug origins 和值来源中。fixture、eval/run 输入等来源
由 Host 注册，物理文件定位不授予语言额外文件访问权限。

## 8. 资源与 Host 边界

Wasmi 提供 fuel、内存增长与调用栈限制，Session 管理诊断和终止状态。静态类型求解不消耗
执行 fuel；这不表示解析、求解或编辑器请求没有资源与取消约束。引擎 trap 和 fuel
耗尽终止会话，不能伪装为可恢复语言失败，也不重置 fuel 后继续测试。

目标是执行有边界、失控时能停机，不是精确计费或限制进程的实际资源占用。
直接使用引擎 fuel；CLI 线性内存增长默认上限为 1024 MiB（1 GiB），函数表上限为 100 万项，
调用栈沿用引擎限制。增长超限直接 trap，不模拟逻辑分配量，也不核算每次复制。
这些是私有实现阈值；Wasm 内存边界不是进程 RSS 上限。

workspace 配置的 `runtime.fuel` 和 `runtime.memoryLimit` 提供会话默认值，分别为
100 和 1024，单位为 1,000,000 fuel 和 MiB（`1 << 20` 字节，16 个 Wasm 页）。CLI 正式参数
`--with-fuel N`、`--with-memory-limit N` 仅在显式传入时逐项覆盖对应配置。
N 必须为正整数，超出可表示范围的输入在配置或参数解析时拒绝。
参数适用于所有执行命令；批量 roots、初始化与后续执行共享预算，不按模块数量放大，
也不在用例或请求之间重置。`check --only-types` 不创建执行会话，因此没有执行用量报告。
`--report-usage` 在执行会话结束时向 stderr 输出合法的 info 级 JSON 诊断
（`schema: telora.execution/v1`、`record: diagnostic`、`code: execution-usage`）。
`usage.fuel` 包含原始单位的 `limit`、`consumed`、`remaining`；`usage.linear_memory`
包含字节单位的 `bytes`、`limit_bytes`。内存占用是引擎已分配的线性内存大小，
按 Wasm 页增长，不是存活对象大小或进程 RSS。已创建的会话即使执行失败也报告用量；
实例创建前的失败没有可报告的会话。这不是精确计费信息，也不进入命令的结果流。

验收用持续尾递归与持续扩大数组的语言用例，验证分别因 fuel 和内存边界终止。
测试使用较小内存边界，避免真的分配 1 GiB；不断言精确扣费次数。

数据入口单独使用 DataLimits 检查文件大小、节点数、深度、单容器成员数和 payload
大小。通过 admission 后才物化 Value。Wasm 内的 codec/parse 同受引擎终止边界约束。
fixture 仅累计已接受的源文本字节作为粗略输入边界，保留展开次数和深度限制；
不再按“节点数 × 固定字节数”估算并重复扣除 guest 堆用量。
错误消息和来源应保留，但配额的具体数值是 Host 配置，不是语言语法。

包管理继续使用私有 IMOS Host。应用不再创建 EES service。

内置 std/_entry/transform 与应用在同一 MIR 中求解，MainService 的 init/transform
实例由静态 trait 证据选择。Plan 是内部 (sources, initializer)；initializer 返回捕获 Self
的已类型化 handler，with_diagnostics 包装每次调用。Host 不解码 Self。

当前 reset 复用 wasmi Module，创建新 store/instance，再恢复初始化后的线性内存及
全部 mutable globals（包括 Rust stack pointer）。函数表由静态链接确定。
不重复 codegen、数据加载或 init。请求临时值、trap 状态和来源登记随 reset 丢弃。
该基线复制是首版实现，不是语言规定；后续可优化 reset 成本。

## 9. CLI## 9. CLI 与 LSP 的阶段边界

| 命令 | 消费边界 |
| --- | --- |
| query modules | inventory 清单 |
| query exports / at、LSP 语义查询 | 三个静态 Pass 后的 MIR，可保留错误和未知事实 |
| check --only-types | 三个 Pass 与 seal，不读取数据内容或执行 Telora 代码 |
| check | seal、codegen、链接、数据注入及整图初始化 |
| eval | 初始化后取得选中 Value 导出 |
| test NAME | 初始化后执行该测试模块直接导出的 Test |
| run / serve | 初始化 MainService，按请求调用 transform，间隙 reset |

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

验证入口包括三个 resolve 模块的单元测试、`crates/telora-wasm/src/tests/` 的
执行与回收测试，以及 `crates/telora/tests/cli.rs`、`tests/runtime/` 和
`tests/language/`。语言规则与诊断回归优先使用 Telora 用例，检查成功、拒绝、来源、
泛型实例以及构造/解码/更新边界。

完整构建的确定性覆盖静态身份与所选执行闭包；Wasm 测试覆盖生成代码、初始化、
数据来源和长期服务根。发布缓存、snapshot、引擎替换与进一步减少复制不属于当前路径。
