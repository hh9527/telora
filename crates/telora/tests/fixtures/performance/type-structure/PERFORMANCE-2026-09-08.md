# RFC 0275 性能审视

本报告测量构造校验实现 `e60feed` 的前端与运行时成本。功能提交已先推送，
性能审视期间没有修改编译器或运行时；基准与报告单独提交。

确认了两个独立问题：标准库安装和工具阶段重复分析带来了较大的固定前端成本；
泛型构造在循环中重复实例化相同类型的元数据，增加了运行时间和内存。

## 环境与方法

- x86_64，8 个逻辑 CPU，虚拟机报告的型号为 General Purpose Processor。
- Rust `1.98.1 (48a229cea 2026-09-01)`，统一使用 dev profile，未优化、带调试信息。
- 所有版本独立 checkout，以同一工具链构建并保存独立二进制；未构建 release binary。
- 每项预热一次，串行测量三次并取中位数，不包含构建时间。
- `check` 测量前端和工作区恢复；`eval` 明确执行循环，并核对计算结果。
- 通过 2,000 与 20,000 次循环的差值估计每次迭代的成本，减少固定启动成本的影响。
- Callgrind 3.24.0 记录指令数 Ir。剖析运行的耗时不混入普通计时。
- 数值是同一台机器上的 debug 对照，不能当作优化构建的绝对性能指标。

原始中位数、版本和指令计数见 [results-2026-09-08.json](results-2026-09-08.json)。
基准命令与负载说明见 [README.md](README.md)。

比较历史版本时为每个 checkout 使用独立的 `CARGO_TARGET_DIR`，或者在构建后立即
保存独立的二进制副本。共享 target 目录的可执行文件名称可能被其他 checkout 覆盖。
本次使用独立保存的二进制进行全部测量，并已恢复工作区的当前 debug binary。

有 Valgrind 的环境可用以下命令复现固定成本剖析；将末尾的 `check @src/startup`
替换为 `eval @src/runtime-generic:result` 或 QueryBuilder 的无输出查询即可剖析其他负载。

```sh
valgrind --tool=callgrind --callgrind-out-file=/tmp/telora-startup.callgrind \
  target/debug/telora -C crates/telora/tests/fixtures/performance/type-structure \
  check @src/startup
callgrind_annotate --inclusive=yes --auto=no /tmp/telora-startup.callgrind
```

## 历史前端对照

版本定义：`83bb8a6` 为 RFC 0274 之前；`58f0b8c` 为本次构造校验之前的 main；
`9f16a6b` 为补齐泛型函数体校验之前；`e60feed` 为本次功能完成提交。
下表单位为秒。

| 负载 | 83bb8a6 | main 58f0b8c | 9f16a6b | e60feed |
| --- | ---: | ---: | ---: | ---: |
| 仅导出一个 Int | 0.320 | 0.591 | 0.593 | 0.592 |
| 100 个简单函数契约 | 0.452 | 0.826 | 0.819 | 0.823 |
| 100 个嵌套结构函数契约 | 0.987 | 1.626 | 1.642 | 1.588 |

最小模块相对 RFC 0274 之前约慢 85%。这项主要固定退化在当前 main 上已经存在，
本次构造校验分支没有显著放大它。这里比较的是整个阶段，并非对该阶段逐提交二分。

更复杂的前端负载也可复现：

| 负载 | main 58f0b8c | e60feed |
| --- | ---: | ---: |
| 100 个递归类型函数契约 | 1.064 | 1.046 |
| 递归浅层值 | 0.650 | 0.646 |
| 递归共享增长值 | 0.649 | 0.646 |
| QueryBuilder check | 2.610 | 2.519 |
| QueryBuilder query，无匹配输出 | 2.616 | 2.530 |

这组递归共享值没有出现随共享图展开的爆炸式增长。QueryBuilder 的无输出查询和
check 耗时接近，说明主要开销在语义处理，不能归因于结果渲染。

## 运行时对照

下表均通过 `eval` 执行 2,000 次迭代，包含各自的加载成本，单位为秒。

| 负载 | main 58f0b8c | 9f16a6b | e60feed |
| --- | ---: | ---: | ---: |
| 整数循环 | 0.610 | 0.610 | 0.604 |
| 普通 struct 构造 | 0.619 | 0.609 | 0.617 |
| 泛型 struct 构造 | 0.621 | 0.620 | 0.661 |
| JSON 解码 | 0.771 | 0.792 | 0.804 |
| 带校验的 struct 构造 | 不支持 | 0.666 | 0.656 |

泛型构造的退化被固定成本部分掩盖。扩大迭代次数后，结果如下：

| 负载 | main 2,000 次 | main 20,000 次 | 当前 2,000 次 | 当前 20,000 次 |
| --- | ---: | ---: | ---: | ---: |
| 普通 struct | 0.616 | 0.738 | 0.621 | 0.740 |
| 泛型 struct | 0.620 | 0.788 | 0.661 | 1.213 |

泛型循环增加的 18,000 次迭代，main 用时约 0.168 秒，当前约 0.552 秒，
对应约 9.3 与 30.7 微秒/次，差值估计约慢 **3.3 倍**。普通 struct 的差值基本不变。

同一泛型长循环的单次峰值 RSS 观测为 main 36,788 KiB、当前 54,744 KiB，
增加约 17.5 MiB。RSS 来自 `os.wait4` 的 `ru_maxrss`，是单次观测，不是中位数。

## 剖析与代码原因

### 固定的标准库安装成本

最小模块在 `83bb8a6` 执行约 26.47 亿条指令，当前约 47.73 亿条。
当前 `install_native_modules_observed` 包含约 98.4% 的总指令成本。

[模块安装代码](../../../../../telora-core/src/module/graph.rs) 遍历整个 `module_specs()`，
逐个解析、分析和初始化内置模块。即使用户模块只导出一个整数，也会支付这项成本。
[Engine 入口](../../../../../telora-core/src/module/engine.rs) 的加载与恢复路径会安装内置模块。

### 工具表达式重复推断与元数据解码

当前最小模块的主要调用路径如下。数字为单个调用上下文的 inclusive 成本，
包含其子调用，**不可相加**。

| 路径 | 占总指令成本 |
| --- | ---: |
| analyze_program_with_bindings_observed | 73.4% |
| evaluate_tool_expression_with_debug | 41.2% |
| infer_tool_expression_evidence | 31.1% |
| TypeGraph::decode_persistent 的递归调用上下文 | 25.4% |
| parse_registered | 22.1% |
| DocumentText::slice | 19.9% |

[infer_tool_expression_evidence](../../../../../telora-core/src/types/metadata.rs)
为每个工具表达式收集注解、克隆环境与 scheme、遍历 bindings 解码类型，
并创建新的 GenericInference。随后还会编译和求值工具表达式。
原先约 3.50 亿条指令的工具表达式求值路径，当前约为 19.68 亿条。
引入该重复推断路径的历史提交是 `4e7c5ab`；测量支持其为主要热点，
但不将整个 RFC 0274 阶段的退化全部归于单个提交。

解析也有独立的固定成本：[Lowerer::text](../../../../../telora-core/src/parser/helpers.rs)
对各 CST 节点调用 [DocumentText::slice](../../../../../telora-core/src/document.rs)，
反复执行 Rope 范围切片。旧版本解析约 10.33 亿条指令，当前约 10.53 亿条，
因此它是持续存在的热点，不是本次主要新增退化。

QueryBuilder 的无输出查询约执行 243.24 亿条指令，工具表达式推断路径占 49.6%，
GenericInference 的创建路径占 20.1%，标准库安装占 19.3%，文本切片占 16.2%。
这些仍是互相包含的调用成本。相比最小模块，用户模块中的重复分析成本进一步放大。

[GenericInference::new](../../../../../telora-core/src/types/inference-context.rs)
会遍历 schemes、局部注解、具名类型和外部接口，重新收集名义类型 body。
[collect_declared_bodies](../../../../../telora-core/src/types/prelude.rs) 的 visiting
集合只防止当前递归路径的环；返回时移除标记，并且不会因 body 已收集而停止遍历。
因此多个契约引用同一名义类型时，仍会重复走它的结构。应区分正在访问和已经完成，
并保留不完整名义引用稍后由完整 body 补充的能力。

### 泛型 owner 元数据重复实例化

2,000 次泛型构造的整个进程约执行 54.03 亿条指令。
`native_apply_type_family` 调用 2,012 次，而最小模块只调用 11 次。
该函数包含约 3.58 亿条指令，主要进入 `instantiate_type_family`、
`bound_type_replacements` 和 `PendingCopy::copy_value`。

[compile_owner_evidence](../../../../../telora-core/src/compiler/expression.rs)
在含类型参数的构造位置发出调用，每次执行构造都会重新求得 owner。
[类型实例化](../../../../../telora-core/src/heap/type-family.rs) 为相同的 `Box(Int)`
重复建立替换表并复制元数据图。TypeId 的 canonical identity 不等于元数据实例化结果的缓存。
这解释了长循环新增的时间与内存。

## 建议的优化顺序

1. 缓存或外提编译器生成的泛型 owner 实例化。以当前 world 中的模板和类型实参为键，
   复用已封闭的元数据；每个新值仍须执行其构造校验。这一项直接处理本次约 3.3 倍的
   泛型循环退化，应优先完成，并以长循环、来源追踪和校验次数用例验收。
2. 复用工具阶段的类型证据与推断上下文，缓存已封闭元数据的解码结果，避免对每个
   类型注解完整遍历 bindings，并避免重复收集已有名义类型 body。缓存必须考虑词法作用域与依赖状态，保留工具阶段的
   诊断和求值语义，不能跳过必要的构造校验。
3. 减少内置模块的重复安装成本。先复用不可变解析结果，再评估按依赖安装或复用
   已验证的内置模块表示。跨 world 复用必须处理 ModuleId、TypeId、FuncId、
   源码位置和配额，不能直接共享含当前 world 句柄的可变堆。
4. 为一次解析使用连续文本快照或高效 token 文本视图，减少 Lowerer 的 Rope 切片。
   文档编辑仍可保留 Rope，并以 UTF-8 位置、增量解析和诊断位置测试验收。

这些优化尚未实施。本报告提供已确认的热点、历史对照和可重复的验收负载，
后续每项优化应分别提交并复测，避免用减少正确性检查换取性能。
