# RFC 0285：SealedMir 到 Cranelift 的机械 codegen

> 后续决议：RFC 0289 移除 cast 并禁止最终匿名 Record 类型，相关历史记录不再构成兼容契约。

- 状态：实施中；已接入整图初始化与隐藏 CLI，继续补齐语言/native 覆盖
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#182](https://github.com/hh9527/telora/issues/182)

## 动机与范围

直接生成机器码，不先建设新字节码或 Wasm 后端。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 首先确定 Cranelift 版本、Cargo feature 和主机 target 支持，依赖应留在独立 native 模块/crate 边界。
- 直接消费 SealedMir、最终 TypeId、符号身份和泛型实例化结果；不得重做 resolve/类型推断或按名字识别内置能力。
- 先跑通常量、标量运算、局部值、分支、函数调用和返回，再补循环/递归、聚合操作、闭包及间接调用等实际 MIR 节点。
- Rust helper 承担对象和原生资源操作。优先固定槽位和统一 ABI，不先做对象访问内联、寄存器分配策略或复杂优化。
- 生成代码和函数地址归 native session 所有，其生命周期覆盖所有闭包和调用；返回状态接入统一诊断。
- 缺失 codegen 规则在执行前明确报 unsupported，并附带位置/类型；不能回退旧 VM。

## 实施计划

### 当前入口与验收状态

截至 `4bf46dd`，隐藏 CLI 已通过 `jit::compile_executable` 消费
`SealedExecutable`，使用既有 TypeId、实例及执行闭包生成机器码。
实际 check/eval/eval-with、初始化发布和发布后的调用已接通；低层 `compile`
和 `compile_roots` 仍用于局部验证。Never 顶层引用、Dict 字段访问已补齐。
当前 native 测试为 139 项单元测试和 3 项集成测试，CLI 验收为 85 项；
语言模块 check 不执行所有测试闭包，不能据此称完整语言运行覆盖已经通过。
共享泛型参数的身份规则已由 RFC 0289 确认：同一个 T 必须具有相同 TypeId，
记录构造由整图上下文决定最终身份，不允许匿名 Record 成为最终用户值。

下面的首批实现及各阶段进展保留历史上下文，旧测试数量和当时未接通项
不代表当前支持范围。完整落地仍按伞 RFC 的验收条件判断。

### 实际语言闭包执行审计（2026-09-13）

新增仅用于测试的 `runtime/tests/language.rs`：读取默认语言验收记录，复用既有
`.telora` 源码，通过 SealedExecutable 初始化后调用已发布的 Test 闭包，对照结果。
它不实现 native test CLI，也不调度 fixture；当前覆盖 402 个非 fixture 闭包，
另外 21 个 fixture case 不在此 harness 范围。大型模块编译使用与当前 CLI 相同的
8 MiB 线程栈；运行预算取 CLI 的 fuel/显式栈槽/分配上限。

最初发现 7 个差异。5 个诊断已对齐：字段重命名碰撞、Function/Type 不可编码、
数组越界、非有限浮点。`Countdown(100)` 递归 checker 则因直接 newtype 构造
经过了额外合成函数而提前触及调用深度限制；现在和 enum 构造一样，直接根据
封闭选择生成 payload 检查及构造，仍保留作为一等函数传递的构造器路径。

最初修复后 401 个闭包结果一致；剩余 `compiler-semantics/tail_calls` 的 1500 次
尾递归现已通过，402 个非 fixture 闭包全部与默认观察一致，没有提高深度上限、
减少次数或排除用例。

尾位置调用使用生成的 C ABI wrapper 循环：私有函数体退出自己的调用/栈 guard
后返回内部状态 2，wrapper 取出下一次调用描述并执行私有 body，公开入口仍只
返回 Success/Failed。转交保持实参求值顺序，只复制描述符；目标函数在任何 callback
前保存参数及闭包描述符，防止嵌套调用复用转交缓冲时覆盖当前输入。
需要返回类型适配或构造检查的路径不转交，保留原有完成边界。

`tail-calls.telora` 另外在调用深度 8、显式栈 8192 words 下覆盖 1500 次异宽互递归、
零参数捕获闭包、嵌套 callback，以及 Unchecked 构造检查和 TypeOf 元数据适配。
非尾递归仍受深度限制，无限尾递归仍由 fuel 中止。

复跑：先 `bash scripts/test-language.sh`，再
`cargo test -p telora-native --features jit published_language_test_closures -- --ignored --nocapture`。
该手动审计已通过；最终 workspace all-features 验证包括 141 项 native 单测
（另有 1 项手动审计默认忽略）、3 项独立实验和 84 项 CLI 测试，均无失败。

### 表达式支持清单（35e9cad）

本表描述已封闭 MIR 的 lowering 路径，不代表所有组合的 corner case 已穷尽。
证据名称为 `crates/telora-native/src/jit/tests.rs` 中的测试或现有语言资产。

| 类别 | 当前路径 | 代表证据与边界 |
| --- | --- | --- |
| Int/Float/Bool/Unit、String/Bytes、类型元数据 | 固定布局常量及文字对象 helper | `machine_code_returns_materialized_scalar_and_unit`；Never 不物化 |
| 算术、比较、短路 | `jit/scalars.rs`，结构相等经 helper | `scalar_machine_code_handles_recursion_and_checked_arithmetic`；共享 T 必须统一为同一个 TypeId（RFC 0289） |
| let/def、引用、导入、泛型实例 | SymbolId/GenericInstanceId 对应局部槽或需求槽 | `generic_calls_consume_closed_instances_without_substituting_types_at_runtime`；无运行时函数族 |
| 函数、捕获、递归、间接调用 | `jit/functions.rs` 与封闭签名分派 | `direct_functions_have_independent_frames_and_support_recursion`、`native_local_mutual_recursion_uses_stable_function_slots` |
| block、if、return | 顺序求值及 SSA 合流，Diverged 不读取值 | `machine_code_branches_on_argument_and_preserves_selected_value_origin` |
| match、if-let、let-else、? | `jit/patterns.rs` | `native_never_payload_branches_do_not_require_runtime_values`、`native_propagation_preserves_failure_and_payload_origins` |
| Array/Tuple/Record/Dict、spread、投影与索引 | `jit.rs`、`jit/sequences.rs`、`jit/records.rs` | `machine_code_constructs_native_objects_without_old_vm`、`native_record_spreads_keep_effect_order_origins_and_shared_backing` |
| Dict 命名字段读取 | 封闭元素类型及有序键查找 | `dictionary_field_access_preserves_value_and_reports_missing_key` |
| enum/newtype、构造检查、结构更新 | 封闭构造选择及普通 checker 调用 | `generated_enums_preserve_inline_and_boxed_payloads_through_publication`；construction-boundaries、newtype-constructors 资产 |
| TypeApply/类型标注/TypeMetadata | 消费既有实例和类型身份 | `native_explicit_generic_enum_constructor_preserves_sealed_selection`；模板不进入值域 |
| interpreter、插值 | 封闭配对计划和 Display 结果 | `native_interpreter_consumes_sealed_pairings_and_keeps_operand_lazy`；eval/interpolation 实际执行 |
| fail/panic/raise/warn/blame/debug | 状态与来源 helper | `native_debug_is_bounded_and_preserves_shared_descriptors`；failure-subjects 资产 |
| CheckedCast | 已按 RFC 0289 移除 | 不再是语言能力，无兼容 lowering |

原生模块调用由 `jit/natives.rs` 按已 resolve 的模块 ABI 身份和导出键适配，
未知 ABI 明确拒绝。构造契约审计确认 `attach_check` 只接受 Nominal owner 的
struct/newtype/payload variant，泛型实例也保留 Nominal owner；普通 Array/Tuple
不产生自身的 construction check。已删除该路径遗留的全表扫描和“尚未接通”拒绝，
容器元素的名义构造仍执行各自 checker。
未识别 HIR 最终返回 unsupported，不回退旧 VM。

### 首批实现历史

`telora-native` 的可选 `jit` feature 使用稳定版 Cranelift 0.135.0。当前验证平台为 x86_64 Linux（64-bit little-endian）。默认 CLI 未依赖该 crate，也未增加隐藏开关；不存在选择 native 后跳过初始化的临时捷径。

`jit::compile` 消费 SealedMir 中指定的表达式或无捕获、单态 Closure：支持 Int/Float/Unit 常量、参数引用、Bool 条件分支和纯结果 block。读取已闭合 TypeId/符号身份，不使用名字推断；其他表达式、局部绑定和捕获/导出引用明确返回带位置的 unsupported。此接口不等同于模块求值，不跳过顶层副作用声称完成 eval。

真实入口为 C ABI `(context, args_ptr, result_ptr, closure_ptr) -> u32`。第四个指针借用闭包描述符，生成函数通过独立运行时 helper 读取捕获；无捕获的 host 根调用传空。调用前核对参数数量/类型/宽度；仅 Success 解码结果并检查 stamp；Failed 不读结果。返回分支按 word 生成 SSA 合流，保留被选值的来源。所有代码地址留在 Compiled 内，借用期间调用，编译失败或 owner 析构时释放 JIT 内存，地址不对外发布。

验证：`cargo test -p telora-native --features jit` 通过 6 项测试，包含真实机器码的常量/Unit/浮点和三参数条件选择、错误参数拒绝、unsupported 不丢语句，以及失败返回不读取未写结果。当前没有 runtime helper 对象访问、程序内部函数调用、整图初始化或 CLI；后续按本 RFC 继续补齐，不关闭 #182。

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

### 对象 helper 接入

后续已接入独立 runtime 的对象 helper：机器码可以构造 String、Array、Tuple/Record、有序 Dict，并按已求解布局读取字段/数组元素。对象描述经固定栈缓冲区传递，引用对象不经过旧 Val 或深复制桥。CallContext 持有 native Runtime，参数验证所属 session，结果继承该 session 身份；helper 的失败携带来源并返回 Failed，生成代码直接传播，不能读取失败结果。

`cargo test -p telora-native --features jit` 当前通过 11 项测试，包括真实机器码构造四类对象、读取已发布 main 数组及有来源的越界失败。尚未实现的 construction check/字段 property、捕获/导出引用和普通语句仍明确拒绝；程序内部函数调用、整图初始化和 CLI 后续推进。

### 函数与标量运算进展

后续已支持普通 let/def、直接函数调用和递归。按稳定 HIR 身份注册函数，声明后排队生成函数体，递归引用不会重复编译；调用使用独立参数/结果栈区域及同一四指针 ABI。无用绑定仍执行初始化并传播失败，不通过删除语句制造成功。

词法闭包支持按稳定 SymbolId 排序的捕获计划；参数和函数内部声明不计入外部捕获。运行时环境独立存入 Vec 槽位，其缓冲区包含完整捕获值，身份编码为环境表 HeapRef + 1。无捕获闭包同样分配空环境槽，保证同一代码表达式被求值两次时具有不同函数值身份。初始化发布会复制嵌套闭包与捕获对象，并保持别名；FunctionId 保留为代码计划身份，不保存机器码地址。全局函数引用读取已初始化值，不重新构造闭包。此处取代早期以环境 0 表示无捕获函数值的简化，不改变完整值宽。

间接调用消费 callee 的封闭 Function TypeId，生成该签名下的 FunctionId 分派；所有可达函数完成注册后再生成分派体，未知或签名不符的 ID 产生一次 Failed。已验证函数参数、返回带捕获闭包、条件选择函数，以及显式实例化泛型函数作为高阶参数。Runtime 首次执行时绑定 Compiled 代码计划身份，拒绝混用其他编译结果；代码内存仍由 Compiled 独占。整图多入口计划与初始化调度尚待接入，这里不是完整 CLI 生命周期的完成声明。

标量 Int/Float 运算与比较直接生成 Cranelift 指令；整数溢出、除零以及非有限浮点结果走有来源的失败路径，逻辑运算短路。当前 `--features jit` 共 13 项测试通过，包含 .telora 递归阶乘资产。Float remainder、非标量操作、泛型实例、捕获/间接调用等仍继续推进；未完成项不走旧 VM。

### 闭合泛型实例进展

函数注册键扩展为 `(HirId, GenericInstanceId?)`，类型直接读取对应实例的归一化节点表；表项缺失即报错，不退回模板类型或执行替换求解。实例内调用直接读取 MIR 的 reference 边，显式 TypeApply 沿 callee 边消费已存在的实例记录，不重新匹配签名。隐式、显式和嵌套泛型调用资产已通过，当前总计 14 项测试。运行时没有模板参数推导。

### 构造检查与 newtype

直接构造的 struct、newtype、payload variant 已执行 MIR 中对应 owner/site 的封闭 checker。checker 表达式作为独立初始化项求值一次，生成普通闭包；缓存以 owner/site 为身份，随全图初始化发布到 main world。构造时通过封闭函数签名和现有 FunctionId 分派器调用，不重新推导泛型参数。返回 Err(BlameError) 在构造位置报告一次诊断；checker 自身失败直接传播，不读取结果槽或重复报告。

struct checker 的 Unchecked(T) 参数使用同一静态骨架，只变更描述符的类型标记；字段读取沿封闭 owner 骨架取类型。newtype 构造器作为普通一等函数注册，构造前检查 payload；投影和模式解构直接读取封闭成员类型。泛型 checker、工厂只初始化一次、发布后重复构造、拒绝与执行失败已有覆盖。

codec 解码也通过普通分派器调用这些 checker。codegen 沿封闭目标类型的成员/参数边，收集可达检查，生成包含 owner/site、初始化槽及临时代码地址的调用包；这些地址仅借用于当前调用，不进入 heap。解码器在构造边界释放 Runtime 借用再调用 checker：子值检查先于父值与后续兄弟字段；检查拒绝保留为原生 Err(BlameError)，执行失败直接传播。缺失的封闭检查记录报内部错误，不补猜或跳过。

编码/解码共用的 codec 调用包也包含可达类型的 property 初始化身份，经现有 demand 机制取得并复用原生 property 值。rename_all 读取实际 case 值，变换字段/variant 的外部名字并拒绝重名；untagged 编码省略标签，解码遍历全部候选（包含构造检查），唯一成功才接受。无匹配合并拒绝信息并保留首个具体拒绝的来源，多匹配返回歧义 BlameError；执行失败中止遍历。

string.parse_with 按封闭 TypeId 解析 Int/Float/String/Option，结构类型按 ParseBy 的命名捕获范围递归构造，复用原字符串的完整值或仅分配实际捕获文本。DecodeByParse 复用该解析器，EncodeByDisplay 通过封闭签名分派 property 内的 Fn(Dyn) -> Fmt，直接遍历原生格式对象生成 String。属性必须成对声明，缺失能力与回调失败明确反馈；codec 中检查拒绝是 Err(BlameError)，普通 string.parse 的检查拒绝保持执行失败语义。已覆盖嵌套文本属性、可选捕获、非有限浮点拒绝、checker 与 untagged 组合，以及 Display 回调失败。

### 数据格式解析进展

JSON/YAML/TOML 的 parse_raw 按 native 模块身份链接，消费封闭的 TypeOf(Value) / Result(Value, BlameError) 签名。复用无 VM 依赖的数据解析计划，直接物化到 native tables；字符串解析产生的子值与键保留输入来源，临时解析 SourceId 不进入运行时值。语法错误返回 Err(BlameError)，资源限制超出产生一次执行失败。YAML alias 保持解析计划的展开语义，发布保留 native 图已有的共享关系。

JSON 紧凑 stringify 与 stringify_pretty 直接遍历 native Value 图生成文本，不构建 host Value 树。pretty 工厂在创建闭包时验证 0..16 缩进，将 Int 描述符放入普通 native 闭包环境；配置后函数的签名读取封闭工厂返回类型。缩进 0 仍输出换行，空容器保持单行。

schema_with 遍历封闭类型布局，直接构造 native Value 对象，递归 nominal 类型用稳定遍历顺序分配的 $defs/$ref 表达。Type 参数允许运行时选择，因此调用包携带封闭图的 concrete type-site property 初始化记录；查询 rename/untagged 与文本桥接选项复用实际原生 property 值，失败不重复报告。不生成 host Value 树，也不重新推导类型。完整 std/json、YAML/TOML 已纳入真实 CLI eval-with 初始化及 entry 路径验证；语言资产对比默认/native 的标量、Unit、Tuple、Dict、Option、nominal/newtype、递归结构、Result 与 rename/untagged schema 输出。

## 验收条件

分支合流现消费封闭结果类型：if、match、if-let 的活跃分支在跳转前执行已有
边界适配，正确将 `TypeOf(T)` 转成 `Type`，保留所代表的 TypeId 与来源。
`prelude-constructors` 初始化中 `Some(Int.type)` 的 match 不再产生值头部类型
不匹配，native check 通过；执行回归覆盖三种分支形式。

`stdlib-semantics` 暴露的 Never 槽缺口已修复：匹配到无居民 payload 的路径结束
生成，不读取 payload 或继续生成该分支体；含无居民参数的函数保留可比较/传递
的函数值，调用体仅报告不可达调用，不读取参数缓冲、不物化 Never。
现有资产 native check 通过；执行回归验证 `Result(Int, Never)` 经 `map_err`
后仍返回正常 Ok 内容，没有执行不存在的 Err 回调。泛型函数族身份问题独立保留。

泛型 newtype 构造引用优先消费封闭构造信息，不再进入全局泛型值初始化路径，
避免为 `Meta` 分配运行时槽。`construction-boundaries` 语言资产 native check
已通过，执行回归包含 `Wrapped(42)` 与保存 `Wrapped@[String]` 后调用。运行时槽
诊断补充 TypeId、helper 和源码位置，后续 `stdlib-collections` 的槽问题另行定位，
不将所有槽错误归为同一根因。

显式泛型构造函数引用现在穿过 HIR `TypeApply` 读取既有 member selection，覆盖
`Message.Data@[String](...)` 及 `let make = Message.Data@[Int]`，不在 codegen
重新推导类型。现有 `enum-constructors` 语言资产 native check 从 callable 未绑定
变为成功；JIT 回归实际执行直接构造与作为函数值保存后的调用。

整体验收发现并修复 String 排序运算缺口：`<`、`<=`、`>`、`>=` 现在按字符串
内容的字典序比较，支持 inline、heap-backed 和 Unicode 文本。现有语言资产
`tests/language/src/test/compiler-semantics/testee.telora` 的 native check 从
`native non-scalar operator is not yet linked` 变为成功；JIT 回归实际执行排序表达式。
该 check 证明整图生成与初始化，不等于执行资产中的全部测试闭包。

Tuple/Array spread 已接入。Tuple 根据 sealed 元素列表直接展开固定字段，Array 用独立 concat helper 按 slice 范围合并；原有元素描述符及其来源保持不变，引用的对象不深复制。Array 合并先检查总长度并计费结果缓冲区，再分配和写入对象表。语言对比覆盖空/嵌套 spread、泛型、nominal 上下文和元数据擦除；单测覆盖切片范围、main 来源、分配拒绝不新增槽位及失败源不会被后续贡献隐藏。此阶段 112 项 native 库测试通过，eval/eval-with 的默认/native 对比通过。

字段投影（含重命名、空投影、重复源字段）和 named record/dict spread 已接入。投影/record spread 按封闭字段索引读取，record 的覆盖选择在编译期完成；所有源表达式仍按源码顺序求值，只有胜出的字段适配最终类型。Dict spread 复用有序列的合并 helper，后值覆盖同名键。输出 record 运行已有构造检查，字段来源和引用 backing 保留；语言对比覆盖泛型投影、异类型字段被覆盖、空投影、Dict 键序及原值不变。额外验证空投影仍执行 receiver、被覆盖字段仍产生副作用、发布后的来源和 backing 一致以及检查失败。

Array、Tuple 与直接 enum payload 构造逐项消费封闭目标类型；TypeOf(T) 到 Type 的擦除仅修改静态标记，保留被表示的 TypeId 和来源。真实 ontology 测试模块及 spider-model 的 `Array(Type)` 初始化暴露了这项遗漏；语言对比已覆盖这三类容器边界。

Native 已消费 MIR 的 value_adjustments（含泛型实例调整），在局部表达式和函数正常/显式返回边界执行构造检查后更新 TypeId stamp；不修改已有候选的来源或 backing。Unchecked struct 字面量从其封闭 owner 骨架读取字段，避免将包装类型的一个参数误作字段列表。类型收尾阶段也把带返回转换的闭包完整签名写入调整表，再物化泛型实例；seal 验证参数不变、返回 Unchecked(T) 对应 T，且返回边界的转换记录存在。Codegen 直接读取这个函数 TypeId，不自行拼接或推断签名。语言对比覆盖多字段候选、泛型正常/提前返回、检查失败及转换后的动态类型身份。

真实 ontology `check --native --lib` 暴露出的 `<~` 和 `?` lowering 缺口已补齐。struct update 在编译期按 sealed 字段布局选择左值或 patch 字段，两个操作数按源码顺序各求值一次，保留 nominal 身份、字段来源与引用共享，并对新对象运行已有构造检查。`?` 按封闭的 Option/Result 家族分支；成功读取 payload，失败按返回边界的类型重新封装，保留原值来源并正常退出当前函数帧，不把语言 Err/None 当 VM 执行失败。泛型更新、空 patch、更新检查失败、跨成功类型传播和嵌套闭包边界已有验证；eval/eval-with 语言资产与默认后端对比通过。真实全模块验收仍继续，不因这两项修复宣称全部语言覆盖完成。

std/test.should_ok/should_fail/should_fail_with/with_fixtures 已按 native 模块身份及封闭签名接通测试描述构造。CLI 验证包含会失败但不能在初始化执行的测试体、缺失但不能在初始化加载的 fixture、发布后的身份比较，以及空错误期望拒绝。

BlameError 采用对象身份相等，重复引用相等、独立 blame 构造不相等，即使消息和来源相同也不按内容合并；发布转发表保留该身份关系。Float remainder 已通过独立原生 helper 接通 Rust 浮点 remainder 语义，拒绝非有限结果并保留运算来源，测试覆盖负数与负零。该项补齐早期进展记录中的 Float remainder 缺口。

Regex 的相等按原始 pattern 字符串判断，不按匹配语言是否等价判断。Fmt 按节点操作和子节点结构比较，不先渲染；Float 格式节点保持 bit 相等语义，区别于普通 Float 数值相等。两者读取原生表并共享已验证的节点读取逻辑。语言资产覆盖独立构造、发布后读取、等价但不同 pattern、相同输出的不同格式树及格式正负零。HashState 按完整摘要状态与缓冲区比较，已有默认/native 对比；其余 opaque 相等契约仍继续核对。

结构化相等比较已接入 std/eq.equal 与聚合 ==/!=。运行时沿封闭布局遍历原生对象，工作列表只持有描述符，忽略来源位置；覆盖标量、String/Bytes、元数据、Array、Tuple/Record、newtype、Dict、enum。Float 使用数值相等，保留正负零相等语义。Dyn 按已分配对象身份比较，重复 pack 不视为同一对象。函数按代码身份和环境槽身份比较，重复创建的无捕获/有捕获闭包互不相等，全局及 native 函数重复引用保持相等。真实 CLI eval/eval-with 对比默认后端并覆盖 main/work 读取。opaque resource 的独立相等契约仍待补齐，当前明确报未接通，不以内容比较兜底。

std/_rt.call_with_diagnostics 使用封闭的 callback 参数/返回类型、Result/Tuple/Array 边与 TypeOf 见证生成调用包，运行时不重做推导。codec 的 enum 编码已覆盖内置 Result 等封闭 enum，支持输出捕获结果。

std/hash 的 sha256/new/update_bytes/update_string/update_int/finish 已消费封闭签名接入独立 HashState 表。Bytes 的相等/不等比较直接借用 native 字节切片，不按 HeapId 比较内容。旧运行时的 hash 实现不变。

std/path 的 join/normalize/parent/file_name 已按 native 模块身份和封闭签名接入。操作直接读取 native String/Array，采用跨平台一致的纯词法斜杠规则，不访问文件系统。语言资产覆盖空路径、根目录、连续父路径、绝对路径重置、反斜杠与 Unicode；真实 CLI eval/eval-with 对比默认后端输出并验证发布后读取。

高阶 native 回调开始接通：array.map 从封闭签名取得输入/输出元素类型，通过当前代码计划的分派器调用语言闭包；跨回调前结束 Runtime 借用，仅复制值描述符而不复制底层对象。验证包含词法捕获、嵌套 map、Unit/String 不同结果宽度、发布后读取与单次回调失败传播。模块命名空间函数引用和导入别名消费同一个 resolved SymbolId。Property provider 链尚未使用这条调用路径，不能据此视为 property 执行完成。

Native 声明现在生成同一调用 ABI 的适配函数，使用已登记 native 模块 ID 与声明 ABI 导出键链接，并检查封闭签名；导入别名和一等泛型实例不改变身份。首批为 array.length / string.length，测试包含真实标准库声明、Unicode 字符数、间接泛型参数和拒绝用户模块同名 native 声明。函数值初始化与适配器调用使用不同编译键，初始化仅生成描述符，不能以空参数调用 native 本体。其余 native 操作和 property 查询仍待补齐。

类型元数据使用封闭 TypeId 作为单 word 数据，`TypeOf(T)` 的见证在构造/发布时核对；比较直接比较 represented TypeId。函数参数和返回边界的 `TypeOf(T) -> Type` 适配只更新外层类型标记，保留 represented TypeId 与来源，不在运行时求解类型。该能力已在普通参数、隐式返回、显式 return 和发布路径验证，property 执行仍需单独接入。

以独立入口运行小型已 seal MIR/源码用例，检查标量、分支、函数、递归、聚合访问及错误路径。建立表达式支持清单；后续以 .telora 用例补齐规则。首个原型无需完整标准库可运行。

### 表达式覆盖阶段进展

Bytes 字面量现已直接 lowering 为只读字节常量与 native Bytes 表构造，复用运行时预算和来源记录。CLI 资产验证空字节串、转义、Unicode、内容相等、hash 消费及初始化发布后 entry 读取，与默认后端对照。

`dbg!` 已接通独立 native 调试事件通道，CLI 将事件写入现有 stderr JSON 格式。生成代码保留输入描述符、来源和执行顺序；formatter 直接读取 native 表，限制 8 层、32 项和 4096 字节，不调用 Display/property 或 codec。默认无 sink 时跳过格式化。CLI 对照初始化与 entry 事件的内容、位置、顺序与结果；native 测试验证长 Unicode 输出截断和 backing/来源不变。函数和 opaque 的展示采用 native 表示，不承诺复制旧 VM 的内部函数名称或 Rust Debug 输出。

Interpreter 已接通 factory/adapter codegen：从 MIR 的 witness/参数配对计划生成 Dyn 包装，未标记参数直接传递；factory 不执行 operand，adapter 调用时才按普通调用顺序求值 operand。普通 lexical captures 与局部泛型实例分别按 SymbolId/GenericInstanceId 捕获；局部模板只生成当前 MIR 已选定的具体实例，不对模板本体分配值槽或进行运行时推导。Never operand 和失败直接传播，不读取无效结果缓冲。

Interpreter 的运行时身份缓存按 factory 的函数/环境身份及 represented TypeId 序列缓存 adapter，忽略调用来源；adapter 捕获 factory 描述符与见证。发布时与导出项共用同一对象重定位表，并用重定位后的 factory 重建缓存键，保证 entry 重复调用继续命中初始化实例。缓存命中不增加堆分配，新条目先检查预算；测试覆盖来源变化、不同 factory、错误签名、发布后复用和预算拒绝。CLI eval/eval-with 覆盖初始化 adapter 复用、嵌套捕获、未使用的模板参数及 operand 执行次数。默认后端没有保留发布前 memo 表，跨初始化的 adapter 相等比较不同；计算结果另行对照，不复制这一旧后端差异。

局部泛型实例的调用也优先读取已捕获的实例描述符，避免直接调用分支重新生成闭包而丢失词法环境。语言资产覆盖普通/显式类型应用、嵌套闭包发布后调用和局部递归；计算结果与默认后端对照。

递归函数的自引用由编译计划预先依据 HIR 父关系和已解析 SymbolId 建表，函数入口绑定当前闭包描述符，作为值读取与递归调用共享同一环境，不重新加载局部声明。裸根调用没有闭包描述符时才创建无捕获入口身份。局部泛型模板跳过抽象实例，只物化 MIR 中 concrete 的实例；测试包含捕获外部值的自比较和泛型递归。

补充闭包创建点的透明包装处理：声明值经 `do` 的结果或 `ty!` 类型标注返回闭包时，自引用计划沿这些既有 HIR 边找到实际闭包。此前只识别声明的直接 Closure 子节点，导致合法局部递归报 `native closure capture plan mismatch`。语言资产 `wrapped-recursive-closure.telora` 覆盖嵌套 block、类型标注、外层与 block 内层捕获、自比较及递归调用；不执行额外名字解析或类型推导。

### 局部互递归函数槽

验收资产 `mutual-recursive-closures.telora` 最初在真实默认 CLI eval 返回 `42`，native eval 报 `native closure capture plan mismatch`，现已作为执行回归接入。资产同时导出 eval-with 入口，初始化返回的互递归闭包必须在发布后继续可调用。

原因：`function_value` 仅捕获当时出现在 `locals` 中的符号，首个函数创建时后续函数尚无描述符。之后的引用重新进入函数物化，得到不同捕获集合。SealedMir 已完成这些引用和类型的闭合，因此修复属于运行时词法绑定物化，不应改动 resolve 或重新推导。

默认实现的语义是 block 入口按稳定符号预留函数槽，声明处填入函数体；闭包捕获槽的稳定身份，因此支持互递归。native 已独立实现普通局部 def/decl 的预留、单次填充和调用解析：用保留的 `u32::MAX` code-id 标识词法槽，其 environment 为空时为 Pending，填充后保存一个完整函数描述符。只有调用时沿槽读取最终代码和环境；值比较、捕获和引用仍保留槽身份，不深复制用户对象。

Host 入口和生成代码的间接分派都处理这种槽。函数自身已捕获稳定槽时，不再用当前函数体描述符覆盖该绑定。Pending 调用、重复填充、main 槽修改以及纯 alias 环被拒绝；Pending 根不得发布。运行时测试验证互相捕获的闭包发布后保留四个 environment（两个槽、两个函数体）和原始来源/别名。初次完整回归 124 项 native 单测及三个 regex 实验通过；原有 84 项 CLI 回归通过，扩展资产另外验证发布后的 eval-with。

泛型函数实例也已接入 block 入口预留槽：从当前函数的已闭合实例引用出发，沿 `GenericInstance.references` 遍历 concrete 实例，按其稳定 Id 预留，再在定义处单次填充。只在兄弟泛型函数体中出现的实例也包含在内，不在 codegen 中做类型替换或新建实例。`decl` 只参与预留，`def` 安装函数体；递归实例已经捕获自身槽时保留该描述符。

语言资产 `generic-mutual-closures.telora` 覆盖显式声明、两组 String/Int 实例、隐式递归调用和显式类型应用，并由 CLI 验证发布后 eval-with 返回 42。该新资产在默认后端报 `function family has invalid or duplicate static instance keys`，因此泛型部分只断言 native 的确定结果，不宣称双后端对照通过；普通互递归仍保留双后端对照。泛型调整后的完整 native 测试 124 项通过；CLI 完整运行原有 84 项通过，新泛型发布用例因上述默认后端限制失败后，单独核对 native 路径。

调整对照范围后，普通/泛型互递归的 CLI eval 与 eval-with 定向回归通过。后续继续覆盖函数别名和提前调用诊断。不依赖默认 VM 的 AllocFunc/SealFunc。

槽链解析不再按别名链长度扣 fuel；host 入口与 dispatcher 共用有界遍历，最多
检查分类表中环境槽数量加一次，保留 Pending 和环错误。遍历借用 arena 内描述符，
最终只复制一次描述符。八层槽回归验证零剩余 fuel 下解析成功，但已经中止的
session 仍拒绝解析、不写结果、不重复报告。函数实际执行由调用边界扣 fuel。

源码间接调用在参数求值后、进入公共 dispatcher 前解析词法槽，解析失败使用调用 HIR 的位置。修复前 `later(42)` 的 Pending 错误标在外层 block，现精确覆盖调用范围。`function-before-initialization.telora` 由 JIT 和真实 `check --native --lib` 验证，失败只报告一次、调用深度归零、命令失败退出；参数自身 fail 的变体仍优先报告参数错误。公共 dispatcher 保留解析检查以覆盖原生回调，回调场景的定位需另行核对，不能用直接源码调用的证据代替。

函数别名按既有语义进一步校正：填充槽时，来源若是 Pending 函数槽则在定义处失败；捕获 Pending 槽仍合法，两者不可混同。真实对照发现修复前默认后端拒绝前向别名，native 却返回结果，现 `function-alias-before-initialization.telora` 验证 native 在第 2 行单次失败。函数相等比较沿槽读取最终函数身份并计费，因此已初始化函数及其别名相等；槽本身的稳定身份用于捕获连接，不是相等比较的最终依据。互递归发布资产增加已初始化别名比较，默认/native eval 和 eval-with 均返回 42。126 项 native 单测及三个 regex 实验通过，CLI 定向覆盖失败位置与发布后别名调用。

map/fold 家族的原生回调在非空输入上准备函数槽，沿槽解析和 fuel 耗尽使用回调描述符的来源（通常是用户源码中的声明），不再由公共 dispatcher 的编译根定位。准备后的函数体复用于该次遍历，不逐元素重复沿别名链查找；每次实际 dispatcher 调用仍执行验证和计费。空输入不准备、不调用回调，因此允许尚未填充的槽。`pending-map-callback.telora` 和 `pending-fold-callback.telora` 分别验证空输入返回 0、非空变体单次失败且来源对应声明、调用深度归零。这是声明来源定位，不宣称获得完整用户调用栈；其他原生回调入口仍需分别核对。

`.cast!` 已消费封闭目标类型与 checker 实例生成调用包；运行时先验证整个输入的表示，再转换并执行声明检查，不调用 encode/decode，也不重新推导类型。支持 Record/Dict、Array、Tuple/Newtype、Option/Result 和 Unchecked 到 owner；拒绝不同 nominal 身份以及数值隐式转换。未改变的子对象保留 backing 与来源。结构不匹配返回 Err(String)，checker 失败只产生一次执行诊断。

CLI 语言资产覆盖 eval/eval-with 的 19 项类型身份与转换检查，并将解包后的容器数据与默认后端比较。默认后端对 cast 容器写入类型标记、对普通容器不总是写入相同标记，其相等比较会受此影响；因此不以该差异规定 native 的相等语义。默认运行时代码保持不变。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
