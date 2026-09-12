# RFC 0284：Native 分类对象表与 world runtime

- 状态：实施中；基础分类表和初始化发布已实现，剩余对象类别及 JIT helper 继续推进
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

已支持 String（inline/heaped）、Tuple/Record 共表、Array/slice、有序双列 Dict。`publish` 对整个根集合使用一次转发表复制，保留共享和来源；临时 main 全部构建成功后才替换状态，清空初始化 work 并更新 session 身份。发布失败不改变旧 world，重复发布拒绝。新 work 可引用已发布 main 对象，但不能写入 main 表。

验证：`cargo test -p telora-native --features jit` 通过 9 项测试，其中 3 项 runtime 测试覆盖分类表共享、持久更新、Dict/Array 嵌套别名发布、来源保留、初始化句柄失效和失败原子性。测试源码放在 `tests/fixtures/runtime.telora`。enum/闭包/dyn/native resource、真实环以及机器码 helper 尚待覆盖；不据此关闭 #181。

后续已支持 enum 的 nullary/full_value/ValueTable 间接 payload 及发布遍历。以已 seal 的 variant 表校验 tag、payload TypeId 和宽度，JIT 按已选择的 variant 构造，不按名字猜测类型。递归类型的有限嵌套及间接 payload 发布已有 .telora 资产验证，当前 native 总计 15 项测试；真实对象环、闭包和 dyn 等仍未据此宣称完成。

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

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
