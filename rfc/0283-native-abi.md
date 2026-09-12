# RFC 0283：Native 值、调用帧与运行时 ABI

> 后续决议：RFC 0289 移除 cast。本文涉及 cast 的段落仅为此前的实施记录。

- 状态：本期已实施并验收；目标为 64-bit little-endian host，不承诺跨 target ABI
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#180](https://github.com/hh9527/telora/issues/180)

## 动机与范围

闭合 MIR 如何成为机器码与 Rust runtime 之间唯一的数据契约。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 物化值保持 loc:[u32;3] + TypeId:u32 的 16 字节头部，data 宽度按 RFC 0281；Unit 16 字节、Never 无运行时值，不能分配伪值；不假定所有类型最多 4 word。
- 定义参数、返回值和临时值的布局与对齐。首版每个临时值独占固定槽位，不做活跃区间复用；标量中间运算可采用 SSA，但物化与报错时不能丢失来源。
- 候选内部 ABI 为 function(context, args_ptr, result_ptr, closure_ptr) -> status；第四个指针借用调用期闭包描述符，无捕获的 host 根调用可以为空。明确调用约定、指针长度/生命周期、重入、递归、间接调用、错误状态及无结果路径。
- TypeId 直接使用 SealedMir 身份，HeapId 区分 main/work 并结合对象类别解释。具体 world 编码须在本子 RFC 实现前定案，不预设最高位方案。
- 明确跨 helper 的 panic 隔离、溢出检查和来源三元组映射；禁止 Rust panic 跨生成代码 ABI 展开。

## 实施计划

### 本期验收结论（2026-09-13）

`abi/tests.rs` 验证混合宽度槽位、TypeId/来源保真、独立递归 activation、
未初始化结果拒绝、world 引用范围和 helper panic 边界。
真实机器码测试覆盖直接/间接递归、异宽参数与返回、闭包发布后调用、Never
失败不读输出、fuel/栈预算退出归还，以及不可捕获 abort 的单次来源诊断。
相关实现已在 `573a229` 及其前序提交中接入隐藏 CLI；完整 CLI 84 项通过。
无 jit feature 的 ABI/runtime 25 项与 3 项独立实验通过；开启 jit 为 139 项与
3 项实验通过。此处完成的是 ABI 子项，不代替整个 native 路线验收。

以下记录中的 regex 内存近似量和 helper 临时分配限制属于 runtime 的资源审计，
不改变 ABI 状态、参数布局或调用生命周期契约；不将逻辑配额称为物理 RSS 上限。

### 首批确定的 ABI v1 契约

独立 `telora-native` crate 消费公共 SealedMir 和候选布局，不依赖旧 VM/Val/Heap 接口。当前支持 64-bit little-endian host；来源三元组为 SourceId/start/end，SourceId=0 且 offsets=0 表示无来源，不截断 SourceId。TypeKey 直接保存 seal 后的 TypeId 数字，不重新编号。

HeapRef 的高位选择 work（1）或 main（0），低 31 位为分类表槽位，零号槽位有效。word 内仍用 u32 保存引用，Dict 两列引用均采用该规则。world 世代由执行 context 管理；句柄不得脱离所属 session 或跨回收保留，后续回收必须重写根。

参数/局部槽位按完整值 word 宽度顺序排列，全部 8 字节对齐；Never 无槽位。Activation 首版使用每调用独占的固定 Box 缓冲区，递归不因共享 Vec 扩容导致指针失效；生成代码可使用等价的原生栈帧。公共检查入口拒绝未写入槽位和异类型写入。

status 使用 u32：0=Success，1=Failed；只有 Success 允许读取结果缓冲区。原始 ABI 指针的长度由已闭合函数签名决定，不得缓存调用期参数/结果指针。host helper 的 Rust panic 在边界捕获并转为 Failed（abort 型 panic 不可恢复）；原始失败记录一次，传播 Failed 不重复诊断。完整 FFI 调用验证由首个 JIT 模块继续落实，不以数据结构单测代替。

会话 fuel 存于 CallContext，初始化、按需回调、发布和 entry 调用共享同一计数。
生成代码现按每次函数／初始化器调用扣减一个单位，已移除逐 HIR 表达式扣费；
直线表达式和未采取的分支不额外扣费。dispatcher 不另增这一调用计数，真正进入
的函数体执行检查，因而递归及集合 callback 仍受限。耗尽后 context 保持 Failed，
仅首次记录来源诊断，不读取输出缓冲区，并退出已进入的帧。CLI 采用会话配置的
fuel 上限。下述 helper 的细粒度计数已经移除，不据此承诺与旧路线的预算数字完全相同。

调用深度现由同一 CallContext 维护：默认上限为 128 个生成函数／property 初始化器的活动帧，可在 host 构造 context 时设置。分发器不额外计数；拒绝进入的帧不增加计数，所有已进入帧在成功、显式失败、fuel 耗尽和子调用失败出口统一减回。直接和间接递归测试验证失败后深度归零。

生成代码的显式栈槽现按 word 累加到活动帧预算，CLI 接入 session 的
`stack_slots` 配额。`seal_frame_charge` 统计已生成的固定 stack slots，
入口准入、退出归还；`native_generated_stack_budget_counts_frames_and_unwinds_on_failure`
验证成功和预算失败后计数归零。这是显式槽位的逻辑预算，不包含机器码 prologue、
寄存器 spill、host helper 栈或操作系统完整栈占用，不能宣称它是精确物理栈上限。

Array spread 已移除按输入描述符和 slice word 数扣减 fuel 的预扫描。有限的数据
复制不构成额外语言调用；描述符验证、长度溢出检查及 backing 分配准入仍由
`array_concat` 执行。边界测试验证零剩余 fuel 下的内部复制成功，随后内存配额
不足时不写结果、不新增数组槽，并只报告一次带来源的终止失败。调用本身的
fuel 由生成代码的调用边界负责，不将内存消耗折算成 fuel。

集合回调的临时结果 Vec 在初始预留和 flat_map 扩容前检查分配预算；扩容按至少
所需容量及几何增长请求，并使用 try_reserve_exact 传播分配失败。flat_map 回调
结果展开不再按元素数 × word 宽度扣 fuel，仍先准入临时描述符存储再复制。
`flat-map-budget.telora` 让一次回调返回已有的万项数组：JIT 验证结果扩大不增加
逐元素 fuel，内存预算不足则中止且只报告一次、调用深度归零。实际回调仍消耗
调用 fuel。这里只统计请求的临时容量和描述符字节，不代表 allocator 实际容量、
所有回调缓冲或峰值 RSS 已完整计费。

String/Bytes 字面量创建、直接相等和 String.length 已移除按输入字节数扣 fuel
的逻辑；字符计数仍返回 Unicode 字符数量。测试覆盖 main 中的字符串/字节值在
零剩余 fuel 下完成内部扫描且不分配，以及字面量在内存配额不足时不创建 backing、
不写结果。实际语言函数调用仍通过生成代码检查，内部有限扫描不再按字节收费。

深层结构相等已移除逐任务和逐字节 fuel，String/Bytes、Dict 键和 Regex pattern
直接比较已有内容。Array/Record/Tuple/Dict 逐项展开，保留首个差异立即返回；
已访问的对象对保证共享/环遍历终止，不深复制用户对象。万项数组测试验证零剩余
fuel 下可比较相等与不等输入，已中止的 session 仍拒绝比较且不写结果。
CallContext 的只读 runtime 工作入口不再暴露成本扣减回调，只保留中止检查与
借用访问。这里没有宣称遍历临时集合已经完整纳入逻辑内存配额。

codec 编解码不再按递归节点、容器元素和 untagged 候选次数扣 fuel。有限遍历由
输入结构和 512 层深度限制约束，进入递归处理时仍检查 session 中止。数组和字典
编解码的临时结果 Vec 在预留前检查逻辑分配配额，并以 try_reserve_exact 处理失败。
实际 checker/property 调用仍走生成代码的调用预算；Failed 不转成语言层 Err。
万项数组编码与 untagged 解码回归在有限调用预算下成功，调用深度归零。这不代表
字段名、路径和所有临时管理数据已实现完整内存计量。

文本解析不再按输入字节或解析节点扣 fuel；保留输入范围验证、深度上限、session
中止检查以及实际 property/checker 调用。捕获字段结果 Vec 在预留前检查分配预算。
JIT 验证正常数字成功，万字节非法数字返回可捕获的 ParseError，且无 VM 失败诊断。
真实资源中止仍不能被当成解析拒绝恢复；不要求正则内部提供逐状态 fuel 回调。

首批验证：`cargo test -p telora-native`，2 项单测通过，覆盖 3/4/2-word 混合槽位、递归独立存储、来源、Never 拒绝、HeapRef 范围和 helper panic/失败传播。随后 `--features jit` 的 6 项测试验证了三指针 C ABI 的真实机器码参数/返回、来源保真和 Failed 不解码结果；尚需程序内部调用、间接调用和对象 helper 的整合证据，#180 暂不关闭。

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

### Fuel 的规范依据

本轮收敛已完成：生成函数／初始化器入口检查调用 fuel，helper 不再按节点、
字节、元素或别名链长扣费。`.cast!` 保留中止检查并采用 512 层独立嵌套上限，
超限产生不可捕获的带来源终止失败；实际 checker 调用仍由生成函数入口计数。
万项 Tuple 到 Newtype 数组转换回归验证有限转换不消耗逐元素 fuel。内存和深度
限制继续独立核对，不能把此次 fuel 收敛当作整体资源配额已全面验证。

遵循 [正式语言文档](../docs/design/LANGUAGE.md#102-fuel-和配额) 和 RFC 0010：
fuel 用来约束失控执行，不要求精确计费。上文按表达式、字节及遍历节点扣费的
描述记录已有实现与历史测试，不构成新增规范。后续应核对调用、实际回边和
callback 重入的检查点，并将与此无关的成本计数收敛；分配准入和来源保留不能
随计数简化而撤除。

正则引擎没有逐状态 fuel 回调，不单独构成本期阻塞项；保留成熟引擎，核对它的
终止性与输入/程序规模限制。无需仅为精确扣费实现新的正则解释器。引擎临时
内存和 cache 准入仍是独立的资源问题，不能用 fuel 定位澄清宣称其已经解决。

### 正则预算接口调查（2026-09-12）

已核对当前锁定依赖 `regex-automata 0.4.16` 的 `meta/regex.rs`：

- `Regex::search_with`、`search_captures_with`、`search_slots_with` 不接收工作量回调或取消标记。`search_captures_with` 返回 `()`，内部直接调用 `search_slots_with`。现有 helper 不能在一次搜索中途扣减 session fuel。
- `Regex::memory_usage` 是近似堆用量，文档明确没有限制此总量的高层配置。NFA、one-pass DFA、完整 DFA、hybrid cache 各有独立限制，不能把单独的 NFA 上限描述为整个 regex 编译/执行峰值上限。
- 当前 native 的 `regex_matches` 和 `regex_captures` 在搜索后核对 cache 增量；这能统计逻辑分配，但不能保证增长先经过配额准入。
- `nfa::thompson::PikeVM::get_nfa` 可以访问 NFA，`NFA::states` 和 `group_info` 提供状态/捕获信息。它提供进一步研究保守工作量准入或插入计数点的基础，但当前仍未接入 native。

以下 PikeVM 实验保留为引擎语义与分配观察证据，不作为切换引擎或精确计费的
前置任务。生产保留现有引擎，不设置未经验证的复杂度乘数。

独立实验已加入 `crates/telora-native/tests/regex-engine.rs`，运行 `cargo test -p telora-native --test regex-engine -- --nocapture`。14 个 pattern × 19 个输入 × 4 个搜索范围共 1,064 组对照通过，覆盖 ParseBy 形式、可选/嵌套捕获、Unicode、词边界、非贪婪、空输入/空范围和 UTF-8 内部边界；比较完整 match 与所有捕获槽，earliest 模式只比较是否命中。

缓存 API 报告值（字节；初始化/短输入/65,536 字节输入；不等于保留容量）：

| Pattern | NFA 状态 | 捕获槽 | Cache |
|---|---:|---:|---|
| `(?P<word>\w+)` | 325 | 4 | 26064 / 26064 / 26064 |
| `(?:a?){32}a{32}` | 101 | 2 | 4880 / 4880 / 4880 |
| `(?P<a>a*)(?P<b>b*)` | 13 | 6 | 1552 / 1552 / 1552 |

源码复核发现 `PikeVM::Cache::memory_usage()` 使用 epsilon 栈的 `len()`，而非 `capacity()`。搜索结束后栈清空，因此上表数值不变不能证明搜索期间没有分配，也不能作为缓存完整计费的依据。

独立进程实验 `cargo test -p telora-native --test regex-allocations -- --nocapture` 用包装 System 的 allocator 观测请求字节数。pattern、捕获结果存储和 65,536 字节输入在观测窗口外构建；窗口包含 cache 构建、搜索和销毁：

| Pattern | 初始请求字节 | 搜索后保留 | 请求存活量高水位 | API 报告 | 销毁后 |
|---|---:|---:|---:|---:|---:|
| `(?P<word>\w+)` | 26064 | 26128 | 26128 | 26064 | 0 |
| `(?:a?){32}a{32}` | 4880 | 4944 | 4944 | 4880 | 0 |
| `(?P<a>a*)(?P<b>b*)` | 1552 | 1680 | 1680 | 1552 | 0 |

这里的高水位只记录请求存活量，realloc 按净增量更新，不包含分配器开销、复制时的瞬时双份内存或 RSS。三个样本分别暴露了 API 未计入的 64/64/128 字节保留容量；这些样本不构成通用内存/工作量上界证明。继续研究准入必须覆盖 epsilon 栈容量，不能只按该 API 报告值计费；也不能用表面 pattern 长度代替 Unicode 展开后的程序规模。生产仍使用 meta 引擎。

已将生产捕获结果存储的配额检查移到 `create_captures()` 之前：依据 `group_info().slot_len()` 计算 `Option<NonMaxUsize>` 数组的请求大小，与当前依赖 `Captures::all` 的构造一致。三个 native string-parse 回归通过，覆盖命名捕获、嵌套解析、来源/字符串复用及 checker 失败。这只修正捕获数组的准入顺序，不涵盖搜索缓存和引擎编译的临时分配。

实验还修复了独立构建缺口：runtime helper 不依赖 Cranelift，因此不再被 `jit` feature 隐藏；不开启该 feature 时 codec 也能正常编译。无 jit 的 22 项 runtime/ABI 测试及两项引擎对照/缓存观察实验通过。

用简单 Rust ABI 单测验证混合宽度参数/返回、递归帧互不覆盖、错误不读取未初始化结果、来源完整保留。记录首个支持的 target 和 word/endian 约束；不宣称 ABI 跨 target 稳定。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
