# RFC 0291：独立 Wasm 发布与执行路线

状态：推进中。分支：`feat/wasm-backend`。跟踪：[#186](https://github.com/hh9527/telora/issues/186)。

## 目标与范围

新增独立的 `telora-wasm` 实现，与默认 bytecode、native 路线并存。
隐藏的命令局部参数 `--wasm` 选择新路线，与 `--native` 互斥；不加入
普通 help、README 或 docs。第一期完成 check、eval、eval-with；不扩展
run/serve，不切换默认后端。一个 RFC 和一个 issue 维护全部设计与进展。

首要交付是可落盘、免源码发布的 Wasm 代码产物，同一产物可以在 CLI 和
浏览器执行。初始化在加载后进行；初始化 snapshot 留作第二部分目标。
Wasm 引擎仍可能需要编译，不能将可移植产物等同于无编译启动。

## 编译边界

复用模块加载、resolve、类型求解和 seal，后端输入为 SealedExecutable。
check 使用既有 seal_modules 建立执行闭包。后端只消费已经确定的类型、
函数实例、引用和稳定 ID，不新增推导，不按内置名称猜测身份，不回退
到旧 VM 或 native 执行。静态失败不进入初始化。

Wasm codegen 根据闭合节点机械生成 Wasm 指令。生成与运行分离：产物
拥有运行需要的全部静态信息，执行端不持有 MIR 或源码解析器。
未实现的表达式明确拒绝，不借助其他后端伪装成已支持。
发现动态类型阶段的遗留时删除对应路径，补充语言回归验证；不建立兼容层。

## Runtime 与 host

值、分类对象表、闭包、初始化需求状态及 property 计算位于 Wasm 内部。
不能通过逐次 host 调用旧 Val/Heap 来实现语言运算。host 仅承担外部
输入输出和资源接口，CLI 与浏览器遵循同一协议。

第一版采用 wasm32、单个线性内存，以逻辑区域表达 main/work；不依赖
multi-memory、Wasm GC、线程或 SIMD。沿用稳定 TypeId 与分类 HeapId，
Tuple/Record 共表、Dict 有序 keys/values 双列等已有语义。native 指针
宽度或 Rust 对象布局不属于 Wasm ABI，物理偏移须由 Wasm 布局明确计算。

第一条纵向链路优先使用无 host import 的标量与控制流程序验证真正的
可移植执行，再逐步补齐聚合和 runtime。可复用与目标无关的静态计划，
不强迫 native runtime 变成 Wasm 的依赖。

### Rust RT 与静态链接

运行时采用 Rust 源码实现并编译为 wasm32 对象文件；Telora codegen 输出
带符号表与重定位记录的 Wasm 对象，由 wasm-ld 静态链接为单个发布模块。
不继续把通用容器、编解码等运行时能力扩展为手写 Wasm 指令生成器。
编译端需要链接器；最终执行端只需要 Wasm 引擎，不需要 Rust 或链接器。
RT 与程序共享模块的 memory 和函数表，语言运算不经过 host 回调。

RT 不按 std/ 的模块清单逐个提供同名实现。std 包含模板方法和模板类型，
而 RT 只提供确定的 ABI 原语，可以保持很小。SealedExecutable 已封闭的
类型、布局、函数实例和调用证据，由 codegen 生成专门化胶水，连接 std
与用户态代码；这些生成代码本身也是发布产物的一部分。

例如 array.map[T, U] 的输入/输出元素布局、回调实例和结果 TypeId 由
MIR 决定，胶水据此生成调用、装配结果。RT 可提供存储、分配、调用以及
值得共享的固定 ABI 操作，但不识别模板参数，不在运行时选择或猜测类型。
是否将某段循环抽到 RT，依据 ABI 稳定性和复用价值判断，不以 std 是否
存在同名方法为依据。保留类型绑定的生成逻辑不是保留旧动态类型路径。

2026-09-13：最小对象协议验证通过。examples/link-object.rs 使用
wasm-encoder 生成 linking/reloc.CODE，调用 Rust 编译的 rt_apply，后者
再调用生成对象导出的 telora_callback；wasm-ld 成功链接，Node 执行
返回 42，最终模块没有 imports。Rust 探针位于
tests/fixtures/rust-rt-probe.rs。此前 Rust RT 与 C 对象的数组回调探针
也成功，但两项都不等同于 SealedExecutable 的完整链接支持。

后续先把生成器的函数、数据、函数表引用统一改为符号重定位，明确
Rust 栈、静态数据与分类堆的地址分配，完成闭包间接回调和分配验证，
再将适合固定 ABI 的基础操作迁入 Rust RT，类型绑定的胶水由 codegen
继续生成。保留语言对照测试作为验收，不保留被替代的手写 runtime
作为最终兼容或回退路径。

后续探针已使用独立 object 模块记录直接调用与函数表槽位重定位，
自动计算 code payload 偏移，不再手填偏移。生成对象将函数指针传给
Rust RT；RT 从链接器提供的 __heap_base 后分配数组并通过间接调用
回调生成函数。连续一万次执行得到正确结果，覆盖 memory.grow，第二
实例验证分配状态隔离，最终产物仍为零 imports。这验证基本 ABI 和
链接条件，尚未代表完整 Telora 闭包环境、分类堆或 MIR 生成器迁移。

复现（wasm-ld 可使用 Rust sysroot 下的 bin/gcc-ld/wasm-ld）：

```sh
cargo run -p telora-wasm --example link-object -- /tmp/telora-app.o
rustc --edition=2024 --target wasm32-unknown-unknown --crate-type staticlib \
  -C opt-level=2 -C panic=abort \
  crates/telora-wasm/tests/fixtures/rust-rt-probe.rs -o /tmp/telora-rt.a
wasm-ld --no-entry --export=answer --export=array_answer \
  /tmp/telora-app.o /tmp/telora-rt.a -o /tmp/telora-linked.wasm
node crates/telora-wasm/examples/link-smoke.mjs /tmp/telora-linked.wasm
```

## 产物与来源

落盘产物包含可执行 Wasm、类型及函数索引、必要静态数据、来源位置和
格式/runtime ABI 版本。运行入口使用明确的导出协议，浏览器不需要
Cranelift 或 Telora 编译器。源码文本可不携带；文件标识与位置保留，
源码上下文可作为可选调试资料。免源码发布不等于内容保密。

产物装载验证版本、索引和内存边界；不得序列化 host 地址。布局、初始化、
诊断及调用协议在本 RFC 内随首条实现收敛，不另拆子 RFC。

## 初始化与执行

装载代码和静态描述 → 注入数据模块 → 求值顶层值与 property → 发布
main world → 创建 entry work world → 执行 eval-with。
初始化维持需求驱动、缓存和循环诊断，完成后主动求完应初始化的图。
check 在初始化完成后结束；only-types 保持纯静态路径，不创建引擎。
失败不发布最终结果。第一期不保存部分初始化结果或进程内存快照。

## 资源与工程约束

配额不作为首期主要工作：引擎有简单 fuel/内存限制接口时桥接，否则
明确记录暂未覆盖的部分，不新建复杂精确计费体系。fuel 用于约束失控
执行，不承诺指令级公平计费；浏览器长任务可由 worker 生命周期控制。
仍须保持基本内存安全和边界检查。

Rust 按职责用普通 mod 拆分模块，不用 include! 拼接代码规避行数限制。
优先复用 .telora 对照用例，Rust 单测集中于 ABI、生成器与装载契约。

## 推进与验收

- [x] 独立分支与单一 RFC。
- [x] 单一跟踪 issue、远端分支。
- [x] 最小 SealedExecutable → Wasm → 独立引擎执行；落盘后独立重载。
- [x] 同一产物在浏览器执行，验证无需源码和 Rust host 语言运算。
- [ ] 分类堆、闭包、泛型实例、来源诊断和 runtime 基本操作。
- [ ] 数据注入、property/顶层初始化，隐藏 check/eval/eval-with 接入。
- [ ] 语言与 CLI 对照、发布物重载、浏览器 demo 验收。
- [ ] 分开记录 frontend、Wasm codegen、引擎装载/编译、initialize、entry
  与内存观察；在端到端完成后做性能评估，不每步重复基准。

引擎选型以首条链路的可用性为依据，解释器与 JIT 是运行策略选择，
不改变发布 ABI。最终验收不得以子集支持代替完整 eval-with 语义。

## 首条链路记录

2026-09-13：新增 telora-wasm crate，wasm-encoder 生成真实 Wasm；
Wasmi 2 作为当前测试解释器，不绑定最终 CLI 引擎选型。
最小测试使用只声明 native Int 身份的 prelude，完成正常 resolve/type/seal；
只接纳整数常量导出，metadata/check 初始化与其他表达式明确拒绝。
这不是完整 prelude、CLI eval 或通用初始化的支持声明。

`cargo test -p telora-wasm`：两项通过，验证生成确定性、释放 MIR 后解释
执行、零 host imports 和不支持表达式拒绝。
`cargo run -p telora-wasm --example scalar -- /tmp/telora-wasm-scalar.wasm`
生成独立文件；另起 Node 进程通过 WebAssembly API 加载，返回 42。
浏览器页面在 examples/scalar.html，使用相同 WebAssembly API 和文件选择器；
尚未实测浏览器，Node 的验证不计作浏览器验收。

本阶段没有增加 CLI 参数、runtime 兼容层或新配额系统。
下一步建立携带 TypeId/来源的值 ABI、函数/控制流和 Wasm 内存布局，
再接初始化及 CLI；当前 scalar 导出协议不冻结为最终 ABI。

## 值 ABI 与函数链路

2026-09-13：原先 i64-only 示例协议已移除，统一生成 wasm32 单内存模块。
值头为 source/start/end/TypeId 四个 u32，标量另带一个 u64；函数另带
Wasm function-table index 与捕获环境。没有 host 语言操作 imports。
Wasm 内部的 allocator、间接调用、需求状态表及错误记录随代码一起落盘。
当前捕获环境是线性内存中的指针列，分类表与 main/work 发布仍待推进。

已实现 Int/Float 基本运算、Bool 短路、if、局部绑定、函数调用、递归、
互递归局部闭包、跨调用捕获和预先封闭的泛型函数实例。整数溢出/除零
产生带来源的错误；初始化重复调用使用已计算的值，失败后不再次初始化。
Manifest 保存稳定类型身份及文件/行列位置，不保存源码文本。
Session 从 Wasm 文件独立装载，Wasmi fuel 直接使用引擎接口，未新增计费体系。

`cargo test -p telora-wasm`：5 项通过；语言场景集中在 tests/fixtures/*.telora。
其中覆盖 3 个函数/控制流资产和 8 个算术错误导出、错误位置、失败后重试、
引擎 fuel。当前仍使用精简 prelude；完整标准库、聚合类型、property 和
CLI 开关尚未完成，不能将这些结果视为 eval-with 验收。

## 分类表与浏览器验证

2026-09-13：增加 Wasm 内部的分类表；每个表的槽位固定为
`{ payload_offset:u32, byte_length:u32 }`，槽位可以增长但 HeapId 不变。
Tuple/Record 共表，Array 保存完整元素值并携带 slice 起止范围；String
支持 14-byte inline 与独立字节对象。Dict 为两个完整 ArrayTable 槽位，
keys 按 UTF-8 顺序排列，字段读取生成二分查找。函数改为三个 word 的值，
捕获环境进入独立表；不再把捕获环境的内存地址直接存入函数值。

初始化成功后冻结每张表的已分配前缀，后续调用追加 work 槽位；该实现
没有深复制 main 对象，也暂不回收初始化临时对象或 work 对象。
表增长仅复制槽位描述符，不复制它们指向的对象。后续需要 GC 时再细化
work 管理，本期不以无限次服务为验收目标。

独立 Session 可按 manifest 中的封闭签名输入 JSON 并调用函数。该接口
用于底层产物验证，不等同于 CLI eval-with 的 Env/Source/entry 适配。
外部输入写入 Wasm 内存，表注册仍调用 Wasm 函数；输出只在外部 JSON
边界读取，内部计算不经过 Rust Value 或旧 VM。

`cargo test -p telora-wasm`：7 项通过。新增 Array/Record/Tuple/String、
Dict 双列与查找、32 次外部输入调用及冻结前缀保持测试。仍是精简 prelude。

实际浏览器：Playwright Chromium 153.0.8010.12，执行
examples/browser-smoke.mjs；聚合产物输出
`[42,[{"label":"短文本","score":19},{"label":"a longer string stored in the string table","score":23}],null]`，
函数产物输入 `[{"name":"浏览器输入","values":[22]}]` 得到
`{"name":"浏览器输入","total":42}`。同一批文件也由 Node WebAssembly API
执行通过。页面 examples/scalar.html 通过 module Worker 执行，有手动停止
按钮；host.mjs 只提供基于持久 schema 的外部传输，不实现语言运算。

浏览器复验需以 HTTP 提供 examples 目录，然后执行：

```sh
node crates/telora-wasm/examples/browser-smoke.mjs \
  http://127.0.0.1:18761 /tmp/telora-wasm-aggregates.wasm /tmp/telora-wasm-input.wasm
```

脚本从普通 playwright 包导入测试工具，也可用 TELORA_PLAYWRIGHT_MODULE
指定安装位置。浏览器依赖不进入 Rust workspace 或运行时发布物。

下一阶段仍需完成完整 prelude/property、代数类型与 native 标准库操作，
再接数据模块、CLI 和完整语言验收；本阶段未新增隐藏参数占位实现。

## 完整 prelude、代数类型与 property

2026-09-13：删除测试专用的精简 prelude，测试与产物示例统一使用真实
标准库清单及隐式 prelude。新增普通 Rust 模块 enums、patterns、natives、
properties，没有使用 include! 拼接代码。

Wasm 指令支持 enum/newtype 构造、模式匹配、guard、if-let、let-else、
Option/Result 的传播。enum payload 根据封闭布局使用内联完整值或
ValuesTable；newtype 使用独立分类表。模板参数继续来自 sealed 实例。

property provider 作为普通生成函数执行，配置参数通过闭包保存，声明链
按顺序归约。property 与顶层值共用需求状态表，查询消费封闭的 owner 与
property 类型身份。标准 property 声明能力及类型/字段/variant 查询原语
按已准入的 native 模块身份连接，不根据用户符号拼写猜测。

`cargo test -p telora-wasm`：9 项通过。新增语言资产覆盖带 payload 的
enum、newtype、Option 传播，以及依赖顶层值的两个 property provider
归约为 42、字段上下文查询返回字段名。浏览器传输层增加相同 schema 的
enum/newtype 输出支持；新增场景尚待实际浏览器复验。

这仍不是完整 CLI 验收：construction check、其余标准库操作、数据模块
注入和隐藏 check/eval/eval-with 桥接尚未完成。Session 的直接函数调用
不能代替 std/entry.Eval 的配置、环境及数据源语义。未做阶段性能基准。

## CLI、数据注入与 Eval 纵向链路

2026-09-13：隐藏的命令局部 `--wasm` 已接入 check、eval、eval-with，
与 `--native` 互斥。only-types 不创建 Wasm 引擎；run/serve 不提供此开关。
这些参数没有加入普通 help、README 或 docs。

产物记录从准入模块导出中解析出的 std/value.Value、std/entry.Eval 身份，
以及数据模块名称、导出 SymbolId 和封闭类型。eval 要求确切 Value 类型；
eval-with 要求确切 Eval 类型，按其 config 检查 sources/envs/args，再将
Context 注入 Wasm 并调用已生成的 evaluate 闭包。没有以任意函数替代 Eval。

数据模块从共享的 ValidatedDataPlan 直接进入 Wasm 分类表，保留数据与
对象键的来源位置，缓存已物化的数据节点；不经过旧 Val 或递归 JSON 中间树。
生成的 telora_inject_data 根据稳定 SymbolId 注入需求槽，只允许在初始化
之前恰好一次。遗漏注入会使初始化失败；property 可以依赖已注入的数据。
模块注入后，初始化完整求出所选执行闭包里的顶层值及 property，并冻结表前缀。

发现共享数据计划仍有通用 Atom(String)/TaggedString 后，直接删除这些
表达，改成 Null、Bool 和具有四种明确身份的 TemporalKind。JSON/YAML/TOML
解析器和现有消费者一起更新，不为新后端保留动态标签兼容入口。

验证：telora-wasm 的 11 项测试通过；CLI 对照测试验证默认后端与 Wasm
的 eval/eval-with 结果一致，覆盖数据模块、依赖数据的 property、外部 YAML、
声明的环境与参数，以及隐藏/互斥参数、check 和 only-types。共享数据回归
分别通过 data（18 项）、toml（9 项）、yaml（6 项）、json（19 项）筛选；
这些筛选集合有重叠，不作为独立用例总数相加。

实际 Chromium 再次验证聚合产物、普通函数输入和标准 Eval 的 Context
输入；同一免源码产物返回预期的嵌套 Value。浏览器页面接受参数数组或
Eval 上下文对象。复验脚本可以追加第四个参数（Eval 产物文件名）。

这是命令纵向链路完成，不是完整语言验收：construction check、诊断宏、
标准库操作和剩余表达式仍需逐项补齐。不能将已有子集成功视为 #186 完成；
尚未进行最终性能与内存观察。

## 构造检查与诊断宏

2026-09-13：新增 checks、diagnostics、diagnostic_output 普通模块。
执行计划不再拒绝 construction check，而是将每个封闭 checker 注册为
需求初始化的闭包；构造点按稳定 owner/site 查找并调用。支持泛型 checker、
具名 record 的 Unchecked 视图、newtype 和 enum variant 检查，以及 MIR
已记录的构造转换边界。返回契约严格为 Result((), BlameError)。

BlameError 和诊断事件进入 Wasm 分类表。blame! 保存消息及 subject 来源；
raise!/fail! 记录失败并终止，warn! 记录警告并返回 None；unwrap!/ok_or_warn!
直接消费既有 MIR 展开。检查返回 Err 时在构造位置报告一次，带上原数据
来源；已失败调用向上传播零指针，不重复生成诊断。CLI 的 check 输出
结构化诊断，eval/eval-with 在最终结果发布之前处理诊断。

分类表与诊断记录 ABI 已改为版本 2，旧实验产物明确拒绝加载，不提供
兼容解码。浏览器使用相同记录，页面单独展示诊断，不混入结果 JSON。

验证：telora-wasm 13 项通过；CLI 两项通过，其中一项逐项比较默认后端
与 Wasm 的 warning/error/来源标签。语言资产覆盖泛型构造检查、variant
拒绝、宏失败与警告，验证失败重试不重复报告。实际 Chromium 复验聚合、
普通函数、标准 Eval、初始化警告及构造拒绝，均通过。

后续仍需补齐 std/_rt.with_diagnostics 的显式诊断捕获、标准库操作、
剩余表达式及完整语言对照；此阶段没有宣称所有诊断/恢复能力已完成。
最终性能和内存观察仍待完整链路覆盖后进行，#186 保持推进中。

## Array 语义对照资产

Rust RT 链接迁移之前的本地 Array 工作已覆盖 std/array 的 14 个操作，
包含回调、短路、fold_control、zip 长度不等、concat 和 flat_map。
flat_map 每个输入只执行一次回调；测试使用 warning 计数确认这一点。
泛型 namespace 字段引用直接消费 MIR 已封闭的实例引用，修正 array.map
这类调用遗漏实例证据的问题。telora-wasm 14 项库测试通过。
这些语言资产继续作为后端改造的对照；Array 的类型绑定逻辑属于 codegen
胶水，共享基础操作是否下沉 RT 单独判断，不要求逐方法翻译成 Rust。
这也不表示其他标准库操作已经补齐。

## 实际编译路径接入 Rust RT

2026-09-13：compile_executable 已切换为生成可重定位对象，再静态链接
Rust RT。函数调用、全局变量、间接调用类型/表以及闭包函数指针分别
记录重定位；函数表槽位不再与 codegen 的函数索引混用。发布物仍为单个
自包含 Wasm，执行端不进行链接、不需要 Rust 工具链。

rt/ 使用普通 Rust 模块实现六个基础辅助函数：分配器、分类表插入/查询/
冻结、闭包调用、字符串比较。已删除对应的手写 Wasm runtime、tables、
strings 模块及旧 bump global。物理布局常量由 codegen 与 RT 共用，
不是复制两套 ABI 定义。剩余语言支持通过小型 RT 与类型绑定胶水共同
补齐，不把 std 的全部能力收进 RT。

内存布局明确保留分类表描述符和整图需求槽位所在的低地址前缀；链接器
使用 global-base 放置其后的 Rust 静态数据，显式 no-stack-first 令 Rust
栈在静态数据之后，分配器从链接器的 __heap_base 开始。main/work 仍按
分类表冻结前缀区分，不引入深度复制或运行时类型猜测。

构建端新增 wasm32-unknown-unknown Rust 标准库目标，build.rs 将 RT
编译成静态库并嵌入编译器；编译 Telora 程序时调用 wasm-ld。默认使用
构建工具链附带的链接器，TELORA_WASM_LD 可指定部署环境的链接器。
链接失败不回退旧实现，并保留对象输入路径供诊断。此依赖只属于编译端。

验证：14 项库测试、两项 CLI 对照通过；实际 Chromium 对重新生成的
聚合值、闭包参数、标准 Eval、构造拒绝与警告全部通过。独立进程将
链接器路径设置为不存在后，仍能加载并执行已有产物。现有冻结前缀测试
补充真实 memory.grow 后调用闭包及分配区内容不被覆盖的验证。

此处尚未完成标准库剩余操作、诊断捕获与完整 eval-with 语义覆盖。
六个辅助函数迁移不等同于完整语言支持，最终性能观察仍待完整验收。

## 诊断捕获前的需求失败闭合

诊断捕获不能仅清除 session 失败标志：失败的需求也必须保留明确结论。
需求槽现在区分 Empty / Running / Ready / Failed，第二个 word 在 Ready
时保存值地址，在 Failed 时保存原失败记录地址。生成的需求函数将所有
正常返回与失败传播汇入统一出口；展开后不残留 Running。再次读取 Failed
传播同一失败身份，不重算、不伪报循环，也不重复添加诊断。

现有 14 项库测试通过，另增失败展开状态验证通过。后续捕获胶水仍需
隔离诊断范围、恢复外层执行状态，并按封闭的 Diagnostic/Label/SourceRange
布局装配语言值。来源名称还需要覆盖运行前注入的数据源，不能只固化源码
模块名。引擎级 fuel/内存 trap 不应伪装成普通已恢复的语言失败。
此处尚未实现 call_with_diagnostics 的完整调用契约。

诊断来源名称现有固定 ABI：初始化前登记 SourceId 与 UTF-8 范围，RT
查询返回范围地址，不构造语言类型，也不推导 TypeId。源码名称来自产物
manifest，注入数据名称沿用整图 SourceDatabase；同一 ID 对应不同名称
明确报错，未知名称使用 source:<id>。后续类型胶水消费此范围生成 String
和 SourceRange。Rust host 与浏览器均已接入初始化前登记。

协议升为 ABI 3，旧实验产物明确拒绝，不添加兼容路径。15 项库测试、
两项 CLI 对照、实际 Chromium 聚合/闭包/Eval/诊断复验通过；数据来源
测试补充了名称查询、未知 ID 回退和冲突拒绝。原测试里独立创建数据源
数据库导致 ID 与源码冲突，已改为沿用整图来源空间。

## 类型绑定的诊断捕获胶水

call_with_diagnostics 现已生成完整的范围捕获胶水：保存外层阶段/失败身份，
执行回调，将范围内的诊断按封闭 Diagnostic/Label/SourceRange 布局装配为
语言数据，从外层诊断序列移除已捕获部分，然后恢复外层状态。正常返回
Ok((value, reports))，语言失败返回 Err(reports)，Never 回调没有成功构值
路径。泛型参数与字段布局均在 codegen 确定，不交给 RT 解释。

来源查询和编号标签文本通过固定 ABI 返回 UTF-8 范围；类型胶水负责
String、记录、数组和 Result 构造。主标签、subject 来源及编号保留，
重复 subject 去重，外层先前的警告不会被内层捕获吞掉。引擎 trap 直接
传播，不转换为普通可恢复 Err；捕获后的新语言失败仍然终止求值。

验证：16 项库测试及既有两项 CLI 对照通过；语言资产覆盖成功带警告、
失败带警告、嵌套范围、Never、算术失败、重复调用、捕获后的未捕获失败，
以及进入捕获范围后耗尽 fuel。实际 Chromium 验证初始化时捕获失败并将
诊断作为正常数据返回，来源和主/次标签保留，捕获诊断不泄漏至外层输出。

std/_rt 维持私有模块，CLI 不新增直接导入通道；捕获资产通过完整内置图
测试。此进展不代表其他标准库与表达式缺口已完成，#186 仍保持推进中。

## Dict 操作与有序双列

新增 std/dict 的 keys、values、pairs、from_pairs、merge、map_values、
filter、fold、get 胶水。keys/values 只构造 Array 描述符，直接共享列的
HeapId；get 与字段读取共用二分查找。merge 线性归并两个有序输入，
同名键取右侧；回调按键序执行，filter 保持相对顺序。

from_pairs 先构造独立的键/值指针对，Rust RT 按 UTF-8 键序原地排序
这张临时表，返回重复键证据；类型胶水再按已知宽度构造双列。输入数组
及 pair 对象不修改。RT 排序和诊断文本格式化均是固定 ABI，不读取
语言 TypeId 推断布局。重复键消息包含转义后的键名，保留其来源证据。

17 项库测试、3 项 CLI 测试通过。语言资产覆盖九个操作、Unicode 键序、
右侧覆盖、空字典、缺失键、重复键捕获、变宽 Array 值映射与回调次数。
200 项逆序输入验证排序及原输入不变；公开 CLI 的结果与默认后端一致。
仍需补齐其他标准库/表达式，并在总体验收时覆盖 Never 等边界组合，
不以本阶段的操作覆盖代替 #186 全部完成。

## String 固定原语与类型胶水

新增 length、starts_with、ends_with、contains、join、join_lines、split、
lines、replace、indent、ensure_trailing_newline、trim_margin。Rust RT
通过三个固定 ABI 入口完成 UTF-8 查询、文本生成和切分，返回标量或原始
UTF-8 范围；codegen 根据封闭签名构造 String、Array(String) 及来源头，
生成参数诊断。RT 不接收模板参数，也不承担标准库模板实例化。

18 项库测试、4 项 CLI 测试通过，公开 String 操作与默认后端结果一致。
覆盖 Unicode 字符计数、空分隔符、CRLF、尾部空行、缩进与 margin 错误。
泛型 parse/parse_with 尚未实现，后续须利用已封闭的解析器身份生成胶水，
不能在 RT 中按类型名称猜测转换规则。此阶段仍不代表整体后端验收完成。

## 词法路径与序列展开

std/path 的 join、normalize、parent、file_name 已接通。固定 Rust ABI
只处理 UTF-8 词法路径并返回文本范围或缺失；生成胶水构造 String 与
Option(String)。路径不访问文件系统，不使用宿主平台的路径规则，保留
绝对路径覆盖、相对 ..、根目录、空路径、Unicode 与反斜杠普通字符语义。

数组和元组构造统一处理普通项及展开项，移除原来的仅普通项生成路径。
所有项依次求值一次；数组按已知元素宽度拼接，元组按封闭字段偏移构造。
TypeOf 到 Type 的合法适配由静态证据生成；元素来源头保留，新容器记录
自身表达式来源。空 Unit 展开仍求值，失败操作数阻止后续项执行。

同时修正不可构造乘积类型的生成：含 Never 的元组在终止贡献项之后
不再分配布局；诊断捕获与 enum 胶水依据布局的不可构造状态处理，不只
识别裸 Never。不存在的成功载荷不会被物化为占位值。

20 项库测试、6 项 CLI 测试通过。语言资产覆盖泛型、空/嵌套序列、名义
字段类型、元数据适配、警告求值顺序、失败短路及精确来源偏移。两组
落盘产物另由 Node WebAssembly 独立装载执行，确认零 imports；本阶段
未重复性能基准，也不将这些结果代替剩余语言覆盖与最终浏览器验收。

## Record 投影、更新与 Dict 展开

Record 普通字段、展开项、字段投影和 `<~` 更新已统一消费编译期字段
贡献。按源码顺序求值后，只将最终胜出字段适配到目标的封闭类型，并按
既定布局构造；被覆盖项仍执行，也仍可能失败。投影接收者只求值一次，
允许空投影和同源字段重命名至多个目标。`<~` 维持基类型身份，完成的
构造仍执行既有 sealed checker，不在运行时重建字段或猜测类型。

Dict 展开复用 std/dict.merge 的有序双列归并生成器，后项覆盖前项；
普通字段按静态键序构造，所有值按封闭元素宽度存储。仅含单个展开项
的字面量共享原有列的 HeapId，只产生带自身来源的新容器头，不深复制
堆对象。删除原先仅普通 Record/Dict 字段的生成路径，没有兼容分支。

21 项库测试、7 项 CLI 测试通过；新增语言资产覆盖名义/泛型字段、
上下文中的嵌套构造、32-byte 数组值的 Dict、顺序、被覆盖失败、构造
检查次数，以及字段/更新容器/展开容器的精确来源。普通 CLI 输出与
默认后端一致，落盘产物独立 Node 执行通过且零 imports。此阶段不新增
RT 入口；非标量比较、Fmt/插值及其余标准库仍待补齐后总体验收。

## 按封闭类型生成比较器

`==`、`!=` 和 std/eq.equal 统一使用按 TypeId 规划的专用 Wasm 比较器。
规划沿封闭成员/变体类型收集有限函数集，递归类型通过函数调用连接，
不在编译时无限展开，也不在 RT 中解释类型描述或遍历 host 值。
覆盖标量、Unit、String/Bytes、Array/Dict、Tuple/Record、newtype、enum
及元数据；字节字面量同时接入 BytesTable。资源类型的专用语义仍随其
能力接入，不用通用 HeapId 比较冒充 Fmt、Regex 等内容比较。

函数比较使用函数表身份与环境身份。无捕获闭包现在也分配空环境槽位，
区分同一代码重复求值得到的函数实例；共享引用与已初始化泛型实例保持
身份。聚合比较保留浮点比较语义，不以指针相同跳过成员比较。

跨后端对照发现并补齐浮点失败边界：产生 NaN/Infinity 的算术运算报告
NonFiniteFloat，可被诊断捕获；不再把非有限数值发布为正常 Float。
41 项语言比较场景与默认后端一致，22 项库测试、8 项 CLI 测试通过，
新增 CLI 浮点失败对照也通过。落盘产物另在 Node 执行 41 项比较，
确认零 imports。此阶段未增加 RT ABI，也未重复性能基准。

最终验收须额外核对初始化范围：测试观察到默认 eval 会求值同模块中
未被选择导出引用的顶层值，而当前 Wasm seal_export 只包含所选依赖
闭包。此差异尚未处理，不能以正向比较用例通过代表初始化语义已对齐。
资源类型比较、Fmt/插值、其他标准库与最终性能/浏览器验收仍未完成。

## Fmt 节点、插值与封闭 trait 成员

2026-09-13：接通 from_string/from_int/from_float/concat/render 和插值。
Rust RT 使用固定的格式节点 ABI：操作码、第一参数指针、第二参数指针；
Fmt 值保存 FormatTable 的稳定 HeapId。参数指向 Wasm 内已经生成的不可变
值，不复制其堆对象。FormatTable 与其余分类表一起冻结初始化前缀。
新增分类表使产物 ABI 升至 4；装载端和浏览器 transport 同步更新，旧实验
产物明确拒绝，不引入兼容路径。

生成器检查已封闭签名，构造格式节点和 String 结果；RT 只按固定操作读取
参数，不查询 TypeId 或实例化模板。concat 在构造时检查两列长度；render
维持递归上限，超过上限生成可捕获的语言失败，不用引擎 trap 代替诊断。
插值按源码顺序求值全部片段后再渲染，片段的 String/Fmt 操作码由 MIR
决定。数字文本沿用 Rust Display，包括整数下界、浮点负零与小数。

插值验证同时补上了已有的 trait 成员消费缺口：通过已封闭的 implementation
SymbolId/GenericInstanceId 获取实现记录，按确定字段偏移读取方法。实现
绑定沿用普通值初始化，不建立运行时方法搜索。语言用例覆盖普通自定义
Display 与带 Display 约束的泛型 trait 实现。

23 项 Wasm 库测试、9 项 CLI 对照测试通过。13 项格式化结果与默认后端
一致；127 层格式节点可渲染，128 层产生可捕获诊断，失败后外层继续执行。
独立 Node 进程重载 ABI 4 产物，13 项结果通过且 imports 为空。本轮未做
性能基准，ABI 4 的实际浏览器全流程复验留在后续验收。

初始化范围调查确认：共享 SealedMir::seal_export 明确裁剪普通顶层导出，
保留 concrete property/check 根；Native 和 Wasm 都消费这条规则。默认
解释器仍从 ExecutionGraph 安装并初始化更大的图。本路线继续遵循共享
SealedExecutable 的根集合，不为对齐默认解释器而额外执行未准入导出。
最终对照须明确这项既有差异，不能将所有后端的初始化范围宣称为相同。

Fmt.prepare、DisplayBy 所需的 Dyn/类型反射、Fmt 结构比较以及其余标准库
操作仍待实现；当前进展不是完整格式化标准库或完整 eval-with 验收。

## 模板准备与显式 Dyn 投影

2026-09-13：Fmt.prepare 接通固定 Rust ABI 的模板扫描器，输出两份 UTF-8
span 列表或错误消息。先验证并计数，再分配连续片段存储；字段名引用已有
输入文本，转义后的字面片段写入一次。生成胶水依据闭合签名构造
Tuple(Array(String), Array(String))，并与 String.split/lines 复用同一份
span 列表装配逻辑。错误经普通诊断路径报告，std/fmt 的公开导出不变。
测试直接选取已解析的私有 native 声明作为 sealed 根，覆盖空文本、Unicode、
转义花括号、连续／重复字段，以及未闭合、嵌套、孤立右括号和非法字段。

新增 std/dyn 的 pack、project_with、desc、四项标量 check；公开的泛型
project 仍由标准库函数体实现。Dyn 遵循既有 40-byte 候选布局，采用明确的
boxed 存储：TypeId、storage=1、ValueTable HeapId。装箱登记已有不可变值的
指针与确定宽度，不深复制描述符或对象；精确投影仅比较 TypeId 并构造
Option(A)。Dyn 相等性按装箱身份判断，重新装箱产生不同身份。
这些操作全部是类型绑定的生成代码，不需要新增 Dyn RT 类型推断或转换。

18 项语言检查涵盖标量、Unit、Never 投影、Array、名义 Record、嵌套 Dyn、
函数投影后调用与装箱身份；默认／Native／Wasm CLI 三条路径一致。
25 项 Wasm 库测试、10 项 CLI 测试通过；独立 Node 重载产物，18 项 Dyn
检查通过且 imports 为空。ABI 仍为 4，本轮未新增物理分类表或外部协议。

DisplayBy 仍需类型描述查询与 Dyn 成员访问；Fmt 结构比较、其余标准库、
完整浏览器及性能验收也仍未完成。下一步应把已封闭的类型描述作为静态
数据供 Wasm 查询，而不是在 RT 重建或推导类型。

## 静态类型描述与 DisplayBy 纵向链路

2026-09-13：将 TypeImage 编码成平坦的只读表，覆盖 kind、children、
opaque_name、resolve_raw、fields、variants。每个 TypeId 对应定宽表项，
子类型／成员／名字采用表内相对偏移；名义类型的 body 仍引用已封闭的
TypeId，不在 RT 展开或重建类型。只有准入图消费这些查询时才生成表。
Wasm 对象增加 data symbol、segment-info 和 MEMORY_ADDR_SLEB 重定位，
由 wasm-ld 放置最终表基址。装载直接获得静态数据，不逐条执行代码建表。

查询结果的 enum、Type、Array 与 descriptor Record 由类型绑定胶水装配。
输入元数据的来源保留到返回值及成员中。Dyn.get_field_value 使用同一张
表中的确定字段偏移和宽度，登记原字段的引用，保持其来源且不复制堆图。
非法接收者、索引和字段／变体查询走可捕获的语言诊断。

DisplayBy 已打通：模板准备、顶层 property 初始化、嵌套 property 查找、
Dyn 字段读取、基本类型投影和 Fmt 渲染均在 Wasm 内执行。覆盖重复字段、
转义括号、嵌套 Endpoint/Service、负零，以及显式 Display 实现优先。

纵向用例发现并修复一处共享类型求解缺口：泛型 Option 解包后读取嵌套
Array(String) 字段，与 [] 分支合流时，空数组曾在成员证据到达前被默认
为 Array(Never)。空数组现在先保留元素空槽，等待尚未完成的成员、调用、
实例化或合流证据，再做 bottom 默认；不是在 codegen 改猜返回类型。
Wasm 字段／tuple 投影也核对布局中的真实字段类型与封闭表达式类型。
新增 .telora 语言回归，成功／空分支都验证，保留严格输出类型检查。

验证：346 项核心 Rust 测试、29 项 Wasm 库测试、97 项完整 CLI 测试通过；
408 个语言验收入口通过，其中新增 empty_arm_waits_for_generic_member_evidence
通过。30 项类型描述检查及 DisplayBy 输出与默认后端一致。独立 Node 与
实际 Chromium 均重载同一个 DisplayBy Wasm 文件，初始化和嵌套渲染通过，
最终模块零 imports；浏览器不读取 MIR 或 .telora 源码。ABI 保持 4。
本轮未做性能基准。

额外 Native 对照在 `td.children(Array(Int).type) == [integer]`（integer
显式标注 Type）处报告值类型标记不符，尚未进一步修复；本轮的一致性
证据仅包含默认／Wasm，不把 Native 计入这组通过结果。Native 实现未修改。

Dyn 其余观察／变体访问、Fmt 结构比较、regex/parse/codec 等标准库操作，
以及最终发布和分阶段性能验收仍待完成，不能将 DisplayBy 通过等同于
完整 eval-with 验收。

## Dyn 变体读取

get_variant_index/get_variant_payload 已消费封闭类型表中的变体定义与
payload 存储方式；类型表增加确定的值宽度，不在执行时计算布局。
Bool、无 payload 变体、递归 tuple payload、Some(()) 与标量 payload
均走同一生成路径。字段和变体读取共用引用装箱，保留原值来源，
不深复制对象图，也没有增加 Rust RT ABI。

负数及超 u32 索引、非 Enum 接收者和预期变体不匹配产生可捕获诊断。
验证包括 30 项 Wasm 库测试，以及默认／Wasm 的 CLI 变体、类型反射、
DisplayBy 对照；另验证标量 payload 的诊断来源。未重复性能基准。
Dyn 其余观察操作和前述标准库、发布验收仍待完成。

## Dyn kind 与 Result 查询

Dyn.kind 已依据静态类型描述分类，名义类型通过封闭 body 获取形状；
enum 依据当前变体是否定义 payload 返回 ValueKind.Atom/Tagged。
这里的名称是现有反射枚举的成员，不引入表面 Atom/Tagged 类型或类型猜测。
覆盖 TypeOf、标量、Array/Dict、名义 Record/Newtype、Unit、Bool、
带 Unit payload 的 enum、Dyn、函数与 opaque Fmt，共 16 项语言检查。

tag_raw/payload_raw 与显式变体读取共用生成路径。公开 tag/payload 的
AccessError 装配仍执行 std/dyn 中已经实例化的 .telora 函数；类型不符
返回 Result.Err，保持原 Dyn 身份，不发布失败诊断。递归 payload 保持
原引用；没有新增 RT 操作。

对照发现默认解释器先看旧值的 atom/tagged 存储，再检查类型身份，导致
Int 的变体查询暴露内部存储诊断。现先以封闭类型判断操作是否成立，
非 Enum 统一返回 Dyn variant access expects Enum，之后才校验存储。
没有保留旧诊断兼容分支。

验证：346 项核心测试、31 项 Wasm 库测试通过；默认/Wasm CLI 中 16 项
kind 与 13 项变体/Result 查询对照通过；完整 97 项 CLI 测试（含语言验收）
通过。独立 Node 重载同一产物，13 项
检查通过且零 imports。没有重复性能基准。fields/field、array_items、
tuple_items 及先前列出的其余标准库和最终发布验收仍待完成。

## Dyn Array / Tuple 观察

array_items_raw/tuple_items_raw 已接通，公开包装继续使用 std/dyn 中的
已实例化函数。Array 消费静态元素 TypeId 和宽度，并遵守描述符的
start/end 范围；Tuple/Newtype 消费封闭子类型列表。结果只新建 Dyn
描述符数组，各元素登记原值引用，不复制引用的堆图，不增加 RT ABI。
Unit 不读取对象句柄；空 Array(Never) 不产生不可能的元素值。

验证：33 项 Wasm 库测试通过，默认/Wasm CLI 的集合、变体、kind、
反射与 DisplayBy 对照通过。语言用例覆盖异构 Tuple、嵌套数组、
Newtype、空集合和错误结果的原 Dyn 身份。Array slice 尚无源码语法，
使用 ABI 测试将四元素数组限制为 [1,3)，确认仅观察中间两个元素。
另补 Array/Tuple 元素诊断指向原始字面量的来源验证。

fields/field、Fmt 结构比较、regex/parse/codec 等标准库能力，以及
最终发布/浏览器/分阶段性能验收仍待完成；本轮未重复性能基准。

## Dyn 命名字段观察

fields_raw/field_raw 已接通，覆盖名义 Record 和有序 Dict。Record 消费
MIR 中排序后的成员表及偏移；Dict 枚举既有键列，单字段查询复用二分查找，
元素步长来自静态类型描述。结果只登记原字段引用，保持来源和对象共享。
空 Record/Dict(Never) 不读取不存在的字段。缺失字段返回带原 Dyn 的
AccessError；RT 仅扩充现有诊断格式化入口的操作码，不承担类型绑定。

34 项 Wasm 库测试通过，默认/Wasm CLI 字段观察等对照通过，新增 10 项
语言检查覆盖排序、异构字段、空集合、二分查找命中和缺失字段错误。
追加 field/fields 取出值的来源验证通过，诊断指向原字段字面量。
至此 std/dyn 当前声明的 native 操作均已具备 Wasm 实现；这不是整个
后端的验收结论。Fmt 结构比较、regex/parse/codec 等标准库能力及最终
发布/浏览器/分阶段性能验收仍待完成。本轮未重复性能基准。

## Fmt 结构比较

Fmt equality 已按固定节点操作及参数递归比较，复用类型专用比较函数。
String 节点比较文本，Int/Float 节点比较原始 bits（Float 区分正负零），
concat 比较字符串列和子节点列。同一节点引用直接相等；来源不参与比较。
不以渲染结果替代结构身份，不新增 Rust RT ABI。

35 项 Wasm 库测试通过。默认/Wasm CLI 对照覆盖 13 项格式比较，包括
渲染相同而结构不同、concat、嵌套在 Array 中及正负零。另以 20M fuel
库测试验证 140 层嵌套的相等与不等，不套用渲染的 128 层限制。
整组深度用例曾耗尽 CLI 固定 1M fuel，因此不计作默认配额下通过；
产品配额保持不变。当前比较仍受引擎 fuel/栈限制，未增加图访问缓存。

regex/parse/codec 等标准库操作及最终发布/浏览器/分阶段性能验收仍待
完成。本轮未重复性能基准。
