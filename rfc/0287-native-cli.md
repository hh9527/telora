# RFC 0287：隐藏 --native 接入与执行语义对齐

- 状态：实施中；check/eval/eval-with 已接入实验后端，完整语义覆盖和配额验证待完成
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#184](https://github.com/hh9527/telora/issues/184)

## 动机与范围

在不切换默认运行时的情况下提供逐步可用的新路线。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 隐藏 --native 选择新后端，普通帮助、README 和 guide 不列出；默认行为保持旧路线。CLI 仅做必要分流。
- 首批支持一个最小 eval 纵向用例，随 RFC 0285 进展接入；随后覆盖 check/eval/eval-with 及批量 check --lib/--tests；本期到 eval-with 截止，native run/serve 明确报 unsupported。
- check --only-types 仍在静态闭合处结束，即使选择 --native 也不创建 native session；普通 check 到初始化完成，eval/eval-with 的既定输出语义保持，entry 调度沿用各命令语义。
- 对每条命令维护 supported/unsupported 清单；未支持组合明确报错，不静默忽略 --native、不回退旧 VM。
- query 等纯静态操作不因 native 选择而创建执行环境；具体开关放置位置和不适用命令行为在实现时明确。
- 用现有 .telora 测试资产比较语义与诊断；旧实现只可作为测试对照，不是新路线运行依赖。

## 实施计划

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

### 当前实施证据

命令局部隐藏参数 `check --native`、`eval --native`、`eval-with --native` 已接入：静态求解仍使用共同 MIR，只有 seal 成功且需要执行时才创建独立 native session。check 支持选择模块及 --lib/--tests，--only-types 和布局导出不创建 native session。未声明 --native 时仍使用原执行路径；其余命令暂不接受该选项。

Native session 编译已加载的完整模块集合，通过与 linker 无关的 catalog 读数据接口解析、注入所有数据模块，再完成初始化和发布。eval 在执行前验证导出的权威 std/value.Value 身份，成功后直接从 native 对象输出 JSON。初始化失败保留位置且不重复附加通用失败诊断；数据解析保留原结构化诊断。隐藏开关不进入普通帮助或用户文档。

已验证真实 CLI 的泛型闭包初始化、--only-types 零执行、未使用顶层 fail 阻止初始化、单次来源诊断，以及数据模块 check 和 eval JSON 输出。完整 std/value 依赖图包含的 nullary enum 比较按封闭布局翻译为 tag 比较。

eval-with 在执行前验证权威 std/entry.Eval 身份，并从封闭骨架读取 config/evaluate/Context 的字段与类型。全图初始化发布成功后，校验唯一非空来源/环境变量名称、准确匹配来源清单以及 args 许可，再将声明的输入直接构造为 native Context，调用发布后的 evaluate 闭包。集成测试覆盖 JSON 数据模块、外部 JSON 来源、环境变量、Unicode 参数、配置拒绝和执行失败来源诊断；真实 `check --native std/entry` 已通过。

基础 codec 解码已接入封闭目标类型，支持标量、Option、Array、Tuple/Unit、结构记录、Dict，以及普通名义 record/newtype/enum（含递归类型和泛型实例）；字段缺失/多余或值不匹配返回携带来源的 `Err(BlameError)`，仅在用户调用 `raise!` 时转成执行诊断。字符串等叶子复用原对象描述符，不经过旧 VM 或 host Value 树。NewtypeTable 使用独立槽位表，发布时保留对象与其 payload 的共享关系。带适用 codec property 或构造检查的解码仍明确拒绝，不生成未经检查的值。

直接构造 struct/newtype/payload variant 的封闭 checker 已接入普通函数调用；checker 闭包工厂作为初始化项求值一次并发布，泛型 checker 消费 MIR 实例。codec 解码内的检查回调仍待接入。

仍未完成完整语法/native API 覆盖（包括 codec 属性编码/解码、解码构造检查）及配额语义，因此不据此宣称整体路线落地。

验证默认路径不变、隐藏帮助、显式 unsupported、only-types 零执行、各命令停止阶段正确。先少量冒烟，再补完整 corner cases；完成总装后才测编译/初始化/执行耗时和峰值内存，不承诺性能收益。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
