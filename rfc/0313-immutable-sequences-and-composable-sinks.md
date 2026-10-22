# RFC 0313：不可变惰性序列与可组合消费者

- 状态：实现完成，最终验收通过；实现 PR #239
- 跟踪：[#237](https://github.com/hh9527/telora/issues/237)
- 实现分支：`feat/237-immutable-seq-sink`；RFC 已由 #238 合入
- 日期：2026-10-22
- 前置：RFC 0312 的准确根、执行期间 GC 与 fuel 停机

## 动机与范围

icloud_model 初始化累计 managed 分配约 145 MiB，最终保留约 3.16 MiB。
这些数值是同一真实模型的既有诊断数据，不是本 RFC 的性能承诺。
目前 array::map/filter/flat_map 物化集合；filter 按输入长度预分配，
length(filter(xs, p)) 即使只要求计数也会分配输入长度的缓冲区。
多次筛选、投影和消费者会增加临时分配及其根发布、地址转换和回收成本。

目标是在用户无法感知 mutable 的前提下，提供显式的逐元素流水线，
并允许一次遍历计算多个答案。已有 Array 算子的执行语义保持不变。
本次不自动重排已有高阶调用，不增加用户可见 move 或所有权失效规则。

本 RFC 不承诺消除重复扫描、存在性检查中的完整 brief 重建，或
fold 中逐项 dict::merge 的累计复制。它们需要应用索引和批量构建设计。
lab-ontology 的应用迁移另行交付，以本 RFC 的基准结果决定迁移范围。

## 调查依据

- `crates/telora-core/modules/std/array.telora`：集合操作为受信任 native 声明，
  已有 fold_control、any/all/find，可参照其提前停止语义。
- `crates/telora-wasm/src/array_ops.rs`：map/filter 生成物化循环，
  packed 输出在循环前按 count 分配。fold 可直接生成消费循环。
- `crates/telora-wasm/src/array_build.rs`：flat_map 先保留子数组以规划总长度，
  再分配并复制；push 已有尾部追加和容量倍增，不应一概判断为逐项全量复制。
- `crates/telora-wasm/src/dict_build.rs`：merge 分配两侧长度之和的键值缓冲，
  Seq 本身不能使该操作变为原地更新。
- `crates/telora-core/src/static_sources.rs` 与 Wasm `natives.rs`：
  native 由准入模块身份激活，用户同名函数不能触发特权 lowering。
- lab-ontology `ontology/src/ontology.telora` 的 build_root、
  check_measure_contract、knowledge_target_declared、dataset_brief，
  以及 `knowledge.telora` 的 index_from_model 是应用侧对照目标。

## 用户可见语义

### 不可变序列

Seq(T) 是有限输入与变换的不可变描述。构造操作先按现有求值顺序取得
源值、回调及其捕获；遍历回调延迟到终结操作执行。
不暴露游标、next/reset、已消费状态、缓存填充状态或消费后失效。

```telora
let s = seq::from_array(items);
let s2 = seq::filter_map(s, f1);
let s3 = seq::filter_map(s2, f2);
let s4 = seq::collect(s3);
let s5 = seq::collect(s2);
```

两次终结操作各自从头求值，互不影响；f1 会执行两遍。
默认不记忆结果。需要共享结果时显式 collect 后复用 Array。
同一描述在相同环境和足够资源下重复求值得到同值，回调中的诊断也会重新执行。
含函数值的相等性遵循既有函数身份规则，不推导序列的外延相等性。

### 多答案求值

以下是拟议 API 示例，实际类型签名须通过下述表示原型门槛再冻结：

```telora
let result = seq::compute(
    seq::filter_map(seq::from_array(items), f1),
    sink::both(
        sink::filter_map(f2, sink::collect()),
        sink::collect(),
    ),
);
let s4 = result.0;
let s5 = result.1;
```

both 是有序二叉组合，可嵌套得到多个答案；首版不要求任意 arity tuple
自动变为消费者。公共前缀 f1 对每个输入最多执行一次，输出值传给两个分支。
分支可以拥有自己的 map/filter/filter_map，不会反向改变源或另一分支。
collect 结果仍需按实际输出分配；不承诺消除业务回调内部的对象构造。

### 顺序、失败与停止

1. 单个输入依次经过流水线，完成后才处理下一输入。
2. both 按左分支、右分支顺序处理当前元素；嵌套组合按深度优先从左到右执行。
3. 失败立即停止整个求值，不再执行当前元素的后续分支或后续输入。
   已产生诊断保持现有传播规则，不伪造完整结果。
4. find/any/all 达到答案后，该分支停止调用回调并冻结答案；其他分支继续。
   所有分支完成时整个输入遍历停止。collect/count/fold 通常需要完整输入。
5. 在开始拉取输入前检查分支是否全部完成；终结阶段按左到右产生最终结果。
6. 初始化状态每次 compute 独立建立，不在源或 Sink 描述中回写可变状态。

因此显式流水线与已有 Array 操作链的诊断顺序可以不同。应用迁移必须核对
失败与诊断行为，不能仅凭值相等宣称保持所有行为。

## 拟议首版 API

| 模块 | 操作 | 约束 |
| --- | --- | --- |
| seq | from_array | 唯一必需输入源，复用 Array 存储，不深复制 |
| seq | map / filter / filter_map | 延迟逐元素处理；filter_map 回调返回 Option |
| seq | compute | 运行一个消费者，含 both 组合 |
| seq | collect / fold / count / any / all / find | 常见单消费者终结便捷入口 |
| sink | collect / fold / count / any / all / find | 构造不可变消费者描述 |
| sink | map / filter / filter_map | 包装下游消费者，得到分支处理链 |
| sink | both | 组合两个同输入消费者，输出有序 pair |

fold 的初值按普通参数规则求值，内部更新不得修改调用者保留的旧值。
count 及尺寸增长遵循现有整数和容量边界，不能通过溢出绕过资源上限。
sum 可以由 fold 表达，首版不新增通用数值 trait。
range、flat_map、字典分组消费者、用户自定义底层 Sink 协议后续扩展。
flat_map 不纳入首版，避免将嵌套输入停止传播和保活留成隐含问题。

## 编译与运行设计

### 表示原型是实施门槛

公开的 Seq(T)/Sink(A, R) 暂为语义记号，不据此直接添加两个内建类型。
现有封闭类型、泛型普通 struct 与函数支持需要用小原型验证以下方案：

- 优先：用 std 中普通泛型结构记录源、变换和消费者，具体 plan 类型保留
  到封闭 MIR；少量受信任消费原语将静态 plan 降低为循环。
- 若隐藏 plan 类型需要现有语言不支持的存在类型或高阶多态，不将复杂类型
  系统扩展暗中加入本 RFC。改为显式携带 plan 参数的泛型类型，或提出有限
  专用类型表示；在本 RFC 修订中记录实际签名及逃逸边界后再实施完整 API。
- 普通闭包组合可作为语义参照，但不能以“结果正确”代替融合性能验收。

原型必须覆盖局部复用、both 异构状态、函数参数/返回和条件选择；明确哪些
情形可静态融合，哪些需要合法的通用执行路径。不得在融合失败时偷偷物化
中间数组。若首版需要限制准入，提供明确编译诊断并在此记录，不能静默退化。

### 循环与状态

针对静态 plan 生成一个输入循环，所有分支共用前缀计算。
单次求值的游标、完成标志、累积状态属于执行局部状态；每次求值独立初始化。
结构性 map/filter/filter_map/both 不得引入随输入长度增长的中间集合，
也不得仅因框架包装而每元素分配一份 managed plan/tuple/Option 对象。
业务回调返回的 Option/对象是否可消除取决于既有表示，须单独报告，不能混淆。
不要求首版消除所有间接回调调用；是否内联由证据决定。

std 承载可组合接口；编译器提供必要的受信任 lowering，所有执行生成 Guest Wasm。
不引入 Host 回调执行用户流水线，也不增加新的不计 fuel 的工作路径。
优化按准入身份识别，别名导入保持身份，用户同名函数不激活特殊 lowering。

### GC、内存和持久性

源 Array、回调捕获、消费者初值按普通值保活，整个生命周期不得保存未追踪
的逻辑地址或跨 safepoint 保存失效物理地址。构造前缀、当前元素、分支状态、
部分收集结果及已完成分支答案均纳入准确根。
收集器必须保留有限输出前缀，框架临时空间受 Guest 限额约束；fuel 耗尽与
内存失败沿用 RFC 0312 的终止及禁止重入规则。

默认不实现隐式缓存，也不依赖 ref count 或完整 aliasing 分析。
返回和捕获 plan 的支持取决于表示门槛；若形成普通可持有值，应按其字段正常
追踪。Dyn/codec/反射、service snapshot 和边界压缩必须逐项声明准入或拒绝，
未验收前不宣称支持。内部 ABI 若变化须同步版本与制品校验。

## 实施计划与交付

1. 独立提交本 RFC，评审语义与验收条件；在 #237 记录阶段进展。
2. 在隔离环境做表示原型，冻结可编译签名、逃逸支持及 lowering 方案，修订 RFC。
3. 实现 Array 源、线性变换及基本终结操作，先验收无中间集合和重复求值。
4. 实现 both 与分支适配器，验收共享前缀、独立停止及失败顺序。
5. 补齐 GC/fuel/持久性边界，更新 LANGUAGE、IMPLEMENTATION 与指南。
6. 完成完整测试；以隔离的 ic-model 迁移实验衡量收益，再提出应用交付。

阶段内选择性运行相关用例，功能完整后恢复全量；构建和实验产物在 /tmp，
不替换 lab-ontology 正在使用的二进制或文件。分支推送和 issue 阶段汇报
遵守标准流程；提交按当前交付授权执行，合入 main 必须另行授权。

## 可执行验收条件

- 语义：空输入、全部丢弃、类型变化、捕获源值、重复求值、分叉分别求值；
  显式物化可复用，原 Array 与消费者初值不因求值而变化。
- 多答案：both 的值等于同顺序参考 fold 的结果；公共前缀调用次数等于
  实际访问的输入数，而不是乘以分支数。使用诊断观察顺序，不给用户增加可变计数器。
- 停止：空输入、find/any/all 单分支和双分支不同结束位置；已完成分支不再
  调用回调；所有分支停止后公共前缀也停止；失败截断顺序准确。
- 类型：异构消费者状态、嵌套 both、别名导入、同名用户函数、函数传参与返回、
  条件 plan；不支持的边界产生稳定诊断而不是错误 Wasm。
- 分配：对递增输入尺寸比较 eager 链、Seq 单消费者、both 与两次独立消费。
  标量终结测得框架 managed 分配不随元素数线性增长；collect 仅要求
  输出相关存储增长，单独量化回调分配、plan 分配和 scratch 峰值。
- GC：每安全点强制 GC，覆盖源存活、捕获、部分输出、两分支当前状态和
  已停止分支持有 String/Array/Dict 的答案；结果与正常 GC 一致。
- 停机：流水线和消费者都能耗尽 fuel 停止；输出增长触及内存上限安全终止；
  trap 后重入拒绝及服务恢复保持已有协议。
- 集成：初步阶段运行相关 core/Wasm/语言用例；最终 workspace、语言 fixtures、
  build/run 与服务 smoke、格式和规模检查；既有失败如实归档。
- 真实模型：固定源码、编译参数和输出，分别记录基线、仅流水线迁移、
  应用索引/构建策略调整的初始化耗时、fuel、累计分配、内存峰值、GC 次数。
  不将 fuel 采样占比冒充 CPU 时间，不预设收益阈值或保证恢复旧版耗时。

## 备选与延期

- 自动融合已有 Array 链：可能改变诊断和失败顺序；延后，显式 Seq 先建立语义。
- 一次性可变迭代器：违反不可观察消费状态的要求，拒绝。
- 默认 memoization：隐式保留结果且改变诊断次数，拒绝；显式 collect 共享结果。
- 自动合并分散终结调用：需要跨使用点调度，可能改变失败时机，延后。
- 完整 move/aliasing 系统：本能力不依赖它；原地复用和字典构建另行设计。
- 只用闭包包装而不检查分配：无法满足本 RFC 的性能目的，不作为完成条件。

## 2026-10-22：表示原型结果

首个类型原型通过：Seq(A, P) 与 Sink(A, R, P) 为普通泛型 struct，
P 是具体嵌套 plan；source 与回调均为普通字段。from_array 的 P 为 Array(A)，
map/filter/filter_map 以对应泛型 plan 包装前一层。both 以普通 Both plan
保存左右 Sink；compute 是准入 std/seq 模块中唯一的 native 消费入口。
模块编号暂分配 36（seq）和 37（sink），通过准入身份识别。

`ordinary_generic_plans_preserve_types_through_functions_and_branches` 已验证
局部复用、泛型函数传参与返回、同一封闭类型的条件选择、filter_map 与
异构 collect/count both 可解析并 seal。该阶段尚未生成或执行 compute Wasm，
不据此宣称性能、GC、不同结构的条件分支或持久性已经验收。
不同 plan 结构有不同类型；首版不引入将任意不同结构擦除为相同 Seq(T) 的机制。

## 2026-10-22：单循环与资源边界阶段

已实现静态 plan 解码及 compute 的单输入循环。both 共用公共前缀，左右分支
按序更新；完成分支跳过自身适配器。collect 使用按输出容量倍增的私有缓冲，
count/any/all 使用局部标量状态，find 保存独立答案而不是借用可复用 scratch。
fold 的旧初值保持普通值语义。手工构造的 phantom 类型不匹配及冒名 plan
在生成 Wasm 前拒绝，遍历深度上限为 128。

针对性 11 项测试通过（日志 `/tmp/rfc0313-stage2-complete.log`），覆盖全部
首版操作、泛型传递/条件选择、嵌套异构 both、共享前缀诊断次数和顺序、
分支停止、立即失败、复用初值、Dyn 投影/反射/可编码源的 codec 往返、
强制 GC、service 初始化压缩与 snapshot/reset、fuel 与增长输出预算终止。

分配对照扣除输入装载，输入 16/256/1024 个 Int：

| 路径 | 16 | 256 | 1024 |
| --- | ---: | ---: | ---: |
| Seq map/filter + both(count, fold) | 888 B | 888 B | 888 B |
| eager map/filter + length/fold | 768 B | 8448 B | 33024 B |

此表只证明该标量流水线的框架分配不随元素数增长，不代表真实模型收益或
所有回调零分配。完整回归、用户指南与真实模型对照尚未完成。

强制初始化暴露并修复空对象追踪问题：空 payload 可以位于对象末尾之外，
其 header 需要保活，但零字节 payload 不应作为 managed 存储根交给 sweep。
该修复不改变布局或 ABI；带捕获 plan 的强制 GC service 用例覆盖此路径。


## 2026-10-22：集成与真实模型验收

首版 API 已冻结为上述普通 `Seq(A,P)` / `Sink(A,R,P)` 表示，公开签名见
`std/seq.telora` 与 `std/sink.telora`。LANGUAGE、IMPLEMENTATION 与指南已同步。
新增六项语言用例通过；release build、build/run smoke、HTTP 与 Unix socket
服务 smoke 通过。原始模型与两个隔离迁移版本的八个实际请求输出全部相同，
原 lab-ontology 源码与运行二进制保持原状。

同一 release 编译器的单次初始化对照：原应用 68.66 秒，Seq 迁移 66.68 秒，
额外应用调整 67.31 秒。Seq 累计分配下降 9.96%，fuel 下降 4.49%，Guest 峰值
由 32.25 MiB 降到 24.44 MiB；单次耗时不能承诺稳定加速。额外应用调整未见
明显增益。这一批迁移不足以大幅降低初始化成本，应用迁移仍作为独立交付。
完整参数、数据与可复现脚本见 [对照实验](0313/README.md)。

补充分配验收：16/256/1024 项的单 count、both(count,fold)、分别 count/fold
分别为固定 480/888/696 B；eager 为 768/8448/33024 B。调用后的保留 scratch
容量分别为固定 336/304/672/336 B。该指标是包含缓冲和元数据的高水位容量
上界；不宣称独立测得瞬时 live scratch。标量回调不构造容器，分配表含 plan
及结果固定开销，未逐节点计费。both 的固定构造成本可高于分别消费。

最终 workspace 全量验收通过：572 项测试、0 失败，含 CLI 95、core 207、
Wasm 133；语言汇总覆盖 441 个 fixture，全部通过。完整命令为
`CARGO_TARGET_DIR=/tmp/telora-rfc0312-target CARGO_BUILD_JOBS=4 cargo test --workspace --no-fail-fast --offline`，
日志 `/tmp/rfc0313-workspace.log`。补充的单消费者/分别消费分配测试独立通过
（`/tmp/rfc0313-allocation-final.log`）；它仅扩展测试和 fixture，未改动生产实现。
格式、diff 与源文件规模
检查通过（已有 generics.rs 软提示保留）；telora-wasm clippy 完成，既有警告
如实保留，新增 sequence 文件未产生警告。


验收归档：release 构建 `/tmp/rfc0313-release.log`；build/run 与服务 smoke
`/tmp/rfc0313-build-run-smoke.log`、`/tmp/rfc0313-http-smoke.log`；
三制品八请求输出重验 `/tmp/rfc0313-check-repro.log`。复现 probe 独立 release
构建通过，prepare 脚本的两份差异补丁与实测完全相同。实验数据见版本化的
`rfc/0313/results.json`；临时日志仅为本次本机验收记录。
