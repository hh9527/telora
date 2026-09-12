# RFC 0283：Native 值、调用帧与运行时 ABI

- 状态：实施中；独立 ABI 数据/帧模块已落地，真实机器码调用验证继续推进
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

### 首批确定的 ABI v1 契约

独立 `telora-native` crate 消费公共 SealedMir 和候选布局，不依赖旧 VM/Val/Heap 接口。当前支持 64-bit little-endian host；来源三元组为 SourceId/start/end，SourceId=0 且 offsets=0 表示无来源，不截断 SourceId。TypeKey 直接保存 seal 后的 TypeId 数字，不重新编号。

HeapRef 的高位选择 work（1）或 main（0），低 31 位为分类表槽位，零号槽位有效。word 内仍用 u32 保存引用，Dict 两列引用均采用该规则。world 世代由执行 context 管理；句柄不得脱离所属 session 或跨回收保留，后续回收必须重写根。

参数/局部槽位按完整值 word 宽度顺序排列，全部 8 字节对齐；Never 无槽位。Activation 首版使用每调用独占的固定 Box 缓冲区，递归不因共享 Vec 扩容导致指针失效；生成代码可使用等价的原生栈帧。公共检查入口拒绝未写入槽位和异类型写入。

status 使用 u32：0=Success，1=Failed；只有 Success 允许读取结果缓冲区。原始 ABI 指针的长度由已闭合函数签名决定，不得缓存调用期参数/结果指针。host helper 的 Rust panic 在边界捕获并转为 Failed（abort 型 panic 不可恢复）；原始失败记录一次，传播 Failed 不重复诊断。完整 FFI 调用验证由首个 JIT 模块继续落实，不以数据结构单测代替。

会话 fuel 存于 CallContext，初始化、按需回调、发布和 entry 调用共享同一计数。当前生成代码在每个实际执行的 HIR 表达式入口扣减一个单位，未走到的分支不计数；这不是机器指令计数，也不承诺与旧字节码的 fuel 数字相同。耗尽后 context 保持 Failed，仅首次记录来源诊断，不读取输出缓冲区。CLI 采用会话配置的 fuel 上限。原生 helper 内部工作量、调用栈和分配配额仍待进一步接入，不以表达式计数宣称配额验收完成。

调用深度现由同一 CallContext 维护：默认上限为 128 个生成函数／property 初始化器的活动帧，可在 host 构造 context 时设置。分发器不额外计数；拒绝进入的帧不增加计数，所有已进入帧在成功、显式失败、fuel 耗尽和子调用失败出口统一减回。直接和间接递归测试验证失败后深度归零。该上限仅限制递归深度，尚不等于精确栈槽预算；大帧尺寸及完整 stack_slots 配额仍需后续实现和验收。

Array spread 的 helper 工作量已接入同一 fuel：先按输入描述符数扣费，再按实际 slice 的元素数 × 元素 word 宽度扣费，全部通过后才分配并复制结果 backing。计费不包含未选择的 backing 区间。边界测试验证精确预算成功、少一单位拒绝、结果缓冲不写入、不新增数组槽、单次来源诊断及 sticky abort。该项不代表所有 helper 都已计费；深层相等、codec、字符串/正则等工作量仍须分别核对。

String/Bytes 字面量创建、直接 String/Bytes 相等和 String.length 已预扣输入字节数的 fuel。字符串描述符的范围长度在 UTF-8 扫描前取得；比较按两侧输入字节总数保守计费，字符计数仍返回 Unicode 字符数量。测试覆盖已发布的大字符串、字符数与字节数不同、精确预算成功、少一单位失败、未写结果缓冲和字面量拒绝时不创建 backing。聚合深层相等及其他字符串 API 的计费仍需继续覆盖，不据此宣称全部字符串工作量已受限。

深层结构相等已接入 session fuel：按遍历任务扣费，String/Bytes 叶子、Dict 键和 Regex pattern 在内容比较前按字节扣费。Array/Record/Tuple/Dict 逐项展开，工作列表不再一次加入整层子项，保留遇到首个差异立即结束的行为。已访问的对象对仍用于共享/环终止；没有对用户对象做深复制。万项数组测试验证小预算下的首项短路，以及相同输入继续遍历时耗尽、未写结果、来源和单次诊断。host helper 使用 CallContext 的分字段借用共享原 fuel，不新增一套预算或临时取走 Runtime。codec 与其余原生 API 的计费仍待覆盖。

codec 编码/解码及文本解析的递归节点已扣减同一 fuel，容器展开在预留条目列表前扣费，untagged 的每次候选尝试也扣费。耗尽返回 DecodeFailure::Failed，不能被当作 Rejected/Blame 收集后继续候选尝试或包装成语言 Err。真实 JIT 万项数组测试验证编码耗尽与 untagged 解码耗尽均只报告一次，即使源码捕获解码 Err 也不能恢复，调用深度归零。该阶段覆盖图遍历，不代表 regex 引擎、字段名处理及所有文本扫描已完成工作量计费。

文本解析入口和 codec 的文本桥接现在从描述符取得范围长度，避免在计费前扫描 UTF-8。每次解析节点扫描输入前按输入视图字节数扣费；读取 property 后需要重新借用文本的路径再次扣费。预算不足时直接执行失败，不能返回可捕获的 ParseError。JIT 用例验证小数值成功、万字节非法数字在扫描前耗尽、原输入来源和调用深度归零。该字节计费不是 regex 自动机状态转换次数的计量，正则引擎工作量仍须单独验证。

首批验证：`cargo test -p telora-native`，2 项单测通过，覆盖 3/4/2-word 混合槽位、递归独立存储、来源、Never 拒绝、HeapRef 范围和 helper panic/失败传播。随后 `--features jit` 的 6 项测试验证了三指针 C ABI 的真实机器码参数/返回、来源保真和 Failed 不解码结果；尚需程序内部调用、间接调用和对象 helper 的整合证据，#180 暂不关闭。

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

### 正则预算接口调查（2026-09-12）

已核对当前锁定依赖 `regex-automata 0.4.16` 的 `meta/regex.rs`：

- `Regex::search_with`、`search_captures_with`、`search_slots_with` 不接收工作量回调或取消标记。`search_captures_with` 返回 `()`，内部直接调用 `search_slots_with`。现有 helper 不能在一次搜索中途扣减 session fuel。
- `Regex::memory_usage` 是近似堆用量，文档明确没有限制此总量的高层配置。NFA、one-pass DFA、完整 DFA、hybrid cache 各有独立限制，不能把单独的 NFA 上限描述为整个 regex 编译/执行峰值上限。
- 当前 native 的 `regex_matches` 和 `regex_captures` 在搜索后核对 cache 增量；这能统计逻辑分配，但不能保证增长先经过配额准入。
- `nfa::thompson::PikeVM::get_nfa` 可以访问 NFA，`NFA::states` 和 `group_info` 提供状态/捕获信息。它提供进一步研究保守工作量准入或插入计数点的基础，但当前仍未接入 native。

下一步先做独立 PikeVM 适配实验：对现有 regex/ParseBy 语言资产验证命中范围、Unicode、捕获和缺省/必需字段语义，量化状态数、捕获槽数与输入大小的关系，再决定能否建立明确的保守准入模型。若需要精确的执行中断，则需给引擎增加计数/取消接口，不能以输入字节数或编译后内存乘积冒充实际状态转换次数。

在实验有证据前保留现有引擎，不切换运行语义、不设置未经验证的复杂度乘数、不关闭正则执行预算验收项。当前文本扫描计费与正则引擎内部计费是两项独立证据。

用简单 Rust ABI 单测验证混合宽度参数/返回、递归帧互不覆盖、错误不读取未初始化结果、来源完整保留。记录首个支持的 target 和 word/endian 约束；不宣称 ABI 跨 target 稳定。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
