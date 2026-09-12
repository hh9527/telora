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

Native session 消费 `SealedExecutable`：check 以选定模块集合为根，eval/eval-with 以选定导出为根裁剪无关普通值；property/check 元数据仍保留 session 初始化范围。通过与 linker 无关的 catalog 读数据接口解析、注入数据模块，再完成初始化和发布。eval 在执行前验证导出的权威 std/value.Value 身份，成功后直接从 native 对象输出 JSON。初始化失败保留位置且不重复附加通用失败诊断；数据解析保留原结构化诊断。隐藏开关不进入普通帮助或用户文档。

已验证真实 CLI 的泛型闭包初始化、--only-types 零执行、模块 check 的顶层 fail 阻止初始化、单次来源诊断，以及数据模块 check 和 eval JSON 输出。eval 的无关顶层值不执行。完整 std/value 依赖图包含的 nullary enum 比较按封闭布局翻译为 tag 比较。

eval-with 在执行前验证权威 std/entry.Eval 身份，并从封闭骨架读取 config/evaluate/Context 的字段与类型。全图初始化发布成功后，校验唯一非空来源/环境变量名称、准确匹配来源清单以及 args 许可，再将声明的输入直接构造为 native Context，调用发布后的 evaluate 闭包。集成测试覆盖 JSON 数据模块、外部 JSON 来源、环境变量、Unicode 参数、配置拒绝和执行失败来源诊断；真实 `check --native std/entry` 已通过。

基础 codec 解码已接入封闭目标类型，支持标量、Option、Array、Tuple/Unit、结构记录、Dict，以及名义 record/newtype/enum（含递归类型和泛型实例）；字段缺失/多余或值不匹配返回携带来源的 `Err(BlameError)`，仅在用户调用 `raise!` 时转成执行诊断。字符串等叶子复用原对象描述符，不经过旧 VM 或 host Value 树。NewtypeTable 使用独立槽位表，发布时保留对象与其 payload 的共享关系。

直接构造及 codec 解码中的 struct/newtype/payload variant checker 已接入普通函数调用；checker 闭包工厂作为初始化项求值一次并发布，泛型 checker 消费 MIR 实例。解码中的检查拒绝返回 Result，执行失败则中止解码。eval-with 已验证外部输入经已发布 checker 检查，拒绝来源保留到输入元素。

codec 的 rename_all/untagged 已读取真实 property 值并参与编码/解码，支持与构造检查、嵌套集合组合；歧义和无匹配为可捕获的解码拒绝，provider/checker 执行失败直接传播。文本转换也已接入 ParseBy/DisplayBy，string.parse 与 codec 共享基于捕获范围的解析器；编码调用原生 property 内的普通 display 闭包。eval-with 增加了发布后解析/格式化外部文本输入的往返验证。

JSON/YAML/TOML 字符串解析直接物化到 native tables，保留输入来源并执行数据限制。JSON 紧凑/pretty 输出读取 native 图，schema_with 消费封闭类型骨架及真实 property 值。完整 std/json 模块已接入初始化，CLI 验证包含 schema 的默认/native 输出对比及发布后文本桥接类型的 schema 查询。

仍未完成完整语法/native API 覆盖及配额语义，因此不据此宣称整体路线落地。

### 全量语言模块初始化复查（2026-09-13）

实现版本 `4bf46dd`，debug CLI；使用默认语言测试脚本生成的
`target/language-tests/workspace`，逐一执行
`check --native @src/test/<name>/testee`。源码中的 76 个 testee 全部完成检查：

- 71 个成功完成编译和初始化。
- `syntax` 为预期语法错误；`empty-expectation` 为预期的空测试错误文本拒绝；
  `initialization` 为源码显式顶层 fail，报告 `test export initialization failure`。
- `nominal-equality` 和 `enum-constructor-context` 仍失败：同一个泛型参数
  接收已物化的匿名 Record 与名义 Item（含 Array/Option 嵌套），MIR 的边界类型
  与实际值身份不一致。共享参数是否要求相同 TypeId 的规则待确认，未以运行时
  重新标记身份掩盖问题。

这批 check 会编译测试闭包，但不运行测试调度器和闭包内容，不能等同于 71 个
模块的全部运行测试通过。另实际执行了 eval/interpolation 的 native eval，
嵌套插值、整数/浮点及自定义 Display 输出符合现有资产预期。
默认完整语言运行测试由上一提交的 85 项 CLI 验收覆盖。

### 分阶段测量入口与真实负载观察（2026-09-12）

内部环境变量 `TELORA_NATIVE_TIMINGS=1` 显式开启阶段计时，逐行输出 JSON 到 stderr，不进入普通帮助/用户文档，也不改变 stdout 的结果。记录是墙钟时间，失败退出也可产生已进入阶段的记录，不代表该阶段成功。eval/eval-with 分开记录 frontend（清单、求解、seal 和入口契约检查）、codegen、runtime_setup、initialize（数据模块注入、全图顶层/property 求值及发布）、entry_input、execute 和 output；eval 无 entry 调用，记录 export 而非 execute。check 经过共享 Session 可记录 codegen/runtime_setup/initialize，但没有 frontend 记录。

阶段记录不包含全部进程开销，例如 CLI 参数处理、部分编译准备和退出释放；不能把阶段之和当成完整耗时。峰值内存仍用 `/usr/bin/time` 记录整进程 RSS，未将其错误归属于某个阶段。

观测构建：`28f097d` 加本次计时改动，`cargo build --release -p telora`；Rust 1.98.1，x86_64 Linux，Intel Xeon Gold 6266C。lab-ontology 版本 `4d8915c25d809a9f687c1dfad554454ecc65166f`。输入是查询 country_name、country_continent 等于 Asia、空 measures/ordering、null limit/offset 的 JSON 请求：

```sh
TELORA_NATIVE_TIMINGS=1 /usr/bin/time -f 'elapsed_s=%e peak_rss_kib=%M' \
  target/release/telora -C ../lab-ws/lab-ontology/world-model \
  eval-with --native @src/bin/make-query:main \
  --source input=/tmp/native-world-input.json
```

输入完整内容：

```json
{"op":"list","measures":[],"dimensions":["country_name"],"filters":[{"dimension":"country_continent","op":"eq","kind":"text","value":"Asia"}],"ordering":[],"limit":null,"offset":null,"output_order":[]}
```

| 阶段 | 本次墙钟毫秒 |
|---|---:|
| frontend | 456.103 |
| codegen | 3571.159 |
| runtime_setup | 3.436 |
| initialize | 4.130 |
| entry_input | 0.112 |
| execute | 0.635 |
| output | 0.029 |

native 整进程 4.05 秒、峰值 70,400 KiB；同一 release 二进制默认后端（去掉环境变量和 `--native`）0.41 秒、45,900 KiB。两者 stdout 经 `cmp` 完全一致。这是各一次观测，不是统计基准，不据此宣布性能改进或完成 M3。计时集成回归复用现有 eval-with 用例，验证阶段顺序、数值字段及 stdout JSON 保持正确。

验证默认路径不变、隐藏帮助、显式 unsupported、only-types 零执行、各命令停止阶段正确。先少量冒烟，再补完整 corner cases；完成总装后才测编译/初始化/执行耗时和峰值内存，不承诺性能收益。

### 默认后端分阶段参照（2026-09-12）

按用户要求补充原路径参照。`TELORA_DEFAULT_TIMINGS=1` 内部入口向 stderr 输出成功走过的阶段；未设置时不读取时钟、不输出计时，不进入公开帮助/用户文档。eval/eval-with 的前端、codegen、link 分开记录，后续 initialize/entry_input/execute 目前只在 eval-with 记录。它不改变执行结果或阶段顺序。

构建为 `e32e20f` 加本次默认路径计时改动，执行 `cargo build --release -p telora`；机器、Rust 版本、lab-ontology 版本和完整输入与上节相同。先运行默认后端，再运行 native，没有同时启动两个负载：

```sh
TELORA_DEFAULT_TIMINGS=1 /usr/bin/time -f 'elapsed_s=%e peak_rss_kib=%M' \
  target/release/telora -C ../lab-ws/lab-ontology/world-model \
  eval-with @src/bin/make-query:main --source input=/tmp/native-world-input.json

TELORA_NATIVE_TIMINGS=1 /usr/bin/time -f 'elapsed_s=%e peak_rss_kib=%M' \
  target/release/telora -C ../lab-ws/lab-ontology/world-model \
  eval-with --native @src/bin/make-query:main --source input=/tmp/native-world-input.json
```

| 阶段 | 默认路径（毫秒） | native（毫秒） |
|---|---:|---:|
| frontend | 354.056 | 350.472 |
| codegen | 17.641 | 3750.783 |
| link | 0.212 | 未单列 |
| runtime_setup | 包含于 initialize | 3.347 |
| initialize | 11.980 | 4.476 |
| entry_input | 0.215 | 0.101 |
| execute | 1.166 | 0.690 |
| output | 未单列 | 0.027 |
| 整进程（秒） | 0.40 | 4.13 |
| 峰值 RSS（KiB） | 46,372 | 71,108 |

codegen 分别是字节码生成和 Cranelift 机器码生成，包含各自编译准备。默认 initialize 包含 main/account 创建、数据物化、初始化执行及发布；native 将 runtime_setup 单列。默认数据文件读取在 link 中，native 在 initialize 内完成，因此两者 initialize 不是逐操作完全相同的边界。entry_input 均包含 VM 中的 Context 构造，但外部文件读取在 CLI 中的位置不同；这里只作阶段分布参照，不据此得出普遍性能结论。

两条路径 stdout 经 cmp 完全一致；再次关闭默认计时运行，stdout 仍一致、stderr 为零字节。以上各一次性能观测，未做统计分布，也未借此修改优化策略。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
