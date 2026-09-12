# RFC 0286：Native 整图初始化、property 与发布

- 状态：本期已实施并验收；check/eval/eval-with 初始化与统一发布已接通
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

截至 `4bf46dd`：CLI check/eval/eval-with 已接通整图初始化与发布。
生产 codegen 消费 `SealedExecutable`；模块 check 初始化所选模块的具体值，
eval/eval-with 按导出入口裁剪普通值依赖，property/check 仍为 session 初始化根。
模板函数族仅存在于静态阶段，不作为多态运行时值物化；执行计划中的函数实例
均由 MIR 确定。`compile_modules` 现为测试辅助入口。
Never 需求允许登记和执行，但不能成为 Ready 或成功发布的值。
共享泛型参数的身份规则已由 RFC 0289 确认并实施，不再是待决项。

本期验收复核（`573a229` 及前序提交）：

- `data_modules_are_injected_before_initialization_without_old_values` 验证数据先注入、
  重复注入拒绝、漏注入失败且不发布；CLI 数据模块与 eval-with 来源输入测试通过。
- `property_queries_execute_provider_chains_and_cache_results`、
  `property_and_global_cycles_fail_once_and_unused_properties_initialize` 验证链式归约、
  顶层值/property 依赖、缓存、真实环和未使用 property 失败禁止发布。
- `generated_global_reads_initialize_once_and_propagate_cycles` 及 runtime 需求表测试
  验证单次计算、失败传播；发布测试覆盖共享、真实闭包环、来源和中途失败原子性。
- `static_cli.rs` 在 only-types 分支结束前不会调用 native Session；
  Session 仅接受 seal 后的静态图，先编译，再创建 CallContext，静态求解模块不依赖它。
- 139 项 native 测试、3 项独立实验和 84 项 CLI 测试通过；真实 world-model
  eval-with 的结果与默认路径逐字节一致（RFC 0282、0289）。

此结论只关闭初始化子项，runtime 资源审计和伞路线总装验收继续推进。

### 实施过程记录

以下段落按推进顺序保留。当时的测试数量和“下一处缺口”仅代表该阶段，
不能作为当前未完成项；其中函数族运行时值的早期设想已由上面的静态模板边界取代。

`compile_roots` 在一个代码内存 owner 中注册多个 HIR 入口，按 HIR ID 排序去重，共享函数及间接调用分派。已验证同一计划执行工厂函数生成闭包，将闭包及捕获发布至 main world，再由另一入口调用；初始化旧句柄被拒绝。

Runtime 需求键使用已 resolve 的导出 SymbolId，或 `(TypeId, PropertySite, PropertyTypeId)`；field/variant property 必须保留 site，不能与类型本身的 property 混淆。状态为 Pending / Evaluating / Ready / Failed。首次递归请求将状态置为 Failed 并返回环错误，后续请求只传播 Failed。发布前所有已注册需求必须 Ready，需求值与显式根共用一轮别名复制，发布后状态表持有新的 main-world 描述符。

生成代码中的顶层值引用现已连接需求状态表：按已 resolve SymbolId 注册无参数初始化函数，首次读取按需执行，后续读取复用完整 native 描述符。初始化函数地址只在 helper 调用期借用，不进入语言值或运行时持久表。已验证共享数组、顶层工厂生成的闭包、发布后重复读取、循环依赖只诊断一次且阻止发布。

`compile_modules` 已支持从所选模块的封闭 scope 注册全部顶层值绑定，包含未使用的私有值；不支持的初始化代码会明确编译失败。`initialize` 主动遍历需求，内部仍按需处理依赖，全部成功后统一发布；export API 只在成功发布后开放，并消费 MIR 已确定的导出别名绑定。已验证未使用的顶层 `fail!` 阻止整个初始化发布，重复初始化不重复诊断。

Native `property` 工厂已生成真实可调用的 provider 闭包，捕获 PropertyTarget；provider 将该目标的 ABI capability bits 与 previous 属性按位合并，并产生 PropertyAttr。工厂与 provider 使用不同编译键和各自封闭签名。已验证无 previous、合并 Type/Field 两种目标及发布后继续调用。

类型级 property 清单现已生成需求初始化函数，按 MIR provider 顺序执行 configured factory 和 provider，传入 owner 元数据及 previous，执行 capability admission。`get_type_prop`/`evidence` native 适配器按封闭 TypeId 查询：可选缺失为 None，必有属性缺失为 Failed，provider 失败直接传播。所选模块中的未使用 property 也纳入主动初始化。已验证双 provider 合并、配置闭包、发布后缓存、property/顶层值互相依赖的单次环诊断、能力拒绝、未使用 property 失败阻止发布。

FieldPropertyCtx / VariantPropertyCtx 与对应查询已接入，字段名、索引、owner/字段/payload 类型均来自封闭骨架；无 payload 的 variant 使用 None，需求键保留成员 site。已验证 field、带 payload/无 payload 的 variant 及统一初始化发布。泛型 property 的不同闭合实例也验证了独立见证与顺序合并；字段读取先使用骨架的物理类型，再按 MIR 表达式类型做 TypeOf→Type 标记适配。

数据注入已接入：core 暴露只读 `data_plan` 解析接口，复用 JSON/YAML/TOML 的扁平验证计划，不创建 Heap/VM。Native 根据已登记 std/value.Value 导出及封闭 payload 类型直接物化，保留值/键来源、YAML Bytes 和 TOML 时间标签。数据模块使用稳定 SymbolId 的需求槽，必须在初始化前注入；重复注入被拒绝，漏注入明确失败并禁止发布。已验证命名空间读取、初始化发布，以及 Bytes 切片的 backing 共享。

Native 语义 Value 已支持直接输出紧凑 JSON：迭代遍历原有表，只构造输出文本和遍历栈，不转换为旧 VM 或 host Value 树。Object 按有序 Dict 输出，保留字符串转义；Bytes、时间值及非有限 Float 明确拒绝。已验证发布前后输出一致，以及 JSON 不支持的数据标签诊断。

顶层泛型实例现以封闭 MIR 的 `GenericInstanceId` 注册初始化需求，复用其节点类型和实例引用，不在 codegen 替换类型参数。整模块初始化主动完成已有具体实例；引用与调用读取缓存的实例值，支持初始化表达式产生带捕获闭包并统一发布到 main world。模板定义不按未实例化签名物化。已验证不同类型实例、嵌套泛型调用、闭包初始化及发布后调用。

更丰富的 owner 上下文、作为多态值传递的函数族/所需 native 操作及 CLI 初始化入口尚未完成，因此暂未覆盖包含整个标准库的全图，不据此宣称 CLI 初始化已可用。

已增加发布后闭包调用接口：host 以 native 描述符传入闭包，编译计划按内部函数 ID 验证其封闭签名及所属代码计划，传递原捕获描述符调用机器码，不复制捕获对象图。无初始化发布、错误参数宽度和未知函数 ID 会明确拒绝。前向 decl 连接到实际 def；Some 等带 payload 枚举构造器可按封闭签名生成为普通可调用函数值。相关验证后 native JIT 测试为 43 passed。

完整 `std/entry` 图此前暴露的 module 2 / desc 属于 `std/dyn`。现已补充 Dyn boxed 存储、pack/desc/project_with 及标量 check 适配器。装箱仅复制固定宽描述符，其底层对象继续共享；发布沿用 ValueTable 的统一别名复制，保留 payload 来源和已有 TypeId。投影比较已存身份，不执行类型推导。已验证匹配/不匹配投影、标量检查及发布后的数组共享，native JIT 44 passed。Dyn 暂统一使用 ABI 已允许的 boxed 表达，inline 优化后置。

完整 eval-with 仍需补齐 std/dyn 其余访问接口及其后续依赖能力，不绕过加载图中的初始化任务。

Dyn kind 与命名字段访问已接入：类别来自封闭类型骨架，Enum 依据当前 variant 的 payload 区分 Atom/Tagged；Struct 使用骨架字段表，Dict 使用有序键查找。字段缺失和错误对象类别作为 Result 错误值进入语言层，成功结果只装箱子值描述符。已在同一 .telora 用例验证发布后 Array/Atom/Tagged 分类、Struct/Dict 字段读取和两类失败，native JIT 44 passed。fields/array_items/tuple_items/tag/payload 等剩余接口仍继续推进。

Dyn 集合与枚举查询已接入：fields、array_items、tuple_items、tag、payload 构造所请求的结果集合，子对象图仍共享；Bool 与无 payload 枚举保留正确 tag/None 语义。get_field_value/get_variant_index/get_variant_payload 消费骨架索引，不匹配索引直接 Failed，不伪装为 None。已验证有序键、异构元组、成功/缺失 payload 和非法访问；独立的 .telora 失败用例产生三条诊断，重复初始化不重报且不发布。native JIT 45 passed。

完整 `std/entry` 图现已通过这些 Dyn 适配器的生成，下一项缺少 module 20 / prepare（std/fmt）适配器；eval-with 仍未完成。

std/fmt 的 prepare/from_string/from_int/from_float/concat/render 已接入独立 FormatTable。节点存放输入描述符，拼接引用 String/Fmt 数组，不创建 Arc 树；发布通过统一别名表搬运可达节点。模板解析保留转义大括号、字段校验、重复字段和字符串间隔；渲染遵守原有 128 层限制。正常与非法模板/拼接的 .telora 用例验证通过，native JIT 46 passed。完整 std/entry 下一处缺口为 fail! 的 subject 参数和动态消息，仍继续推进。

fail! 与 panic 的动态 String 消息已连接生成代码；fail! 按求值顺序消费 subject，只收集其已有来源位置，不复制 subject 对象图。规则位置为 primary，subject 来源去重后作为 secondary；首次失败报告后需求表只传播 Failed。已验证动态顶层消息、重复 subject、初始化不发布及 CLI 三处位置输出，native JIT 47 passed，native CLI 用例通过。完整 std/entry 图下一处缺口为 module 19 / compile（std/regex）。raise!/warn!/BlameError 构造的完整诊断语义仍未完成。

std/regex compile/is_match/prepare 已接入独立 RegexTable。编译验证命名捕获，prepare 对照封闭字段表、Option 标记和静态 property 存在集合检查匹配及可选性，不查询或执行嵌套 property 值。Regex 描述符在统一发布中按旧 HeapId 去重，prepare 返回原描述符；匹配借用原输入字符串。已验证有/无匹配、可选捕获、匿名捕获错误、字段与捕获不符、必选性错误及发布后的资源别名，native JIT 48 passed。完整 std/entry 图下一处缺口为 std/string.join，仍继续补齐后续标准库适配器。

std/string 的 join/join_lines/split/lines/starts_with/ends_with/contains/replace/indent/ensure_trailing_newline/trim_margin 已接入。split/lines 对 heap String 返回共享 backing 的 UTF-8 切片，保留原先的末尾空项与 CRLF 语义；新构造文本将拥有的缓冲区移入 StringTable。已验证 Unicode、空分隔符、换行、负缩进/空 margin 失败和发布后的切片共享，native JIT 49 passed。泛型 parse_with 尚未接入；完整 std/entry 图下一处缺口为 module 3 / kind（std/type-desc）。

用 .telora 用例覆盖数据依赖、跨模块导出、顶层值/property 双向依赖、真正求值环、单次计算/失败传播、发布后值一致性。静态阶段证明不持有 native VM/context。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
