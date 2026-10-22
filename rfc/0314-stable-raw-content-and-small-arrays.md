# RFC 0314：稳定 Raw 内容存储与空数组、单元素数组特化

- 状态：RFC 已经 #243 合入；实现与验收完成，实现 PR #244 待合入评审
- 跟踪：[#242](https://github.com/hh9527/telora/issues/242)
- 日期：2026-10-22
- 前置：RFC 0245、0304、0312；与 RFC 0313 的序列消费者兼容

## 动机与范围

动态执行需要长期健壮的 GC，并要求内存受限、CPU 可通过 fuel 停机。
临时小集合和字符串应减少底层分配；字符串内容回收应避免搬移全部活内容、
逐个修补切片。用户态继续只能观察不可变值，来源不改变值相等性。

本 RFC 将两项相关的存储变更放在同一设计中：String/Bytes 的稳定 Raw 存储，
以及 Array 的 Empty/Single 特化。实施可分阶段，但必须分别报告收益和成本。
不引入用户态 mutable、引用计数、move、隐式 memoization 或编译器自动流水线融合。
不承诺这些表示变更能单独大幅降低 ic-model 初始化时间。

## 当前实现调查

调查基线为 `6b48a33b`；codec 临时路径根修复 #240 / #241 独立交付。

| 路径 | 当前事实与约束 |
| --- | --- |
| `crates/telora-core/src/candidate_layout.rs` | 完整值有 8 字节来源头；Array 完整值 24 字节，Int 16 字节，String/Bytes 24 字节；元素按完整值 packed 存储 |
| `crates/telora-wasm-shared/src/arena/content.rs` | 内容小于 16 字节时内联；长内容为共享 Vec 中的 start/end/raw_start；小切片已复制内联 |
| `crates/telora-wasm/rt/content.rs` | GC 汇总活内容范围、复制、修补每个活描述符；内容 Vec 不是独立 RawString 对象堆 |
| `crates/telora-wasm/src/aggregates.rs` | Array 头是 16 字节的 element_type/data/len/cap；元素缓冲单独分配 |
| `crates/telora-wasm/src/array_ops.rs` | array_parts 假定每个数组均有对象头；map/filter 在循环前分配输出缓冲 |
| `crates/telora-wasm/src/array_build.rs` | push 比较 view.end 与 raw.len；尾部可增长并更新共享头，历史切片复制；扩容更换缓冲而非头 |
| `crates/telora-wasm/rt/heap.rs` | 非移动 GC 保持逻辑引用；底层 Vec 可能搬移，Wasm 物理地址不能跨分配持有 |
| `crates/telora-wasm/rt/gc_mark.rs`、`crates/telora-wasm-shared/src/trace.rs` | 数组扫描有效 raw 长度；Dict 两列也走 ARRAYS 对象边，不能直接改为新 Array 值解释 |

上述尺寸说明不能直接把 `Single { value: T }` 当作免费的通用内联布局。
任意完整 T 加标签会扩大固定宽度 Array，递归类型还需要明确的间接边。

## 用户可见语义

表示差异不进入用户可见类型。相同元素序列的数组在相等性、索引、顺序、
序列消费、序列化和诊断上保持既有行为；来源仍按当前规则传播。
String 的 `<16` 门槛按 UTF-8 字节数判断，切片边界仍须符合现有字符串规则。
Bytes 使用同一字节存储规则。

持有旧值后，从同一旧值分别追加得到两个新值，三个值必须独立可读。
仅仅存在多个别名不禁止尾部追加：只要没有改写任何已发布范围即可。
第一次追加推进 raw.len，随后从旧 end 再追加时必须分叉。
用户观察不到 len/cap、缓存、存储身份或内部追加状态。

## String/Bytes 的目标表示

保留 16 字节 payload 与 24 字节完整值：

```text
Inline: bytes[15], length:u8 (0..15)
Slice:  raw:u32, start:u32, end:u32, reserved[3]=0, tag:u8=16

raw -> { len:u32, cap:u32, bytes:[u8;cap] }
```

raw 为一个稳定的受管逻辑引用，指向连续头与字节区；不增加逐对象 ID 查表。
start/end 是相对字节区的偏移。分配尺寸按 `8 + cap` 检查溢出并计入配额，
对齐和 allocator 元数据也纳入真实占用。len 只描述已初始化字节。

GC 标记 raw 分配一次，不扫描字节，不搬移它，也不修补各 Slice。
仍需扫描根、局部变量、聚合字段和临时描述符，找到它们指向的 raw。
来源追踪属于独立的现有边，不能因内容内联而跳过。

切片短于 16 字节时复制内联，解除对 raw 的引用。其他切片保活整个分配，
不在首版引入按保留比例启发的隐式复制。必须测试大 raw 被中等切片保活的成本。

### 追加、容量和冻结

- 仅允许在 view.end == raw.len、容量足够且对象可写时追加；结果保留原 start，
  更新 end 与 raw.len，旧描述符保持不变。前缀外历史字节不得改写。
- 容量不足时分配新 raw，复制当前 view 并追加；不得 realloc 一个仍被引用的
  连续 raw，也不得通过旧引用写入新容量。容量采用有界几何增长并检查乘加溢出。
- 冻结初始化图中的 raw 在请求阶段只读。即使有剩余容量，也要分叉，防止
  请求间共享状态、恢复快照差异或反复请求造成常驻容量增长。
- 物理指针只在无分配、无 safepoint 的短暂借用中使用。源来自受管堆时，先分配
  目标再重新解析源逻辑引用，不能跨分配保存 Vec 指针。

## Array 的目标表示

首版保持完整 Array 的固定 24 字节宽度，使用现有 16 字节 payload：

```text
payload = { reference:u32, start:u32, end:u32, kind:u32 }

kind=0 Empty:  reference=0, start=0, end=0
kind=1 Single: reference 指向一个独立的完整 T 槽，start=0, end=1
kind=2 Slice:  reference 指向 RawArray<T>，0 <= start <= end <= raw.len
```

Empty 与 Single 是存储特化，业务仍是 Array(T)。新发布的数组按实际长度
规范化：0 为 Empty，1 为 Single，至少 2 为 Slice。可变构建器内部允许暂存
0/1 个元素，不把未完成的内部状态当作发布值。

### Empty

不分配 RawArray 头或元素缓冲，GC 没有数组存储边。完整 Array 值自身仍可能
位于局部帧或其他受管对象中；“零分配”仅指底层数组存储。
uninhabited 元素类型只能构造 Empty，不能误读不存在的元素布局。

### Single

首版选择间接 Single：独立完整 T 槽带来源，省去 16 字节 RawArray 头，
不扩大 Array 描述符、不递归展开 T，也不要求引用计数或 aliasing 分析。
已有独立、生命周期合适的元素槽可以复用；scratch/栈内元素不能直接逃逸。

从大数组取一个元素构造 Single 时复制完整 T 到独立槽，不把原缓冲内部地址
作为 Single 的长期引用，否则仍会保活整个缓冲。复制是浅复制，T 的子引用正常保活。
不是每次复制 Array 描述符都复制元素：描述符复制共享 Single 槽。

GC 的 Single 边保活槽，并按 T 的准确布局扫描；即使 T 不含 GC 引用，
槽本身也必须保活。嵌套 struct/tuple/enum、递归类型和来源边均遵守既有规则。

直接内联 Single 是原型对照方案：比较只内联有限 payload、扩大固定描述符、
以及当前间接 Single。须包含来源头和递归边成本，不能只算元素数据字节。
若原型证实直接内联更好，先修订本 RFC 的编码与验收再实施；首版不默认扩大布局。

### Slice 与追加

沿用稳定逻辑 RawArray 头和可替换元素缓冲，头包含 element_type/data/len/cap。
String 的连续 Raw 布局不强制推广给 Array：数组已有共享头扩容机制，另行统一
不能作为本 RFC 的隐含改动。冻结图中的头和缓冲都不可在请求中修改。

Empty push 产生 Single，Single push 产生 Slice；共享 Single 槽不被改写。
Slice 的尾部追加沿用 end == raw.len 检查；第一次追加推进 raw.len，历史视图
后续追加分叉。需要修正带非零 start 的视图增长计算：raw 的写入位置/新长度
与 view 的元素数分别计算，不能拿 count+1 替代 end+1。

第一版 GC 扫描整个 RawArray 的有效前缀，不扫描未初始化容量；小切片可能
保活其他有效元素引用。Empty/Single 的规范化降低此问题，但不等同区间 GC。

## 生产路径与统一遍历

新表示必须覆盖 literal、push、map/filter、flat_map/concat、zip/enumerate、
Seq collect/Sink collect、codec/parser 和 transport 输入，而不只修改 array_result。
统一长度和元素访问辅助接口，Empty 不解引用，Single 不解释成 RawArray 头。
匹配数未知的构建器在首个结果前不分配输出数组存储，首个结果保留 Single，
第二个结果才建立增长缓冲；不能只在结束时转 Single 而仍预分配整个输入长度。

map 已知输出长度时选对应布局；filter 必须保持原回调顺序与次数，不做两遍
谓词求值。结果发布、复制前后以及回调 safepoint 的部分输出按准确类型保活。
这也覆盖 #234 的空/单命中情形，较多命中按有界增长；验收前不关闭该 issue。

共享 trace 层表达新的 Raw 内容边与 Single typed-value 边，mark、transport、
snapshot/恢复、Dyn 和 Host pin 使用同一含义。Dict 的列存储仍用专用 RawArray
对象语义，不把 Dict 列误读为带 kind 的 Array 描述符。

## 配额、停机与制品兼容

将长内容纳入受管堆后，原先 heap_limit 只限制 words 的含义发生变化；须同步
GcConfig 文档、统计口径与内容计数器，单独报告内容和一般对象。原 content Vec
不再留下一份长期镜像。Guest 总内存上限仍覆盖根表、工作队列、allocator 元数据、
transport 临时缓冲和编译产生的其他内存；失败不得保留可见半成品。

复制、扫描、增长与回收均在 Guest Wasm 的受 fuel 执行路径上。新增批量工作
不能挪到不计 fuel 的 Host 循环；大复制需核对引擎 memory.copy 的计量，必要时
分块并显式计费。编译 CPU 可放宽，执行 fuel 耗尽后沿用禁止重入规则。

当前共享 ABI 版本为 36。实现时提升版本并同步布局镜像、静态内容物化、
snapshot/service artifact、transport 的验证与边界编码；旧制品应明确拒绝，
不把旧 raw_start 或 Array 的 reserved=0 误解为新格式。不在 RFC 文档交付中改版本。
初始化静态对象、冻结图和请求临时图都必须能恢复、重定位、扫描并正确释放。

## 延后的方案

- 引用计数和所有权分析：引用复制路径仍广泛存在，本次保持准确根与 mark-sweep。
- 任意 T 的直接 Single 内联：先做布局成本原型，避免扩大所有数组与递归布局。
- RawArray 头与缓冲强制合并：会改变现有扩容共享机制，首版无此必要。
- 自动小切片脱离大 raw、按可达区间扫描元素：需要额外策略和度量。
- 通用 arena 内容压缩：本次目标是稳定 Raw 非移动回收；不暗含整理器。

## 实施与交付计划

1. 单独交付本 RFC，核对表示、范围和验收条件；在 #242 汇报。
2. 隔离原型比较 Single 三种方案，统计真实类型宽度与空/单数组频率；冻结编码。
3. 实现稳定 Raw String/Bytes，打通准确根、内容统计、边界和快照；阶段验收。
4. 实现 Empty/Single 与公共数组访问、构建路径，核对 Dict 与 Seq；阶段验收。
5. 补齐不可变追加、冻结图、资源失败和制品版本验证；同步当前设计文档及指南。
6. 功能完整后恢复全量测试，隔离重跑 lab-ontology/ic-model，对照性能并交付实现。

阶段选择性测试，不反复跑全量。编译器与实验产物均放 /tmp，不替换应用正在使用
的工具。工作分支提交与 issue 汇报沿用会话授权；RFC 与实现的合入分别等待人的授权。

## 可执行验收条件

- 布局：完整 Array 保持 24 字节；Empty 无底层分配；间接 Single 不建 RawArray
  头；来源保留；直接内联替代方案须先记录原型数据和 RFC 修订。
- String/Bytes：长度 0/15/16/17、多字节 UTF-8、嵌套切片、非零 start、尾部与
  非尾部追加、容量不足、大对象小切片、同一旧值分叉追加均正确。
- Array：长度 0/1/2、unit、Int、String、含引用 struct/tuple/enum、递归类型；
  map/filter/concat/flat_map/zip/enumerate/push/Seq/Sink、编解码及 Dyn 均覆盖。
- 分配：对大输入 filter 的零命中和单命中，不产生按输入长度分配的输出缓冲；
  回调次数/顺序不变；第二次命中再增长，增长乘加与配额正确。
- GC：自动回收关闭、每 safepoint 强制回收和正常间隔结果一致；覆盖临时 Single
  与 Slice、回调捕获、部分输出、独立子槽、Host pin、重复请求和来源边。
- 释放：所有长内容 Slice 死亡后 RawString 可回收；Single 从大数组脱离后原
  数组可回收；有子引用的 T 继续存活；未初始化容量不扫描。
- 共享：对同一原值连续生成不同追加结果、带非零 start 的视图追加，以及冻结
  图的请求内追加，旧值不变；请求顺序和恢复快照不改变结果与常驻图。
- 边界：静态常量、transport 往返、snapshot 保存/恢复、service 制品、旧 ABI
  拒绝；GC 不再修改活 String Slice 的 raw/start/end；Dict 两列不受误解释。
- 资源：低内存、大容量溢出、fuel 耗尽均及时终止；报告 managed/总 Guest
  峰值、allocated_total、collections、fuel 与墙钟，说明统计口径迁移。
- 性能：相同编译配置、配额与输入，对 String 密集、0/1/多元素集合基准及
  ic-model 至少各 3 次隔离对照，报告中位数与范围，分别解释分配、GC、复制
  和 CPU 变化。出现明显回退先调查并修订，不用布局节省替代性能证据。
- 完整实现通过 workspace 与语言完整测试；RFC 文档交付仅做链接、内容和 diff 检查。

## 阶段记录

2026-10-22：RFC #243 已由人授权快进合入 `61fc2e74`。布局原型见
[0314/README.md](0314/README.md)，确认首版间接 Single，Array 完整宽度保持 24 字节。
原型只有静态布局证据，真实应用动态频率和收益仍待实现后测量。
Raw 内容已接入受管堆与边界复制/快照，阶段 shared 33、content 3、service 8、
准确根 13 用例通过。Array 特化与按需 filter/collect 已完成主路径，增加了
Single 引用元素强制 GC、稀疏输出分配与非零 start 分叉追加验收。
Raw 尾部追加已实现并用于内部文本拼接，验证冻结与历史视图分叉。
完整验收覆盖及真实应用对照已完成，尚未合入实现。普通 GC 不搬移 Raw；
Array 保持 24 字节，Empty/Single 按实际长度发布，稀疏 filter/collect 按需增长。
循环内空数组发布须每次显式重置 reference/kind，不能依赖 Wasm local 的入口零值。
准确根依赖包含 #241 的 codec 临时路径修复，未单独合入 #241。

验收命令、原轮测试失败与修订后重跑结果如实保留在
[验收记录](0314/validation.md)；新增大 Raw 的中等切片保留／释放与 Single 脱离
大数组验证通过。三轮 [性能报告](0314/results.md) 显示 ic-model 初始化中位数
65.89→59.97 秒、fuel 约降 10%，Guest 峰值仍为 24.44 MiB。
零／单命中过滤累计分配约降 93%／92%；全命中几何增长造成额外缓冲复制，
耗时约回退 13.4%，String 密集样本 fuel 约增 8.7%。这些成本进入实现合入评审，
本版本不承诺普遍加速或显著降低 ic-model 的总线性内存配额。
