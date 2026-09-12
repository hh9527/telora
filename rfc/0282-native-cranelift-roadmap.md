# RFC 0282：Cranelift Native 路线图（伞 RFC）

- 状态：实施中；check/eval/eval-with 已纵向接通，语义与资源预算验收继续推进
- 日期：2026-09-12
- 跟踪：[#179](https://github.com/hh9527/telora/issues/179)
- 开发分支：`feat/native-cranelift`
- 基点：`464f5f7`（继承 RFC 0281 的候选布局和隔离对象表实验）
- 前置：[RFC 0280](0280-demand-driven-inference-materialization.md)、[RFC 0281](0281-uniform-runtime-value-layout.md)

## 动机与范围

利用 SealedMir 已闭合的符号、类型、泛型参数及布局信息，直接通过 Cranelift 生成本机机器码。继续采用 Rust 分类对象表和 main-world/work-world，避免同时建设新解释器或为 Wasm 重构全部内存边界。

当前主线与旧运行时保持默认可用。独立分支逐步增加 native 模块，CLI 最终以隐藏 `--native` 显式选择；仅增加必要的边界接入，不借此重构旧 VM。新代码可以参考旧实现，但不得依赖将被替代的旧 VM、Val、Heap 或通过它们求值、补类型和回退。前端、SealedMir、来源和诊断等稳定公共数据结构可以复用。

本 RFC 组织路线与验收，不把未实现功能描述成已经完成，也不预先承诺性能收益。

## 确定的设计方向

1. 源码/模块图 → resolve → 类型闭合 → SealedMir，完全不执行用户代码。
2. SealedMir → 布局/ABI → Cranelift codegen；运行时不再猜测类型。
3. 在整图 Initialize WorkWorld 注入数据、完成 property 和顶层求值，成功后统一发布只读 main-world。
4. 首期仅推进到 eval-with，单次执行结束整体释放。run/serve、跨请求根和复制回收留待下一期。

物化值统一保留来源和 TypeId 头部，静态确定 data 宽度。连续调用帧与 Cranelift SSA 不矛盾；不先建设 stack-based 字节码。分类表采用拥有缓冲区的 Item，Tuple/Record 共表，Dict 为有序双数组。main/work 引用编码、精确调用 ABI、native resource 生命周期须在对应子 RFC 明确后实施。

## 子 RFC 与依赖

### 全图封闭与静态模板边界（实施调整）

执行入口的所有可达值及具体函数实例必须在 MIR 封闭前确定类型；codegen 不发现
新实例、不补类型。泛型声明可以按绑定参数检查契约，但函数族不是运行时值。
未被执行入口依赖的导出可以裁剪；check 则按所选模块检查声明契约，普通 check
的初始化范围仍须明确建立，不能误用单一 entry 的可达集合。

当前实现尚未完整满足此边界：通用 seal 不接收执行根；native compile_modules
仍服务于模块检查并按模块收集初始化项。入口编译另行消费静态依赖闭包，但还未
统一为在类型求解阶段封闭的
入口可达图。后续应在静态阶段建立执行根及其依赖闭包（包含 property、顶层值、
闭包捕获和具体实例），验证其中值类型完全具体化后交给后端。声明契约检查与
可执行图封闭必须区分，不能把含绑定参数的模板签名直接当成可执行签名。

已移除把未解出的函数值重新泛化的兜底；ExecutionGraph 不再为模板建立全局任务，
默认 codegen 直接读取具体实例任务，不再生成函数族物化或运行时特化指令。
旧 VM 的相关指令、对象和复制分支，以及不再被消费的 MIR 函数族表也已删除。接着让
两个后端消费静态阶段确定的实例和执行依赖。不能用运行时函数族身份比较或
后端自行特化来掩盖缺失的类型证据。

Native codegen 前已接入 `validate_execution_roots`：检查显式入口、初始化项、
property 和校验器根，并沿已求解的普通引用、泛型实例和 trait 实现边核验类型及
字段布局。该检查不求值、不选择新实例，直接拒绝以模板作为执行根。闭包节点
按稳定 HIR/实例 Id 排序，native eval/eval-with 已用它保留所选导出的顶层值和
具体实例依赖，裁剪无关导出；check 保留模块初始化范围。property 和校验器仍
作为整图元数据初始化根，保持当前工具阶段语义。无关导出中的运行时警告、
debug 或失败不会因执行另一个入口而发生。目前闭包仍位于通用 seal 之后；尚未
把声明检查和 entry 可执行图封闭统一为独立的发布类型。

| RFC | 模块 | 前置 |
|---|---|---|
| 0283 | [Native 值、调用帧与运行时 ABI](0283-native-abi.md) | 起点 |
| 0284 | [Native 分类对象表与 world runtime](0284-native-runtime.md) | 0283 |
| 0285 | [SealedMir 到 Cranelift 的机械 codegen](0285-cranelift-codegen.md) | 0283；对象部分依赖 0284 |
| 0286 | [Native 整图初始化、property 与发布](0286-native-initialization.md) | 0284、0285 |
| 0287 | [隐藏 --native 接入与执行语义对齐](0287-native-cli.md) | 最小入口依赖 0285；完整流程依赖 0286 |
| 0288（延期） | [服务边界 work-world 复制回收](0288-native-work-collection.md) | 后续 run/serve 阶段，不在本期 |

这些是模块边界，不是要求旧系统每个内部步骤都改造成功的流水账。0284/0285 可以在 ABI 明确后分别推进；0287 的最小入口可以随 codegen 原型提前打通，不必等待服务回收。具体 crate/目录划分由模块依赖决定，不要求每个 RFC 都新建一个 crate。

本期实施 issue：ABI [#180](https://github.com/hh9527/telora/issues/180)、runtime [#181](https://github.com/hh9527/telora/issues/181)、codegen [#182](https://github.com/hh9527/telora/issues/182)、初始化 [#183](https://github.com/hh9527/telora/issues/183)、CLI [#184](https://github.com/hh9527/telora/issues/184)。均关联为伞 issue #179 的子项。

## 里程碑与实施方式

本期实施 RFC 0283–0287；RFC 0288 仅为后续草案，不建立本期交付依赖。隐藏 native 的 run/serve 明确拒绝。

### M1：首个可执行原型

完成 ABI 的必要子集、最小 Cranelift codegen 和隐藏 native eval 入口。跑通标量、分支、函数调用、来源错误；不要求完整 prelude/语言资产可执行。缺失能力执行前明确报 unsupported，不回退旧 VM。

### M2：整图执行闭合

分类表、泛型实例化、闭包/enum/dyn 等规则齐备；完成数据注入、property/顶层需求计算与 main-world 发布。check/eval/eval-with 使用正确阶段，only-types 永不创建执行上下文。

### M3：eval-with 总装验收（本期终点）

完成 check/eval/eval-with 范围的语言用例和诊断差异，记录真实负载的编译、初始化、执行时间与峰值内存。run/serve 和 RFC 0288 的服务边界回收明确延期，不阻塞本期关闭。

每个模块先保证独立编译和少量有效单测，再进入后继模块；诊断/corner cases 优先用 .telora 测试资产。不要为无预期收益的中间阶段反复跑性能。所有阶段性成果提交并推送到远端独立分支。默认后端切换、旧模块摘除及 main 合入不由本伞 RFC 自动授权，另行验收和决定。

## 用户可见边界

隐藏 `--native` 不进入普通帮助、README 或 guide。默认命令仍走旧路线；native 尚未支持的命令或表达式明确拒绝。具体参数放置与冲突行为在 RFC 0287 落实。query/only-types 保持纯静态；普通 check 到初始化，其他命令遵守已有语言与 CLI 语义，不通过改语义掩盖能力缺口。

## 可执行验收与伞 issue 关闭条件

资源验收遵循 [LANGUAGE §10.2](../docs/design/LANGUAGE.md#102-fuel-和配额)
及 RFC 0010 的既有定位：fuel 约束失控执行，不作精确成本计费。验证递归、重复
控制流和 callback 重入的预算边界、耗尽传播与来源；不以逐字节、逐元素或正则
状态转换计数作为落地前提。内存、输入规模及 native 操作自身的终止性独立验收。
当前 native 的表达式与 helper 细粒度扣费是实施现状，不是新的语言契约；需要按
这一定位审计并收敛，不能通过扩展正式语言语义来合理化已有实现。

- RFC 0283–0287 五个本期子项均给出实现提交、相应测试证据、剩余限制；不以仅提交草案视为子项完成。
- 新运行时依赖审计证明没有旧 VM 求值、类型推断或 Val 深复制桥。
- 原有默认 CLI 测试通过；native 完整目标用例覆盖静态错误、初始化错误、entry 运行与来源诊断。
- 初始化发布保留共享、main 引用和来源，单次执行资源正确释放；失败不发布成功结果。服务边界复制回收不在本期验收范围。
- 记录基准版本、命令、机器/target、编译/初始化/执行分段时间和内存；先观察，不预设胜出结论。
- 如能力推迟，显式调整伞范围及 issue，不用 silent fallback 宣称全部完成。

## 延后的方案和风险

Wasm/Wasmtime、Pulley、另建字节码解释器、AOT、沙箱和跨平台后端不在首期范围。直接 native 执行不自动提供沙箱，沿用现有信任边界。

重点风险是 ABI/来源保真、JIT 代码生命周期、helper 的指针有效性、真实递归需求、host 资源回收，以及单次长请求在边界回收前的内存增长。相应契约归属子 RFC，不以持续自动 GC 或旧实现兜底消解设计问题。
