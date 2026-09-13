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

2026-09-13：最小对象协议验证通过。examples/link-object.rs 使用
wasm-encoder 生成 linking/reloc.CODE，调用 Rust 编译的 rt_apply，后者
再调用生成对象导出的 telora_callback；wasm-ld 成功链接，Node 执行
返回 42，最终模块没有 imports。Rust 探针位于
tests/fixtures/rust-rt-probe.rs。此前 Rust RT 与 C 对象的数组回调探针
也成功，但两项都不等同于 SealedExecutable 的完整链接支持。

后续先把生成器的函数、数据、函数表引用统一改为符号重定位，明确
Rust 栈、静态数据与分类堆的地址分配，完成闭包间接回调和分配验证，
再将现有 runtime 逐项迁入 Rust RT。保留语言对照测试作为迁移验收，
不保留旧手写 runtime 作为最终兼容或回退路径。

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
