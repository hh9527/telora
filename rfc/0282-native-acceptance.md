# RFC 0282 本期落地验收记录

日期：2026-09-13。实现基准：`a6e6b22`，远端分支 `feat/native-cranelift`。
本记录核对 RFC 0282–0287 的本期范围：隐藏 native check/eval/eval-with。
不包含 native test/run/serve、服务 GC、默认后端切换或 main 合入。

## 要求与证据

| 要求 | 实现与验证证据 |
| --- | --- |
| 无 VM 的静态闭合，所有执行类型及函数实例预先确定 | CLI 先 seal/seal_export/seal_modules，再调用 `jit::compile_executable`；`functions::Key::ty` 读取既有实例节点类型。static_cli 的 only-types/静态错误分支不创建 Session。批量静态错误实测 execution_seconds=0，开启 native timings 也没有执行阶段记录。 |
| 固定 ABI、来源和稳定 TypeId | `abi.rs` 的 TypeKey 直接使用 MIR index，Origin 为三个 u32；Layouts 拒绝无布局值，Never 不物化。现有 ABI/JIT 测试覆盖标量、Unit、异宽参数、来源、错误状态及间接调用。 |
| 独立分类表、main/work 边界 | `runtime.rs`、`runtime/publish.rs`；runtime 测试覆盖 Tuple/Record 共表、Array slice、Dict 有序双列、不可变更新、共享 backing、过期句柄拒绝。JIT 测试补充 enum inline/boxed payload 和 Dyn。 |
| 共享与真实环在发布后保持 | `lexical_function_slots_preserve_cycles_and_identity_across_publication`、`closure_environments_publish_nested_captures_and_shared_objects`、`native_mutual_recursive_closures_survive_initialization_and_entry`。 |
| 数据先注入，property/顶层值按需计算后主动完成 | `Session::initialize` 先注入数据，再调用 Compiled.initialize；数据、provider chain、全局/property 互依赖测试覆盖单次计算、缺失注入、重复注入和真实需求环。RFC 0286 已验收。 |
| 初始化失败不发布，不执行 entry | `publication_failure_keeps_initialize_world_and_main_unpublished` 及需求/CLI 失败测试；发布先建临时 main，全部成功后替换。CLI 遇到初始化 error 在调用 entry 和输出结果前返回。 |
| 直接机器码，无旧 VM 回退或运行时类型求解 | 生产 native 的 core 引用为 AST 操作枚举、MIR、candidate_layout、type_image、data_plan、source/DataLimits。resolver 只在 cfg(test) 的 test_support 构图。CLI 分流后不经过旧 codegen、execution_link 或 Vm。见下方依赖审计。 |
| 函数、控制流、聚合、泛型和语言诊断 | RFC 0285 表达式清单；141 项 native 单测和 402 个实际语言测试闭包审计。1500 次尾调用已通过，额外测试异宽互递归、捕获、callback、返回适配和构造检查。 |
| fuel 限制失控执行，资源错误不可吞掉 | 函数入口和 callback 使用同一 context；无限尾递归仍耗 fuel，非尾递归检查调用深度，显式 stack words 准入/退出成对。诊断捕获测试验证资源 abort 不可恢复。数组、字符串及共享图 JSON/Fmt 输出在构造/增长前准入。 |
| 单次 session 资源释放 | Runtime/Tables/CallContext 以 Rust owner 持有缓冲和资源；Compiled 的 CodeMemory::drop 调用 JITModule.free_memory，包含编译提前失败。公开调用借用 Compiled，无机器码地址向外返回；转交地址只在当前调用中使用。无静态可变资源表、forget/leak 或旧 Val 桥。 |
| check/eval/eval-with 阶段及输出正确 | 84 项 CLI 测试包含数据初始化、eval JSON、eval-with 来源/env/args、config 拒绝、初始化/执行错误、warning/debug 和来源；真实 world-model 输出逐字节等于默认后端。 |
| 批量检查与隐藏范围 | release 手动覆盖 --lib、--tests、两者合用及各自 only-types；根数分别为 2/2/4，测试范围不会初始化无关库根，选中失败根时初始化拒绝，only-types 均零执行。check/eval/eval-with 帮助不列开关；run/serve/test/query 拒绝 --native，退出 2。docs/README 不包含该隐藏开关。 |
| 默认路线保持可用 | `cargo test --workspace --all-features` 全部通过，包含默认语言验收、默认/native CLI 对照；未指定 native 仍走旧执行分支。语言层已授权的 cast/匿名 Record 清理由 RFC 0289 独立记录。 |
| release 分段时间与内存观察 | 下表与复跑命令；仅记录当前实现，不据此承诺性能收益。 |

## 最终验证命令与结果

- `cargo test --workspace --all-features`：通过。包含 354 项 core 测试、84 项 CLI
  测试、141 项 native 单测（另 1 项手动审计默认忽略）、3 项独立 regex 实验；
  其他 workspace 套件及 doc tests 也通过。
- `cargo test -p telora-native`：25 项单测、3 项独立实验通过；无 JIT 可独立构建。
- workspace CLI 验收已重新生成默认语言观察；随后运行
  `cargo test -p telora-native --features jit published_language_test_closures -- --ignored --nocapture`：
  402/402 个非 fixture 闭包的结果与默认观察一致，耗时 51.12 秒。
- 21 个 fixture case 不由该 harness 调度，不计入 native 执行覆盖；native test
  调度器不是本期交付。数据注入、来源、codec 和 eval-with 外部数据由独立 CLI 用例覆盖。
- `cargo build --release -p telora` 及 `git diff --check` 通过。

## 依赖及封闭边界审计

检查 `telora-native/Cargo.toml`、lib.rs 的 cfg(test) 边界、生产 core 引用、
CLI 分流和对象/代码所有权，而非仅以没有某个关键词作为证明。
native 直接依赖 core 的静态公开接口、serde_json、regex-automata/regex-syntax，
可选 jit 引入 Cranelift 0.135；sha2 仅作为测试向量参照。
data_plan 产生解析计划，直接物化到 native tables；输出从 native 图写 JSON，
不经过旧 Val 或 host Value 对象树。

Runtime::new 读取封闭骨架并建立只读布局/反射索引；运行时读取已有 TypeId 头部、
Dyn 见证或 codec 目标不构成类型求解，不新增 TypeId 或函数实例。
codegen 的未识别 HIR/native ABI 分支明确拒绝；没有第二后端 fallback。
字面量字段 decorator 的旧 unsupported 分支不代表本期可执行语言缺口：当前
decorator 仅适用具名类型/member，类型求解只登记这些 provider，seal 拒绝未登记
decorator。Float remainder 则已在通用 Float 算术分支之前专门 lowering。

## 最终 release 观察

机器：Linux x86_64、Intel Xeon Gold 6266C、rustc 1.98.1；64-bit little-endian。
以下每项只执行一次，未做统计或与旧阶段归因比较。

工作区 `../lab-ws/lab-ontology/world-model`；输入文件内容：

```json
{"op":"list","measures":[],"dimensions":["country_name"],"filters":[{"dimension":"country_continent","op":"eq","kind":"text","value":"Asia"}],"ordering":[],"limit":null,"offset":null,"output_order":[]}
```

```sh
TELORA_NATIVE_TIMINGS=1 /usr/bin/time -v target/release/telora \
  -C ../lab-ws/lab-ontology/world-model \
  eval-with --native @src/bin/make-query:main \
  --source input=/tmp/native-world-input.json
```

默认参照改用 `TELORA_DEFAULT_TIMINGS=1` 并去掉 `--native`。两者 stdout 经 cmp
一致：`{"bindings":["Asia"],"sql":"SELECT c.Name FROM country AS c WHERE c.Continent = ?"}`。

| 观察项 | 默认 bytecode | native |
| --- | ---: | ---: |
| frontend | 443.58 ms | 511.20 ms |
| codegen | 17.82 ms | 2737.83 ms |
| initialize | 12.15 ms | 4.06 ms |
| entry execute | 1.05 ms | 0.73 ms |
| 总墙钟 | 0.49 s | 3.28 s |
| 峰值 RSS | 46144 KiB | 62800 KiB |

native 另有 runtime_setup 3.54 ms、entry_input 0.14 ms、output 0.03 ms。
默认 link 0.21 ms、entry_input 0.20 ms；计时边界沿用 RFC 0287 的定义。

同工作区 `check --native --lib`：3.79 s，61964 KiB；静态 516.21 ms，执行阶段
3262.04 ms。`check --only-types --lib`：0.45 s，45680 KiB，execution_seconds=0。
两者根均为 model 和 bin/make-query，Unknown/Conflicted/unproven_bounds 全部为零。

## 明确限制

本期只验收当前 Linux x86_64 host，不承诺跨 target ABI、AOT 或沙箱。
分配预算是逻辑累计量，不是 RSS 上限；JIT 内存、allocator 预留容量、regex 编译
临时峰值和部分 Rust scratch 不是完整物理计量。显式栈预算也不等于机器栈字节数。
fuel 按既定定位约束失控执行，不是精确成本或墙钟计费。
这些限制保留，不用降低测试次数、吞错、兼容推导或旧 VM 回退掩盖。

本期 check/eval/eval-with 路线完成；服务运行、GC、性能优化及默认切换另行推进。
