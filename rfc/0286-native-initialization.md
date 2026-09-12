# RFC 0286：Native 整图初始化、property 与发布

- 状态：实施中；多入口代码计划与运行时需求状态表已建立，整图调度接入中
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#183](https://github.com/hh9527/telora/issues/183)

## 动机与范围

在 native 路线重建既定三阶段语义，不半发布用户结果。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 阶段一为无 VM 静态求解；阶段二为初始化；阶段三为 entry 执行，后两阶段完全消费已求解类型。
- 创建一个整图 Initialize WorkWorld，先解析/注入数据模块，再计算顶层导出与 property；所有模块共享初始化上下文。
- ExportId 和 (TypeId, PropertyTypeId) 为稳定需求键；native runtime 管理未开始/计算中/完成/失败状态，函数代码访问该状态，不与 codegen 共用可变求值表。
- 主动驱动整图顶层值/property 完成；内部按需求值处理相互依赖，遇到真实递归需求给出环诊断，Failed 不重复报告。
- 全部成功后将可达初始化数据一次复制到 main-world 并固化；保留别名、共享和来源，TypeId 不重分配。
- 初始化失败禁止执行 entry 和发布成功 session；依旧允许输出已收集诊断。

## 实施计划

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

### 当前实施证据

`compile_roots` 在一个代码内存 owner 中注册多个 HIR 入口，按 HIR ID 排序去重，共享函数及间接调用分派。已验证同一计划执行工厂函数生成闭包，将闭包及捕获发布至 main world，再由另一入口调用；初始化旧句柄被拒绝。

Runtime 需求键使用已 resolve 的导出 SymbolId，或 `(TypeId, PropertySite, PropertyTypeId)`；field/variant property 必须保留 site，不能与类型本身的 property 混淆。状态为 Pending / Evaluating / Ready / Failed。首次递归请求将状态置为 Failed 并返回环错误，后续请求只传播 Failed。发布前所有已注册需求必须 Ready，需求值与显式根共用一轮别名复制，发布后状态表持有新的 main-world 描述符。

生成代码中的顶层值引用现已连接需求状态表：按已 resolve SymbolId 注册无参数初始化函数，首次读取按需执行，后续读取复用完整 native 描述符。初始化函数地址只在 helper 调用期借用，不进入语言值或运行时持久表。已验证共享数组、顶层工厂生成的闭包、发布后重复读取、循环依赖只诊断一次且阻止发布。

`compile_modules` 已支持从所选模块的封闭 scope 注册全部顶层值绑定，包含未使用的私有值；不支持的初始化代码会明确编译失败。`initialize` 主动遍历需求，内部仍按需处理依赖，全部成功后统一发布；export API 只在成功发布后开放，并消费 MIR 已确定的导出别名绑定。已验证未使用的顶层 `fail!` 阻止整个初始化发布，重复初始化不重复诊断。

Native `property` 工厂已生成真实可调用的 provider 闭包，捕获 PropertyTarget；provider 将该目标的 ABI capability bits 与 previous 属性按位合并，并产生 PropertyAttr。工厂与 provider 使用不同编译键和各自封闭签名。已验证无 previous、合并 Type/Field 两种目标及发布后继续调用。

类型级 property 清单现已生成需求初始化函数，按 MIR provider 顺序执行 configured factory 和 provider，传入 owner 元数据及 previous，执行 capability admission。`get_type_prop`/`evidence` native 适配器按封闭 TypeId 查询：可选缺失为 None，必有属性缺失为 Failed，provider 失败直接传播。所选模块中的未使用 property 也纳入主动初始化。已验证双 provider 合并、配置闭包、发布后缓存、property/顶层值互相依赖的单次环诊断、能力拒绝、未使用 property 失败阻止发布。

FieldPropertyCtx / VariantPropertyCtx 与对应查询已接入，字段名、索引、owner/字段/payload 类型均来自封闭骨架；无 payload 的 variant 使用 None，需求键保留成员 site。已验证 field、带 payload/无 payload 的 variant 及统一初始化发布。泛型 property 的不同闭合实例也验证了独立见证与顺序合并；字段读取先使用骨架的物理类型，再按 MIR 表达式类型做 TypeOf→Type 标记适配。

数据注入已接入：core 暴露只读 `data_plan` 解析接口，复用 JSON/YAML/TOML 的扁平验证计划，不创建 Heap/VM。Native 根据已登记 std/value.Value 导出及封闭 payload 类型直接物化，保留值/键来源、YAML Bytes 和 TOML 时间标签。数据模块使用稳定 SymbolId 的需求槽，必须在初始化前注入；重复注入被拒绝，漏注入明确失败并禁止发布。已验证命名空间读取、初始化发布，以及 Bytes 切片的 backing 共享。

Native 语义 Value 已支持直接输出紧凑 JSON：迭代遍历原有表，只构造输出文本和遍历栈，不转换为旧 VM 或 host Value 树。Object 按有序 Dict 输出，保留字符串转义；Bytes、时间值及非有限 Float 明确拒绝。已验证发布前后输出一致，以及 JSON 不支持的数据标签诊断。

更丰富的 owner 上下文、泛型函数族/所需 native 操作及 CLI 初始化入口尚未完成，因此暂未覆盖包含整个标准库的全图，不据此宣称 CLI 初始化已可用。

用 .telora 用例覆盖数据依赖、跨模块导出、顶层值/property 双向依赖、真正求值环、单次计算/失败传播、发布后值一致性。静态阶段证明不持有 native VM/context。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
