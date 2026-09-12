# RFC 0284：Native 分类对象表与 world runtime

- 状态：实施中；分类表、循环闭包与全图发布已接通，资源计费完整性仍待验收
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#181](https://github.com/hh9527/telora/issues/181)

## 动机与范围

实现不依赖旧 VM/Val/Heap 的独立对象运行时。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 从实验布局实现提取或迁移独立生产模块，消费已闭合类型描述；旧运行时可参考但不能成为新运行时依赖。
- 分类表采用 Vec<Item>，Item 拥有缓冲区；Tuple/Record 共表，Dict 使用 ArrayTable 的有序 keys/values 两列和二分查找。
- 逐类实现 String、Array/slice、聚合、enum、闭包、dyn 和必要 native resource，全部引用可由 TypeId 精确遍历；不保留未知类型兜底。
- main-world 固化后只读；work 可引用 main，main 不得引用 work；初始化发布的复制需保存共享和来源。
- helper 使用 RFC 0283 ABI，不借用旧 Val 转换桥；明确 borrowed view 在分配和回收期间的有效期。

## 实施计划

### 基础表与发布进展

独立 `telora-native::runtime` 复用 RFC 0281 实验的存储设计，不导入旧 VM/Val/Heap。与 native ABI 共用完整 Value 描述，描述外携带 session 身份以拒绝跨 session 和过期初始化引用。分配发生在 work，读取根据 HeapRef 的 world 位选择 main/work。

当前 Tables 包含独立 String/Bytes 表、Tuple/Record 共表、Newtype 表、Array 表、Value 表、Environment 表、Format 表，以及 Regex/Hash/Blame/Test 槽位。Dict 使用有序双列 Array，enum 按布局使用内联或 Value 表 payload，Dyn 使用 Value 表保存擦除后的值。Environment 同时承载闭包捕获及稳定词法函数槽；这些结构均不依赖旧对象运行时。

`publish` 对整个根集合使用一次转发表复制，保留共享和来源；临时 main 全部构建成功后才替换状态，清空初始化 work 并更新 session 身份。发布失败不改变旧 world，重复发布拒绝。新 work 可引用已发布 main 对象，但不能写入 main 表。词法槽和函数体互相捕获形成的真实环已经由运行时测试及 eval-with 语言资产验证，发布后仍保留别名和可调用性；Pending 函数槽不能发布。

enum 的 nullary/full_value/ValueTable 间接 payload 以已 seal 的 variant 表校验 tag、payload TypeId 和宽度，JIT 按已选择的 variant 构造。递归类型的有限嵌套、间接 payload、泛型互递归闭包及发布后的动态调用已有语言资产；具体 codegen 与函数槽证据见 RFC0285。

### 当前验证记录（基于 e4dda0a）

- `cargo test -p telora-native`：24 项 runtime/ABI 单测及 3 项独立 regex 实验通过。不启用 jit 仍可独立编译运行，helper 不依赖 Cranelift。
- `cargo test -p telora-native --features jit`：最近完整运行 128 项单测及 3 项 regex 实验通过，覆盖真实机器码调用和运行时对象操作。
- `runtime/tests.rs` 包含来源、main/work 共享、失效句柄、失败原子性、函数槽真实环、实例适配器身份、数据物化和分配/fuel 边界测试。语言资产包含普通/泛型互递归及 eval-with 发布后的调用；这些是具体验证范围，不是对所有输入的证明。
- 生产 native 源码的旧 Vm/Val/Heap、resolve/type-resolve/codegen 调用依赖检索未发现旧后端接入；测试构造 MIR 的依赖另列，不作为生产执行路径。检索不能代替全接口依赖审计。

剩余限制主要是正则执行中预算、引擎编译/缓存临时内存、尚未逐项计费的 helper scratch/扫描，以及物理机器栈与 RSS 和逻辑配额的区别。下方条目说明已实施的计费范围；#181 仍未据此关闭。

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

数组 backing 的分配准入已提前到内容构造之前：普通 array、spread、push、
enumerate、concat 和 zip 均先确定长度及封闭元素宽度，检查配额后直接填充最终
word 缓冲。取消构造 helper 的整批临时 Value 列表，spread 不再保留区间列表；
空 Never 数组不请求 Never 的值布局。新增拒绝测试验证配额不足时 enumerate
连 tuple 元素也不构造；139 项 native 测试及 3 项独立实验通过。
这只覆盖上述数组路径，其他 helper 的临时分配仍需审计。

字符串 join/join_lines、replace、indent、ensure_trailing_newline、trim_margin
已按确切输出长度先准入，再构造并移交 StringTable，避免扩张完成后才检查配额。
split/lines 先统计项数并准入数组 backing，再写共享切片描述符，不保留区间与
Value 的整批临时列表。Unicode 空分隔符、空 needle、非重叠替换、CRLF 和空 join
已有语言资产覆盖；已有 JIT 超限用例验证耗尽无法被诊断捕获函数吞掉。
这仍不代表 regex 引擎临时内存、所有 host scratch 或 RSS 的完整计量。

Regex 使用 regex-automata 的显式编译程序与匹配缓存。编译时按引擎报告的近似 heap 大小计费，缓存初始大小与后续净增长计费，捕获槽临时数组按长度计费；原始 pattern、捕获名及必选捕获名按内容与描述符计费。发布共享不可变编译程序，只计复制的槽、名称和匹配缓存，共享根仍只转发一次。NFA 编译上限取默认 10 MiB 与 session 剩余预算中的较小值，因 session 限额编译失败标记为不可捕获资源耗尽。108 项 native 测试覆盖既有匹配/捕获语义、构造与发布超限、共享根及极小编译预算。该计量仍是近似逻辑量：解析/编译临时峰值、BTreeSet 节点开销、引擎内部管理数据和缓存重分配瞬时峰值未被完整覆盖，不能当作 RSS 上限。

HashState、Blame、Test 独立槽位也已纳入累计分配预算：HashState 计固定上下文大小，Blame 计槽项、消息描述符及来源列表，Test 计槽项、参数槽数组及参数描述符。引用到的 backing 对象由其自身表单独计费；发布只对首次转发的资源计费，构造与发布采用相同规则。测试覆盖构造拒绝、发布中途拒绝及共享根只复制一次。Regex 编译程序/捕获名称、helper 临时缓冲及其他未计量项仍继续推进。

Native allocation_bytes 开始在 Runtime 内累计，跨初始化、发布与 entry 不重置。基础 word 表按 payload word 字节数加槽项大小计费，String/Bytes backing 按长度加槽项大小计费；inline String 不分配 backing。发布沿转发表对每个实际复制的 backing/word 对象计费一次，超限保留旧 world 和来源，后续 helper 将其传播为不可捕获 abort。计量是逻辑累计请求量，不是 RSS，也不包含 Vec 预留容量、描述符临时副本、helper 临时缓冲、JIT 代码或独立资源表；这些遗漏仍需继续补齐，不能据此宣称完整分配防护。

Native 栈预算开始消费 CLI session_quota.stack_slots，以显式临时槽的 u64 word 为单位。函数生成结束后把全部显式槽宽度写入入口 admission 常量，进入时累加、所有返回/失败路径归还；超限为不可捕获 abort。预算不按运行时类型猜测。递归正常/超限/零预算用例验证余额和调用深度均归零。当前是逻辑显式槽预算，不包含 Cranelift spill、机器帧开销或 Rust helper 临时空间，不能宣称完整物理栈防护；allocation_bytes 及 helper 工作量计费继续推进。

std/test 的 Test 描述按 native type (33, 0) 存入独立 Vec 槽位，保存操作种类及闭包/期望/fixture 清单的原生描述符，不复制字符串或捕获对象。构造只验证参数（包含非空错误期望和 fixture 的权威 Value 回调签名），不执行测试或加载 fixture。发布遍历这些参数并保持 Test 身份共享；Test 相等采用对象身份。此处支撑 check/eval/eval-with 对含测试定义模块的初始化，不增加 native test 调度命令。

诊断捕获的上下文边界已区分普通 Failed 与不可恢复 abort。fuel 耗尽、调用深度超限、helper panic 和字符串解析资源限制设置 session 中止标记；后续调用/helper 不再执行，但栈退出 guard 仍能清理。普通范围可移出其新增报告，嵌套范围不吞掉外层报告；abort 保留全部报告给 session 最终输出。

call_with_diagnostics 已通过封闭回调签名分派，按实参中的权威 TypeOf 见证核对 Diagnostic/Severity/Label/SourceRange。报告直接构造到 native tables，来源名称来自静态源码清单及后续登记的数据来源；只复制诊断文本与位置，不复制 subject 对象。成功返回 Ok((value, reports))，普通失败返回 Err(reports)，Never 失败路径不读取结果槽。已覆盖嵌套范围、失败后继续执行、发布后的诊断数据读取、来源范围以及不能捕获 fuel/调用深度限制。

HashState 按权威 native type (16, 3) 存入独立 Vec<Context> 槽位，描述符保存 HeapRef。Context 为独立纯 SHA-256 状态算法，保存摘要字、缓冲区及长度；保持现有逐字段相等契约，不用最终 digest 判断状态相等。旧运行时实现不变，native 不依赖旧 VM/Heap。更新读取原状态并复制固定大小摘要上下文，直接借用 native String/Bytes；不修改旧状态，不复制输入对象树。发布按原 HeapId 转发，保留重复引用；main 状态可在新 work 中分叉更新。增量协议保留版本前缀、输入类型标记、变长输入的大端长度及 Int 大端编码。固定摘要向量、标准 SHA-256 分块/填充边界与真实 CLI 初始化/entry 分叉更新及相等比较已有验证；sha2 仅为测试参照。

单测构造/访问/更新、空容器、异宽与递归对象、浅层共享、Dict 排序与重复键、错误来源、跨 world 引用约束。每个新增类别用少量有意义的单测验收，不接入旧 VM。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
