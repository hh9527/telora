
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
  -> 求值初始化根，注册长期保活根
  -> eval 调用、Test 或 Entry 调度，安全边界准确回收
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
| 初始化、需求求值 | `crates/telora-wasm/src/{transform_service,properties,session}.rs` |
| Rust RT、值与复制回收 | `crates/telora-wasm/rt/`、`rt/collect.rs`、`rt/collect_trace.rs` |
| RT 共享 ABI、JSON 文本原语 | `crates/telora-wasm-shared/src/` |
| package 与 Host 契约 | `package.rs`、`runtime_host.rs` |
| CLI 静态输入与编辑器快照 | `crates/telora/src/static_input.rs`、`crates/telora/src/mir_workspace.rs` |
| CLI 命令消费者 | `crates/telora/src/{static_cli,eval_cli,test_cli,main}.rs` |

codegen 消费 SealedExecutable，生成 Wasm 指令和类型确定的胶水。Rust RT 在 Cargo
构建期间预编译、预链接并嵌入 Telora；用户执行期间无需外部 linker，不写临时代码文件。
链接阶段用固定宽度的 LEB 字段填入重定位结果，完成全部编号和地址分配后，将最终代码段
重新编码为最短 LEB 形式。压紧保持函数索引、指令语义和其他段的内容，适用于所有编译入口。
执行统一为 Wasm/wasmi；`telora build` 可保存普通 Wasm，也可嵌入初始化 snapshot，
`telora-run` 独立执行制品。没有运行后端选择开关。

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
`Bound`、`Unresolved` 和 `Conflicted`；冲突区分重复定义、多个 `use` 候选等。
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

`static_input::Inventory` 保存 package root 和可按 cname 访问的来源。module Pass 从
固定根开始，解析 CST 后沿 `mod` / `data` 边发现并分配 ModuleId。共享依赖只读入并
解析一次；未挂载文件不进入 MIR。发现顺序确定，且与源码读取和初始化顺序无关。

源码访问边界以规范化 cname 为键，逻辑路径始终使用 `/`。Host 根据
`telora-config.json`、`telora-crate.json` 和 `telora-lock.json` 建立资源地图，
负责物理路径规范化、目录包含性检查和文件读取；传入 module Pass 的只有入口 cname、
发现回调和按 cname 读取文本的回调。物理路径不进入 MIR 的模块身份。
入口直接从固定根或测试访问边界选择，不构造假文件路径或 `<pending>` 模块。内置源码、磁盘源码和
编辑器文档使用同一逻辑身份边界。旧的基于物理路径的 `ModuleResolver` 已删除。

`telora-crate.json` 只声明 crate 名和直接依赖。每个 crate 固定从 `src/lib.telora`
开始，CST 中的 `mod` / `data` 声明驱动按需发现；`use` 不能装载未挂载文件。测试选择
额外递归建立当前 crate 的 `tests/` 访问边界并拒绝 symlink；测试模块可相互引用，
普通源码不能反向访问测试。

数据模块在静态阶段只有编译器生成的接口：

```telora
use std::value::{ Value };
decl data: Value;
pub use self::{ data };
```

数据内容在静态阶段不读取、不解析；因此类型检查成功不代表 JSON/YAML/TOML 内容有效。

数据模块导入与 Wasm RT 的 `json.parse`、`toml.parse`、`yaml.parse` 共用
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
不在 Host 构造并复制数据树。发布时，数据模块在 Guest 中解析、物化并回收临时对象，
以二进制语言值镜像保存到 Wasm passive data segment。启动仅装载镜像，不重复解析。
外部静态来源仅在快照构建时固化；源码及数据文件原文不进入制品。
内置 `json.parse` / `yaml.parse` / `toml.parse` 借用 VM
中的输入字符串并直接导出 Span，每次解析的转换文本共用该次解析上下文中的缓冲区，
不创建临时 Rope 或逐字符串的 owned-plan。

Host 的 workspace/package 配置经共享 JSON 解析及 `json_serde` 适配取得 Rust 结构。
编译器侧完整宿主描述属于 `telora.tooling`，使用类型固定的 serde 协议读取；发布时移除。
发布 `telora.manifest` 仅保留 ABI 版本，类型反射与回收使用 Guest 内的编译后镜像。
Guest 中的数据解析使用 telora-data。独立 runner 的制品 envelope、服务响应与 LSP
协议使用 serde_json；这些 Host 协议与 Telora 的 JSON 数据解析不是同一入口。
数据字节先传入 Guest，再接受格式与 DataLimits 检查；只有成功解析才安装为语言值。

symbol Pass 先索引模块声明、公开项和作用域，再闭合引用。`use` 的 namespace alias
和选择性绑定都建立普通静态绑定；显式绑定与遮蔽按普通名称解析规则处理。内置类型的特殊身份来自
native 声明的 NativeTypeId，不能根据 Int、Array 等拼写识别。默认的
`std::prelude` fallback 提供普通名称；`@property` 等装饰器也遵循这些绑定规则。

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

运行时值头是一个 8 字节 packed Loc（src-id:14、start/end:25），后接由静态布局
决定的 payload。普通值不逐值保存 TypeId：函数签名、字段/元素布局、demand 根和 Host
typed handle 提供封闭类型；Dyn payload 保留具体 TypeId。标量值为 16 字节；函数保存
函数表索引与闭包环境。闭包环境将捕获指针和对应封闭 TypeId 分列存储，collector 不从
被捕获值反查类型。
有限且不含普通 GC 引用或内部动态所有权的 struct/tuple/newtype 将字段内联在值中；
payload 的首个字为固定 `INLINE_FIELDS` 标记，字段区从完整值偏移 16 开始，字段
各自保留完整来源头。按值复制不依赖原值的载荷地址。递归存储及含引用聚合使用
间接载荷；Array 的动态元素区也使用带封闭布局的直接对象引用。普通对象的扫描
角色与资源表索引分开，Regex/Fmt 等资源仍通过各自的表槽访问。
String/Bytes 不足 16 字节时直接内联，最后一字节保存长度；其余值保存
`raw/start/end`，最后一字节为 16。raw 是受管堆中稳定的逻辑引用，指向
`len:u32/cap:u32/bytes:[u8;cap]`；start/end 相对字节区。小切片复制内联，
长切片保活完整 Raw。执行期间 mark-sweep 只标记底层分配，不搬移内容或修补视图；
显式边界压缩将共享 Raw 复制一次并重定位引用。没有独立内容池或 String/Bytes 表。
线性内存 48/52 保存保留零字与活 Raw 分配字节数（含头），已包含在 managed
统计中。结构化对象、Raw 内容与表缓冲区通过 Guest 默认 Rust 分配器独立零初始化
分配；准确 GC 的登记表记录 Layout、范围、扫描描述和标记，内部引用保活所属分配。
committed 包含分配的对齐存储和登记表容量，不包含底层分配器隐藏开销。
RT 受管堆默认预算为 32 MiB，来源和 Rust 资源另受 Guest 线性内存上限约束。
动态引用为稳定 Guest 地址；小于 origin 的引用指向永久 Wasm 静态区，origin 是链接后
静态镜像末端。动态分配的地址及范围必须小于 0x80000000，高位命名空间用于 scratch。
Host 交换缓冲区和来源记录使用真实线性内存地址，不能当作语言引用传入。
40/44 为保留零字与当前受管分配字节数，56 保存 origin；这不是动态存储映射表。
类型追踪与反射描述在编译时进入永久静态镜像。全部 Ready demand、service handler、
活动调用根、Host pin 以及尚待消费的诊断/debug 事件保活各自可达图。初始化专用句柄
在成功初始化后清空。显式复制回收会更新移动句柄；普通准确 GC 保持地址稳定。
Array 完整值固定 24 字节，payload 为 reference/start/end/kind。Empty 没有底层
存储；Single 引用一个独立完整元素槽，按元素布局扫描；Slice 引用 RawArray 头和
共享缓冲。Dict 使用有序 keys/values 的 RawArray 列，列自身不带 Array kind。
数组发布按长度 0/1/其余规范化，dict::keys/values 也产生合法的 Array 值。
filter 和 Seq/Sink collect 从零容量开始，第一个结果分配单槽，第二个结果才建立
增长缓冲；不对谓词做第二遍求值。GC 仅扫描有效元素，不扫描剩余容量。
内部 push 仅在可写 RawArray 的 view.end 等于 raw.len 时复用尾部；历史视图和
代码静态对象均分叉。非零 start 的视图保持自身计数与 raw end 分开计算。
字段操作与构造胶水消费已闭合的布局证据。
具体尺寸与表示以 `telora-wasm-shared/src/abi.rs` 和生成器为准，不构成发布 ABI。

MIR 的 value_materializations 按表达式记录 enum/Bool 与 newtype 构造事实，类型
来自该节点或封闭泛型实例，Loc 来自该节点。seal 检查身份别名边界、variant 与签名；
codegen 不沿 def/let initializer 追溯构造器。类型域别名不作为初始化 demand。
pattern 使用单独的 member selection 事实，不执行值物化。

enum 构造器代码按封闭签名和 variant 复用。其函数值的 environment 为 0，invoke
将函数值地址作为第一个参数交给构造器胶水，用于复制 8 字节 Loc；payload
仍按原布局搬运，不重写其来源。普通闭包使用非零环境句柄。
生成函数使用 `(environment, arguments, output) -> i32`；成功时将完整值复制到
非零 output 并返回该地址，语言失败返回 0。尾调用传递同一个 output。
Equal/Json 内部 helper 返回原始状态，不把其结果解释为值。Host、高阶回调和
service 调用提供结果区；初始化入口使用 0，由 demand 保留持久结果。Ready
demand 的存储先于调用方结果复制发布，不保存调用方 output 地址。对非纯值的
Ready demand 引用使用 0 借用其持久值头，避免读取全局函数也分配新的值头。

生成的 callable 帧使用独立的可复用 scratch 区。纯标量、有限纯聚合的普通构造、
纯值调用结果和临时参数包按编译期分配点占槽，循环复用；来源仍存放在完整值中。
逻辑 scratch 引用从 0x80000000 开始，与动态对象分离；普通返回先完成结果
复制，再释放帧。语言失败走同一个退出边界；引擎 trap 后由会话终止或 reset
清理临时区。闭包捕获和延迟 debug 事件复制纯值，保留自己的完整值副本。

尾调用先把参数包及纯值参数复制到受保护 transfer 区，再释放旧帧。callee 进入
时采用完整包并修正包内引用，之后可正常嵌套调用；无环境构造器继续接收函数值
本身以保留来源。临时区容量按最大活动帧需求复用，不随尾调用次数增长。调用结果
槽在调用前保留旧内容，使 fold 的上轮结果可同时作为本轮输入。
初始化 demand、二参数 materializer 及含动态所有权的值仍使用独立受管分配。
生成函数发布独立 shadow frame：参数、返回值和表达式结果使用封闭类型根，原始
分配仅保留 storage，数组构造和回调输出发布已初始化前缀，local 复用时清空根槽。
普通退出和可恢复失败退出释放根帧，尾调用在受保护参数交接后释放旧根帧。
运行时标记审计默认关闭，可在 callable 入口安全点遍历活动帧、Host pin、Ready
demand、service 初始化输入以及待消费的诊断/debug 事件。审计不执行 sweep；
显式 `telora_gc_collect` 则标记并回收动态对象、内容、未引用资源槽及动态来源，
释放失活 Regex 的 Rust 资源。资源空槽可复用。执行中收集在 callable 入口及 parser/codec 数组循环安全点
触发。默认分配间隔下限为 64 KiB；每次收集后，间隔取配置下限与当前 managed
占用一半的较大值，并以受管预算剩余余量的四分之一封顶；压力下间隔至少为配置下限与预算十六分之一的较小值（最少 1 字节），
使大存活图的扫描成本得到摊销。`telora_gc_interval` 配置间隔下限，0 关闭
自动触发，1 在每个安全点强制收集。`telora_gc_heap_limit` 可设受管
预算，预算不足时停止执行。parser/codec 上下文、Regex 捕获缓冲区和 postorder
parser plan 使用独立根角色，按实际字段/已初始化前缀扫描。物化会消费 plan：
packet 只保留未处理的 rows 后缀，子节点使用计数使临时值头在最后一个父节点
复制后退出根集合；共享语言值仍由准确 mark-sweep 追踪。
Host 使用 `Session::pin_value` 取得 Session 所有的 generation 句柄；解析 pin
取得当前借用值，边界复制回收会更新 pin，unpin 后的句柄副本及跨 Session 使用会
被拒绝。reset 使活动 pin 失效。延迟格式节点持有纯值参数副本，诊断事件持有
来源范围副本，均独立于生产它们的 scratch 帧。制品静态来源索引由代码镜像
持有，生命周期与制品一致；动态来源仍由值/来源根保活。service initializer、
handler、已注入值和所有声明的 source 身份由服务阶段拥有。
`Session::configure_gc` 配置间隔和受管预算，服务 snapshot 重建保留配置。
`gc_stats` 分别报告累计分配、当前 managed 占用、对齐存储与登记容量、收集次数和
线性内存高水位；managed 占用包含尚未清扫的临时分配，不能当作准确存活量。
`collect_garbage` 在 Host 边界执行非移动收集；测试队列持有 pin，使用此接口
回收已完成任务。`release_feedback` 在 Host 复制反馈后释放 Guest 历史，保留当前活动失败事件；
Failed demand 独立拥有其错误包，即使历史已释放也能再次报告。只读反馈接口
保留历史直到显式释放。活动失败期间仅允许非移动收集，`collect_work` 拒绝
压缩，避免移动诊断协议持有的原始错误地址。
Guest trap 使 Session 终止，拒绝继续执行、分配或改变根；语言失败仍按状态码
返回。服务 reset 以初始化 snapshot 替换陷阱实例，普通 Session 通过重新 load 恢复。
快照和数据镜像从发布根复制可达图至连续空间，修补引用并保留共享与环。导出期间
源图保持原址和所有权；导入将镜像重定位为普通独立分配，并通过长期根保活。
恢复后的对象参与准确 GC。服务 reset 清理调用状态、scratch、pin 和已消费反馈，
对象回收由 GC 决定。内部 ABI 版本为 38；旧 Wasm 制品需重新生成。
`interpreter!` 在构造时捕获输入函数，工厂和适配器使用普通值引用环境；
没有适配器 memo 槽或 raw-parent 环境，回收按实际闭包环境布局追踪。

Loc 标记值的产生处：字面量、类型域物化、计算和构造写入对应表达式的位置；
绑定、字段/元素读取及返回已有值转发原来源。native 计算的新值使用调用处，
不使用库中 native 声明处。内容不变的计算仍产生新来源，可共享底层 payload，
不能通过复用输入值头覆盖原值的位置。解析和 codec 的输入来源传播契约保持不变。

普通函数返回时不重写来源，也不分配独立 Loc。可能调用 native 的封闭签名，
在参数指针数组末尾附加一个来源指针；它指向 Wasm 静态常量，无运行时位置登记。
native 内部调用 native 回调时传递自身的计算来源，普通回调则按其函数体产生或转发值。
不可能指向 native 的普通调用省去这个隐藏参数；不存在全局“最近调用位置”。
Test 构造器使用同一新值来源规则，不再对所有调用维护测试专用状态。
Host 直接调用时没有 Telora 调用表达式，隐藏来源为 0，不伪造 native 声明处的来源。

全零 Loc 表示无来源；其余位置直接保存实际输入中的 UTF-8 字节范围 `[start,end)`。
不维护逐位置登记表。静态源码的来源名称和 BOLs 随 Wasm 生成；data-source 在 Guest
解析时建立 BOLs，随来源记录保存和回收。CRLF、LF、CR 都计作一次换行。
`with_diagnostics` 在 Guest 内查询普通来源行索引，生成从 1 开始的行号。
动态解析使用来源 ID 0 及输入内字节跨度；诊断以 `<input>` 展示，展开为 `line: 0` 和整段输入内的
`offset`，不建立 BOLs。普通字符串解析继承输入 Loc；其局部解析错误仍附带输入内
的 start/end 字节范围。`telora build` 在输入端归一化 EOL，使相同文本的 LF/CRLF/CR 构建得到相同制品；
普通源码执行仍用实际输入的字节位置，不改变原文件。
原始文本片段、UTF-16 列和终端宽度的转换由 Host 负责。
详细布局见 [RFC 0300](../../rfc/0300-host-guest-abi-and-location-ids.md)。

类型元数据复用静态 TypeId，不递归重建类型描述符。语言值和闭包留在 Wasm 内存，
Host 通过带类型的 session 句柄传递根；仅输入、输出、资源和诊断跨 Host 边界。

typed equality 使用类型身份及对应值表示，来源位置不参与相等；Dyn 的投影与 codec
通过已确定的见证检查契约。动态值检查属于运行时行为，不是重新推断表达式类型。

### 不可变流水线 lowering

std/seq（准入模块 36）和 std/sink（37）以普通泛型 struct 保存处理图。
compute 是唯一消费 native，构造和便捷入口是普通 .telora 函数。
`sequence_contract.rs` 验证 nominal 准入身份、phantom 输入/结果参数、回调及
嵌套 plan 的一致性；`sequence_plan.rs` 从封闭布局解码字段和结构。
用户同名结构不激活该 lowering，非法手工构造在发射循环前拒绝。
嵌套深度上限 128，不引入运行时类型推断或 Host 执行用户代码。

`sequence.rs` 为具体 plan 生成一个输入循环，公共前缀执行一次，分支从左
到右更新私有局部状态；完成标志沿组合树传播。不同 plan 类型生成各自代码，
同类型的条件值和函数返回值使用同一 lowering，不要求源表达式在调用点可见。
collect 的私有存储按实际输出倍增，发布已初始化前缀；标量终结状态在循环外
分配并复用。find 复制保存答案，避免引用下一轮复用的 scratch。最终才构造
组合结果。没有隐式 memoization，也不依赖 move/aliasing 系统。

源与捕获由普通参数根追踪，当前回调值、fold 状态、已停止答案和部分输出
显式发布准确根。零字节对象 payload 可位于 header 末尾之外，仅保活 header，
不将无存储的 payload 作为 managed 根。普通 plan 图支持边界压缩和快照恢复。

### 惰性 pipeline 与内部 builder

std/pipeline 是准入模块 39，公开组合函数使用普通 HIR／闭包实现。节点闭包捕获私有
类型化 memo 槽；每次 run 分配独立 token，以 token 存储身份判定命中。memo 槽为
Empty／Pending／Ready，Ready 保存 token 和不可变结果。递归 Pending 报错，语言
失败清除 Pending；trap 沿用 Session 不可继续执行的约定。显式 cache 使用独立槽及
固定 token，不受来源在其他 run 中更新槽位的影响。所有引用沿用类型化 GC 与快照。

内部 MutBuilder(Item, Result, P) 保存类型化 Array(P)，P 为普通状态 tuple 或 ArrayMut。
代码生成检查标准库名义身份与类型一致性。MapMut 用独立 keys/values 数组及排序的 u32 index，完成
时整理为 Map 的两列布局；ArrayMut 增长私有缓冲，关闭后经新不可变 header 转移
缓冲区。StringMut 内含字符串块 ArrayMut，完成时一次 join。私有入口只由标准库
实现调用，应用只持有节点与终端计划。首版每个节点完整物化，未做融合或按需前缀缓存。

## 6. Property 与初始化/请求生命周期

静态 property 记录说明某个 owner/member 是否具有特定 carrier，以及 provider 的
签名和来源。判断 HasProperty 不需要执行 provider。property 内容则是运行时值，
不属于类型骨架。

SealedExecutable 确定全局值、具体函数实例与 property 初始化集合。property 使用
TypeId、carrier TypeId 和 member/site 身份建立键。同一键的 provider 按既定顺序
fold，最终只有一个有效结果；不同成员仍是不同键。

类型阶段证明 `Property(P)` 只检查声明关系，不执行 provider。执行阶段的需求表虽然
允许依赖在首次读取时求值，但当前选入制品的具体 property 都是初始化根：生成的
`telora_initialize` 会在服务发布前主动读取这些根，并完成其依赖。因此成功发布的
service 不会在某次请求中首次计算这批 property；运行期通过 `Type` 和 member index
反射查询的也是已初始化结果。这不意味着未进入执行闭包的 property 也被计算。

生成代码访问 Wasm 内的需求状态表：Pending、Running、Ready、Failed。读取已完成值
直接复用结果；再次请求 Running 节点报告依赖环，Failed 传播已有失败。
codegen 不与运行时共用可变推导状态。

数据注入后主动完成初始化根，顶层值与 property 的相互依赖由需求求值处理。
初始化不调用普通函数体，除非某个初始化计算实际调用它。成功后，顶层 demand、
property 与 handler 作为长期根保留普通对象；执行阶段复用这些初始化结果。

Guest 使用 wasm32-unknown-unknown 标准库默认分配器，不自定义全局分配器。
mem-alloc/realloc/free 将这套分配器暴露给 Host，与 Rust Vec/String 共用。
语言对象独立分配，登记表负责准确标记和 sweep；Regex 等 Rust 资源独立持有，
死亡资源随准确追踪结果释放。free 允许分配器复用空间，不意味着 Wasm 线性内存缩页。

镜像重整保留全部 Ready demand（包括 property 结果）及 service handler，按闭合
类型布局遍历，更新所有移动句柄并保持共享与环。测试等显式边界也支持复制回收。
正常服务请求结束清理调用根与反馈；允许按阈值收集。只有 trap/poisoned 状态使用
初始化快照恢复新实例。

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
直接使用引擎 fuel；CLI 线性内存增长默认上限为 64 MiB，函数表上限为 100 万项，
调用栈沿用引擎限制。增长超限直接 trap，不模拟逻辑分配量，也不核算每次复制。
这些是私有实现阈值；Wasm 内存边界不是进程 RSS 上限。

构建期间计算独立采用 `runtime.buildtimeFuel`（默认 5000 百万 fuel）和
`runtime.buildtimeMemoryLimit`（默认 256 MiB）。`--buildtime-fuel` 与
`--buildtime-memory-limit` 覆盖这两个预算；它们用于构建期间求值，不写入运行期预算。
初始化与请求共享 `runtime.memoryLimit` 总内存边界，托管堆上限采用同一个边界，
初始化占用也计入请求可用的总量。内存 override 同时作用于初始化和请求。
`build --report-usage` 的 `telora.build.usage/v1` 记录包含准备阶段消耗的 fuel、
线性内存、托管堆及 GC 用量；此记录在运行期预算重置前采集。

workspace 配置的 `runtime.initializationFuel` 与 `runtime.requestFuel` 默认分别为
5000 和 1000（单位：百万 fuel）。`telora` CLI 的 `--initialization-fuel N` 与
`--request-fuel N` 直接覆盖相应配置值；`telora build` 将最终预算写入制品，
内存中的 Wasm 也采用同一规则。独立 `telora-run` 的同名参数覆盖制品预算。
`runtime.memoryLimit` 默认 64 MiB（每 MiB 为 `1 << 20` 字节，即 16 个 Wasm 页）。
`--with-memory-limit N` 覆盖内存边界。N 必须为正整数，超出可表示范围的
输入在配置或参数解析时拒绝。惰性字节码翻译/验证不扣 Guest fuel。
参数适用于所有执行命令。`check`/`test` 的批量 roots、初始化与用例在同一会话内共享
预算，不按模块数量放大，也不在用例之间重置；`run`（含 `--serve URI`）在每次请求前 reset，每个
请求获得独立预算，初始化不占用请求预算。`check --only-types` 不创建执行会话，因此没有执行用量报告。
`--report-usage` 在执行会话结束时向 stderr 输出合法的 info 级 JSON 诊断
（`schema: telora.execution/v1`、`record: diagnostic`、`code: execution-usage`）。
`usage.fuel` 包含原始单位的 `limit`、`consumed`、`remaining`；`usage.linear_memory`
包含字节单位的 `bytes`、`limit_bytes`。内存占用是引擎已分配的线性内存大小，
按 Wasm 页增长，不是存活对象大小或进程 RSS。已创建的会话即使执行失败也报告用量；
实例创建前的失败没有可报告的会话。这不是精确计费信息，也不进入命令的结果流。

`build --report-usage` 还报告构建阶段耗时（`schema: telora.build.timing/v1`、
`record: diagnostic`、`code: build-phase-timing`），`phase` 标识阶段，`elapsed_seconds` 使用秒。
`parent_phase` 标识父阶段：总阶段为 null，引擎子阶段为 `engine_load`，其余阶段为 `total`。
`frontend` 包含源码加载、类型求解和入口闭包检查；`codegen_link` 为代码生成与链接；
`data_bundle` 包含数据模块打包和前端资源清理；`engine_load` 为引擎加载与实例化；
`service_prepare` 求值静态 Service 定义，准备来源与初始化入口；
`service_initialize` 包含来源读取与服务初始化回调；`service_seal` 封存初始化状态；`snapshot` 提取并附加快照；
`artifact_output` 完成发布封装和原子写入；`total` 覆盖整个构建调用。
`engine_load` 的子阶段包括元数据解析 `load_metadata`、引擎创建 `load_engine`、
模块加载与验证／翻译 `load_module`、Store 创建 `load_store`、实例化与启动函数
`load_instance_start`、导出绑定 `load_exports`。
子阶段包含在 `engine_load` 内，汇总时只累加同一层级；Wasmi 公开 API 将实例化与启动合并执行。
普通构建跳过 `engine_load`、`service_prepare`、`service_initialize`、`service_seal`、`snapshot`；已进入的阶段即使失败也报告耗时，
构建是否成功仍以退出码为准。`TELORA_WASM_TIMINGS=1` 也可开启构建阶段计时。
引擎采用惰性翻译，因此初始化耗时可能包含首次执行函数的翻译成本。

验收用持续尾递归与持续扩大数组的语言用例，验证分别因 fuel 和内存边界终止。
测试使用较小内存边界，避免真实耗尽默认配额；不断言精确扣费次数。

数据入口单独使用 DataLimits 检查文件大小、节点数、深度、单容器成员数和 payload
大小。通过 admission 后才物化 Value。Wasm 内的 codec/parse 同受引擎终止边界约束。
fixture 仅累计已接受的源文本字节作为粗略输入边界，保留展开次数和深度限制；
不再按“节点数 × 固定字节数”估算并重复扣除 guest 堆用量。
错误消息和来源应保留，但配额的具体数值是 Host 配置，不是语言语法。

包管理继续使用私有 IMOS Host。应用不再创建 EES service。

内置 std/_entry/collection 与应用在同一 MIR 中求解，MainService 各字段的 init/transform
实例由静态 trait 证据选择。Plan 是内部 (sources, initializer)；initializer 构造集合并返回
按静态路由选择字段服务的 handler，with_diagnostics 包装每次调用。Host 不解码服务状态。

正常完成请求后，reset-service 清理 scratch、pin 和已消费反馈；GC 按可达性释放
动态对象与资源。Host 恢复执行 globals、debug 游标和每次请求的 fuel。
输出及 Host ABI 缓冲区必须先消费并释放。临时 parse 不登记请求来源，来源表保留
初始化时的名称/BOLs。JSON writer 在普通语言失败路径也显式释放。
只有 trap/poisoned 状态或清理本身失败时，才复用 wasmi Module 创建新 store/instance，
恢复初始化快照及全部 mutable globals（包括 Rust stack pointer）。函数表固定。
两条恢复路径都不重复 codegen、数据加载或 init；保留快照是异常恢复策略，不是旧值布局。

## 9. CLI 与 LSP 的阶段边界

| 命令 | 消费边界 |
| --- | --- |
| query modules | package root 与已发现模块 |
| query exports / at、LSP 语义查询 | 三个静态 Pass 后的 MIR，可保留错误和未知事实 |
| check --only-types | 三个 Pass 与 seal，不读取数据内容或执行 Telora 代码 |
| check | seal、codegen、链接、数据注入及整图初始化 |
| eval | 初始化后取得选中 Value 导出 |
| test NAME | 初始化后执行该测试模块直接导出的 Test |
| run / serve | 初始化 MainService 集合，按 method 或 HTTP 路由调用字段服务，间隙 reset |

`check MODULE_ID` 选择一个根。`check --lib` 从当前 crate 的 `src/lib.telora` 发现并
选择整个可达模块树，包括私有模块和数据模块；`check --tests` 递归选择当前 crate 的
tests/ 模块。两个开关可以组合，与显式 selector 互斥。依赖按静态模块边加入同一张图；
不会对每个根重启编译器。空集合成功，未挂载文件不进入图。

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
- native 特殊身份来自声明的稳定标识，普通名称受 `use`、遮蔽和作用域规则约束。
- 类型骨架不依赖 property 值，数据内容不进入静态求解。
- 初始化所选执行图的根并统一发布；移动回收保留共享、封闭类型身份和来源。
- 构造校验覆盖新的合法值边界，不能用跳过检查换取性能。
- query/LSP 可观察失败图，执行入口只能接受成功 seal 的图。

验证入口包括三个 resolve 模块的单元测试、`crates/telora-wasm/src/tests/` 的
执行与回收测试，以及 `crates/telora/tests/cli/`、`tests/runtime/` 和
`tests/language/`。语言规则与诊断回归优先使用 Telora 用例，检查成功、拒绝、来源、
泛型实例以及构造/解码/更新边界。

完整构建的确定性覆盖静态身份与所选执行闭包；Wasm 测试覆盖生成代码、初始化、
数据来源和长期服务根。Wasm 制品由 `telora build` 发布；`--snapshot` 可在 custom
section 中携带 ready service 状态，移除被其覆盖的数据模块镜像。`telora-run`
使用 wasmi 独立加载；普通制品接收外部来源并初始化，快照制品恢复固化状态，拒绝来源覆盖。
两个 CLI 共用 `telora-run::transport` 的 JSONL/TCP HTTP/Unix socket HTTP 传输层，
执行回调保持串行，各自执行入口负责 reset 和资源配额。HTTP 分帧由 Hyper 处理。
引擎替换与进一步减少复制不属于当前路径。


### 构建期双路产出

`std::build::stage(prepare, program)` 独立于 Service。`prepare: Fn() -> Output(P1)`
返回 `{runtime: P1, information: Dict(Value)}`；`program: Fn(P1) -> B` 在同一个受限
构建作用域装配运行程序 B。P1 可含类型与闭包，保持同一次编译的 TypeId 和函数表坐标，
不通过 JSON 重导入。普通运行环境中的 `stage` 失败；作用域只能由显式构建入口打开。

`telora-wasm::staged_build::StagedBuild` 支持普通函数导出。通用带类型程序根参与封存、
运行期裁剪及两种 GC，加载 API 从同一制品恢复状态，直接读取运行根而不重跑模块初始化。
首版普通函数 API 保留完整 tooling manifest，须使用 `staged_build::load` 加载；
它不是 `telora-run` 的 Service 文件格式；首版普通函数 API 暂不支持数据模块，
也尚未提供普通函数的 CLI 发布入口。

Service 使用 `build --staged` 适配，默认裁剪运行代码；`--staged=full` 保留全部函数体用于
对照。与 `--snapshot` 互斥，拒绝 `--source` 和任何 Service 运行期源声明，并检查 Guest
无 Host imports。源码和显式编入制品的数据模块属于构建输入。旧 snapshot 保留原有来源约定，
不会自动获得执行 `stage` 的权限。所有构建步骤成功后才原子替换输出文件。

P2 使用 UTF-8 JSON 信封 `{version:1,entries:{name:value}}` 写入唯一
`telora.build.information` custom section，名称非空且唯一。RT collector 仅持有编码后的
Rust 字符串，不把语言信息对象当作 B 的根。旧 `telora.service.metadata` 继续兼容，
同一制品禁止重复附加新旧信息段。当前保留不可达 trap stub 与必要的 Ready getter，
尚不保证得到最小制品，也不意味着知识生成算法的耗时已优化。


## 列式 Map 与私有 MapMut

内置 Map(K,V) 沿用 collection element 在前的内部 MIR 参数排列 `[V,K]`，Dict(V) 透明展开为 Map(String,V)。24 字节 value header 保存来源、key 列对象、长度和 value 列对象。GC 的两条 detail 分别指向实际 K 和 V，复用普通数组追踪、固化与恢复。反射对 String 特化提供 Dict(value)，其余提供 Map(key,value)。

可信 native 模块 41 经 `telora_column_map` 向 RT 传入 column_map::Request，包含封闭 key/value/pair 类型编号、宽度、偏移和比较函数。内部 MapMut 用追加的 key/value 引用列和有序 Vec<u32> 索引进行二分插入，finish 整理成紧凑两列。普通 insert/remove/merge 使用私有副本，不改动输入版本。

MapMut 只存在于编译器生成的构造/更新/finish 指令及 RT 中，标准库不声明 MapMut 类型。pipeline 仅使用普通不可变 tuple `(Array(K),Array(V),Bytes,Bool)` 描述 GC 状态布局，内部指令识别受信任 MutBuilder 的封闭结果及布局合同。Bytes 独占缓冲区保存 packed u32 index，按几何级数扩容；只使用计数以内的索引。其原始容量至少 16 bytes，确保统一采用 out-of-line byte view，GC 能追踪缓冲所有权。keys/values 的初始值先复制，finish 关闭 builder 并按 index 复制发布。任何用户可观察的 Map 保持不可变。

公开 collect_map 与 collect_dict 使用同一泛型 builder；ArrayMut 和 StringMut 保持独立。aliasing 优化不作为批量构造的前置条件。自定义 key 默认顺序支持标量与有限产品/newtype；Float 等无隐式全序。非 String Map 的直接 JSON codec 和 host typed input 要求应用显式转为 pairs，Host 输出支持有序 pairs。

构造器 GC 根依据实际调用约定区分：canonical constructor literal 接收函数值，普通闭包及 newtype 构造器接收环境对象。共享 ABI 为 42，编译器与 runner 必须匹配。
