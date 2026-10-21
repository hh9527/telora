# RFC 0312：执行期间的准确追踪 Mark-Sweep GC

- 状态：Implemented（工作分支实现与验收完成，待审查；未合入 main）
- 跟踪：[#232](https://github.com/hh9527/telora/issues/232)
- 工作分支：`rfc/0312-mark-sweep-gc`
- 日期：2026-10-21
- 关联：RFC 0292（Wasm 执行）、0300（Host/Guest ABI）、0302（单 Vec 堆）、
  0304（闭合物理布局）、0305（函数体依赖）
- 当前设计基线：[IMPLEMENTATION.md](../docs/design/IMPLEMENTATION.md) 第 5、6 节与
  [LANGUAGE.md](../docs/design/LANGUAGE.md) 第 10 节

## 动机与范围

Telora 需要支持更长的纯数据计算：单次计算可能构造并丢弃大量小对象，计算结果、
property、闭包和未来的缓存可能长期存活。初始化结束或请求结束不再是唯一可用的
回收边界。Agent 动态生成和执行源码进一步放大这一需求，但本 RFC 不引入语言内
eval、动态加载、jq/JSX 语法或缓存 API。

资源目标是内存有边界、CPU 可停机，不要求精确的编译 CPU 成本模型或实时 GC。
编译器未来可以编译为 Wasm，但编译器部署方式和 Host 总任务配额另行设计。本 RFC
负责 Guest 内的语言堆及其关联资源，不能把 Guest memory limit 宣称为整个进程 RSS 上限。

第一版采用单线程、停止程序执行后准确追踪、非移动的 mark-sweep。正常执行可以
经过多次 GC 后继续，不依靠 reset 恢复内存。reset 继续服务于现有请求隔离语义，
不承担长期堆正确性的前提。

GC 按类型选择性接入：String、Array、Dict，以及传递包含这些类型的
struct/tuple/enum 需要追踪；纯标量及不包含这些成员的聚合不因当前实现将它们
放在 words 中就被提升为 GC 对象。Bytes 复用 String 的内容存储规则；闭包和
opaque 类型的内部所有权见下文，不能仅按公开函数签名判断。

本 RFC 修订 RFC 0302 的只追加语言堆及事件边界回收方案，并将 RFC 0305 延后的
执行中 GC 纳入范围；保留闭合类型布局、纯计算、来源追踪和静态类型身份。
不改变 property 的求值阶段、初始化集合或 demand 根裁剪策略。property 在打包期
求值的目标是设计背景，必须另行规定与外部 source 的依赖、固化和发布契约。

## 一、源码调查与证据

以下是提案时的源码调查基线；后续实现状态见第十节的阶段记录。

| 入口 | 现状及约束 |
| --- | --- |
| `crates/telora-wasm/rt/heap.rs` | `Vec<u64>` 追加、freeze/truncate；引用为 `origin + offset`；没有空洞复用和分配边界索引。Vec 扩容后物理地址可能变化。 |
| `rt/collect.rs`、`rt/collect_trace.rs` | 布局驱动的边界复制回收；显式工作表处理共享和环；旧堆在复制期间仍存活。现有扫描同时执行重定位，不能原样当作 mark 阶段。 |
| `rt/collect/initialization.rs` | 所有 Ready demand 是初始化根，诊断和 debug 保序保活。这里“保守”指多保留语义根，不是猜测整数是否为指针。 |
| `src/emit.rs`、`src/stack_values.rs` | 表达式值、原始存储引用、索引等都可能使用 i32 local；局部槽位会复用。`LocalScopes` 是 codegen 槽位复用机制，不是已证明正确的 GC 活跃性描述。 |
| `src/functions.rs`、`src/tail_calls.rs` | 闭包环境记录捕获引用及封闭 TypeId；参数数组单独分配；尾调用使用 `ReturnCallIndirect`。尾调用不能累积 shadow frame。 |
| `src/capture.rs` | 语言失败可以被捕获并继续；保存 error/phase、截断诊断表，且恢复旧状态。新增根必须覆盖这些保存值及失败退出路径。 |
| `rt/content.rs`、`telora-wasm-shared/src/arena/content.rs` | String/Bytes 有内联表示和共享切片；非内联内容独立追加，边界回收汇总存活范围并复制。只回收 words 无法约束字符串负载。 |
| `rt/tables.rs`、`rt/regex.rs` | 部分对象通过直接堆引用访问，部分资源通过表 ID 访问；Regex 持有独立 Rust 对象。表槽和 Rust 资源都需要释放和复用。 |
| `rt/service.rs`、`rt/service_sources.rs` | Rust 状态持有 initializer、handler、source key/value 等语言引用；初始化期间的根与 Ready 阶段不同。只扫描成功 service 不足以覆盖执行中 GC。 |
| `src/transport.rs`、`src/input_heap.rs`、`src/session.rs` | Host 在多次 Guest 调用之间持有值引用并组装输入；Host 内的整数句柄不会自动成为 Guest GC 根。 |
| `rt/snapshot.rs`、`rt/data_image.rs` | 保存连续 words/content、表、demand 和独立资源；分页、空洞、扫描元数据不能靠现有字节镜像自动恢复。 |
| `telora-core/src/candidate_layout.rs`、`src/emit.rs` | 布局已区分固定载荷、字节内容、序列、捕获等存储，但 `scalar_as` 仍经 `value_as -> alloc` 生成堆地址；固定 struct/tuple 也可能有间接存储。现有“有地址”不代表类型需要 GC。 |

现有执行证据包括 `src/tests/{expression_locals,tail_calls,content,host_memory}.rs`，
以及闭包、来源、Regex、service 和 build/runner 测试。这些覆盖当前行为，但没有
证明单次长计算内反复回收正确。本次调查没有运行性能实验，不据此给出收益数字。

## 二、用户可见语义

GC 不增加语言层可观察的对象地址、finalizer、弱引用或回收通知。成功计算的值、
函数身份、相等关系、来源和诊断顺序不因回收变化。来源仍不参与值相等，但 GC
必须保留仍可报告的来源，不能把“代码通常不能观察来源”当作丢弃它的依据。

GC 的指令消耗包含在现有 Guest fuel 中；不承诺沿用旧实现的具体 fuel 消耗量。
内存/fuel/stack 耗尽仍是终止性失败，不能被 `diagnostic`/Test 的可恢复失败捕获。
对于必然增长的存活图，允许耗尽资源；GC 不承诺让任何程序都能完成。

初版不增加公开内存配额或 GC 调参语法。现有 Wasmi 内存增长限制保持硬边界；
内部分配阈值和预留空间用于在硬边界之前触发回收，不构成精确进程内存核算承诺。
不同 GC 策略不改变成功值，但可能改变程序是否在给定资源预算内成功。

## 三、准确根与安全点

### 3.0 类型分类与值的存储

编译期针对封闭实例计算 `contains_gc_refs(T)`，生成字段扫描描述；规则依赖类型
身份与布局，不依赖源码名字。对于普通值，基础规则如下：

| 类型 | GC 处理 |
| --- | --- |
| Int/Float/Bool/Type/TypeOf 等固定标量 | 不含 GC 引用，不生成 GC 根或每值 GC 分配 |
| String/Bytes | 类型可能持有动态内容；运行时内联值不持有内容块，非内联值标记所属内容分配 |
| Array(T) | 存储归 GC 管理；仅在 T 含 GC 引用时扫描元素，不逐个追踪 Array(Int) 的数字 |
| Dict(T) | 存储归 GC 管理；扫描字符串 key 及含 GC 引用的 value |
| struct/tuple | 根据字段递归计算；不含 GC 引用的字段跳过，全部字段均不含时整个值无需扫描 |
| enum/Option/Result 等和类型 | 取各分支可能引用的并集分类，实际扫描只访问当前 tag 的 payload |
| 透明包装/newtype | 继承实际载荷的分类，不仅因为多一层包装就新增 GC 对象 |

`contains_gc_refs` 不等于 `is_gc_allocation`：含 String 的 struct 可以按值保存在
调用帧或数组元素中，只需描述其中引用，不必为 struct 再分配一个独立 GC 对象。
同样，无引用的 Array(Int) 元素不扫描，但数组的动态存储仍必须标记和回收。
分类在类型图上通过工作表/固定点求解，不用递归展开泛型或递归类型。

需要分开处理以下内部表示，不把它们混进“所有值都由 GC 管理”的旧假设：

- 闭包扫描依据实际捕获环境，不依据 Fn 的参数/返回类型；只捕获 Int 的环境没有
  GC 字段，但环境自己的动态存储仍需要所有者。逃逸环境的保活与非逃逸环境的
  栈/临时存储策略必须明确。静态代码指针不是 GC 对象。
- Dyn 使用封装载荷的实际布局；Fmt/Test/Blame 等 opaque 表示必须声明内部引用
  描述，不能仅因公开类型没有 String 字段就视作无引用。
- Regex 等独立 Rust 资源需要所有权与析构协议，不能因不属于上述普通容器而泄漏。
  递归 enum 的 boxed 载荷也需要明确存储所有权；类型图不含 String/Array/Dict
  并不自动意味着递归表示可在固定大小空间内内联。
- 内部动态块哪些由 GC 管理、哪些由帧/宿主显式拥有，在第 1 步列出完整表；
  若需要扩大普通语言类型的 GC 范围，须先补充本 RFC 的决策，不能隐含引入。

非 GC 值在 Wasm locals 或有明确释放边界的帧/scratch 存储中传递。第一版可保留
“按地址读取固定值”的调用约定，但地址指向的 scratch 必须及时复用，不能继续
每次标量运算追加永久 words，也不能把这些 scratch 块伪装成 GC 对象来规避改造。
返回值和逃逸捕获需要复制/转交到比当前帧更长的明确所有者，不能返回已释放帧地址。

来源随非 GC 值保留，来源元数据的保活协议与对象指针扫描分别描述。没有 GC
字段不代表其动态来源可以提前释放，也不要求为每个标量引入 GC 堆分配。

### 3.0.1 存储与交接的实施决策

源码进一步审计表明，仅给 `scalar_as` 更换分配入口不够：`RecordTable`/
`NewtypeTable` 即使字段全为 Int 也有独立间接对象，当前闭包还捕获这些值头的
地址。若只复用值头，返回聚合、Array 中的聚合和闭包仍会引用已失效的 scratch。
因此后续接入采用以下协议；本节描述待实现的方案，不代表当前 ABI。

1. **固定值按值存储。** 无普通 GC 引用、无内部动态所有权的有限 struct/tuple/
   newtype 使用内联字段布局，字段保留完整来源头；复制整个固定值即可转交所有权，
   不再为其间接载荷逐值注册 GC 分配。enum 的有限内联载荷遵循相同规则。先在
   类型依赖图上确定递归 box 与内部动态边界，再求有限布局，不能递归展开类型环。
   含引用聚合可保持当前间接表示，后续优化不作为第一版验收前提。
2. **scratch 归调用帧所有。** 编译器按活跃值的布局宽度安排槽位；表达式结束或
   local 复用时释放槽位，循环复用同一组槽。纯标量/纯有限聚合使用该存储；含引用
   值的固定头也可使用 scratch，其动态存储由准确根保活。一个调用帧的 scratch
   是明确拥有的内存区，不是等待 GC 猜测可达性的对象。正常返回和可恢复失败释放
   帧，尾调用交接后复用帧；不能每次调用追加一个永不归还的 scratch 区。
3. **caller 拥有结果空间。** 新内部调用契约显式传递输出地址，callee 在退出前
   复制固定值到该地址。caller 先预留空间，在下一 may-GC 点前发布结果根。
   返回参数/字段借用同样执行复制；禁止把 callee scratch 地址交给 caller。
   普通函数与生成 helper 分别声明“固定值输出”或“原始整数/状态输出”，Equal
   等 helper 不按语言值复制。RT invoke、Host 和生成调用点一起迁移，不靠猜测
   `key.ty` 的返回类型识别 helper 契约。
4. **捕获按拥有者转交。** 普通不可变捕获将完整固定值复制到逃逸环境；环境拥有
   捕获区，字段含 GC 引用时按实际闭合类型扫描。预留的局部函数头具有前向填充
   语义，必须保留显式 managed cell，捕获描述区分 inline value 与 cell reference；
   禁止把尚未填充的递归函数头按值复制成永远未初始化的函数。
5. **尾调用使用独立交接区。** callee/environment/参数值在当前帧退出前复制到
   受保护 transfer packet，输出地址沿用原 caller 的地址。交接区在 callee 接管前
   保活，并允许有界复用；不能让新参数继续借用将被覆盖的旧帧。参数原地覆盖时
   要处理重叠，可用两个轮换的交接区，不能假定参数之间没有别名。

内部动态存储的所有者明确如下：

| 存储 | 拥有者与扫描职责 |
| --- | --- |
| Array/Dict 动态载荷，含引用聚合的间接载荷 | managed allocation；描述区分原始容量、初始化前缀和元素/字段类型 |
| 逃逸闭包环境、局部函数前向 cell | managed internal allocation；扫描实际捕获和 cell 的初始化状态，不依据 Fn 签名 |
| 递归 enum box、Dyn boxed payload | managed internal allocation；扫描实际载荷 TypeId，与普通 `contains_gc_refs` 分类分开 |
| String/Bytes 的非内联内容 | managed pointer-free allocation；切片保活其原始分配 |
| Fmt/Test/Blame/诊断等内部载荷 | managed internal allocation 或显式资源表槽；按内部布局描述扫描，表自身不是根 |
| Regex | 资源槽显式拥有 Rust 资源；从存活资源引用标记槽，扫除时析构一次并复用槽 |
| 非 GC 值头、参数临时区、固定聚合 | frame scratch；Host 跨调用持有的同类存储由显式 pin/lease 拥有并释放 |

来源根独立于对象根：标量保活一个 packed range，纯内联聚合按布局保活其字段范围，
enum 仅访问当前分支。纯聚合不为此变成 GC 对象；根注册表需要表达“按类型只追踪
来源”的角色，不能用一个头部 Source 根覆盖聚合内部所有来源。

以上改动涉及闭合布局、输入输出、反射、codec、调用 ABI 和制品版本，必须成套
迁移。先完成这种存储约束，再连接自动分配慢路径；不以“扫描 flags 为零”冒充
已消除纯标量/纯聚合的逐值堆增长。

内部生成调用统一采用 `(environment, arguments, output) -> i32`，参数仍为逻辑
引用；语言值成功返回 output，语言失败返回 0，raw-result helper 忽略 output。
参数数组继续按静态签名解释，各 helper 的前两个参数角色单独规定，不把所有
调用误认为 closure 调用。`LocalScopes` 的 scratch local 起点随三参数 ABI 迁移，
不能保留当前硬编码的 2。RT 的函数指针转换、尾调用、静态直接调用、Host invoke
及生成导入类型必须同时更新。

demand 不把 caller scratch 地址登记为 Ready 值：每个 demand cell 拥有其固定
布局结果区，首次成功时复制进去，后续调用复制到各自 output。固定结果区数量
受制品的 demand 集约束，其引用字段仍作为语义根扫描。Failed 状态保存原有错误
身份。service initializer/handler 和 Host 长期持有的结果同样必须有独立拥有者，
不能把“执行调用已经返回”误当作“返回值的固定存储仍然有效”。

scratch 物理存储按帧/slab 分配，逻辑地址与 managed storage 的地址转换统一；
slab 通过显式帧退出释放，不能混入 sweep 的普通对象集合。编译期计算固定槽总
宽度及可复用范围，包含多个独立来源头的纯聚合也只占其固定槽。原始物理指针
不跨 may-GC 借用。Host 的非 GC 输出区归 pin/lease 所有，unpin 同时释放固定
存储与来源根；pin 普通 managed 对象时则只解除根，由后续 sweep 回收动态存储。

### 3.1 根是显式协议

编译器生成线性内存中的 shadow stack，记录当前活跃引用及对应扫描描述。
Wasm local 和 operand stack 不由 collector 自动枚举，不扫描“看起来像指针”的
任意 i32/i64。数值、TypeId、表 ID 和直接语言引用必须有明确区分。

根描述至少区分：含 GC 引用的封闭 TypeId 值、内部动态存储所有者、闭包环境、
带布局的原始对象/数组和构造中对象。无 GC 引用的普通值与 scratch 不生成对象根；
它们可能仍有来源保活记录。不能用假的 TypeId 解释原始内存。
具体编码在实施第 1 步确定；必须覆盖下列不变量后才能进入分配器改造。

- 根槽引用有效对象或明确的空值；回收不能解释尚未初始化的 tag、长度或字段。
- 一个 local 复用为新角色之前，旧根被清除或替换；分支合流显式决定根状态。
- 表达式中尚待后续参数、字段或分支使用的结果必须保活；不能只枚举词法绑定。
- 循环迭代结束或确定最后一次使用后释放临时根。首版可在受限词法范围内多保留，
  但不得累积每轮垃圾，或把整次长计算的所有历史临时值保活到函数返回。
- 内联值和指向数组元素/字段的内部引用保活所属分配及必要扫描范围；不能只标记
  内部地址所在字节，也不能不加说明地拒绝现有 interior reference。
- 正常返回和所有可恢复失败经过根帧清理；返回值在 caller 接管前不能失根。
- 尾调用在移除当前帧前，把 callee、environment 和参数转交给受保护的调用包。
  callee 接管前没有可触发 GC 的无根窗口；递归次数不能造成根栈线性增长。

不要求首版引入完整寄存器所有权 IR 或每次 local 复制的 retain/release，但需要
统一的引用生产、借用、发布与失效 API。裸 `LocalSet` 不能悄悄改变根槽含义。

### 3.2 安全点与构造协议

GC 只在根集合一致的安全点发生。可能分配的调用之前发布仍需使用的值；只在函数
入口回收不足以覆盖单个生成 helper 或 RT 内部的大循环。

代码与 RT 必须标注/区分 may-GC 和 no-GC 路径：

1. 可扫描的已发布对象：字段与布局有效，GC 可正常访问。
2. 构造中对象：保护其分配，按描述扫描已经初始化的引用；用初始化前缀或位图
   表示进度，不能按最终数组长度扫描空 tag。递归闭包的预留头也使用此协议。
3. 有界 no-GC 临界段：只做预留空间内的写入；不得包含可无限增长的分配或用户回调。

RT 使用 scoped root 保护临时对象，生成 helper 与 RT -> Telora 回调遵守相同协议。
语言引用与转换后的物理地址不同：Rust slice/裸指针不得跨会分配、重入或回收的
调用长期借用。非移动 GC 不豁免这一限制，底层缓冲区扩容仍可能使物理地址失效。

### 3.3 根拥有者清单

| 拥有者 | 根及释放条件 |
| --- | --- |
| 活跃生成帧/RT 帧 | 参数、捕获、表达式临时值、构造对象、调用包、保存的错误状态；退出或最后使用后解除 |
| demand/property | 保留制品现有全部 Ready 值；Running 的计算现场由活跃帧保活；Failed 关联的反馈须能报告 |
| service 生命周期 | 按阶段枚举 plan/initializer、已注入 source key/value、handler、处理中请求与待输出结果；不使用 Ready-only 枚举器 |
| Host | 跨 may-GC 调用继续使用的值必须显式 pin/unpin；批量输入组装时先前已构造的参数也须注册 |
| 反馈与命令调度 | 未消费或仍在捕获范围内的诊断/debug，以及 eval/Test 结果与待执行 Test；消费/离开后按原协议释放 |
| 固定镜像 | 当前制品的静态类型/布局/函数描述保持有效；任何可变外部槽持有的堆引用另行枚举 |

Host pin 表是准确根；不能把过去返回过的每个值永久注册。释放后的句柄再次使用
应在可检查边界被拒绝；需用句柄代数或等效检查防止槽复用后的陈旧句柄误命中。
当前 `Value { pointer, ty }` 的会话有效性约定必须与这项改造一起审计。

## 四、堆与扫描

### 4.1 非移动分页分配

GC 管理的动态存储使用固定逻辑页范围、小对象尺寸分级和大对象独立块。页内可以顺序分配，
回收后通过空闲槽复用。分配元数据记录块边界、大小、分配状态、标记与扫描角色；
使用旁表还是对象头需在第 1 步以现有值宽和小对象开销测量决定，不偷占用户载荷。

存活对象的逻辑引用不因日常 GC 改变。保留“引用通过 RT 换算访问”的抽象，不承诺
继续使用单 Vec 或现有偏移编码。字段偏移和内联值可访问性必须保持，不能让一个
大对象在逻辑或物理访问上被不透明地截成不连续片段。

空闲页可以归还 Rust 分配器并复用；Wasm memory 不会因此缩页。必须分别统计
存活、已分配、已提交、空闲和线性内存高水位，不能只报告存活值字节来证明资源受控。
尺寸分级的内部碎片、页外大块的外部碎片和元数据均计入观测。

### 4.2 统一准确扫描

从 `collect_trace.rs` 提取与复制/重定位分离的扫描逻辑，复用闭合布局镜像及类型分类：
普通值、Array/Dict/Record、内联/boxed Enum、Dyn、Newtype、闭包环境、Fmt、Test、
Blame、诊断及资源。扫描只访问类型描述确定的 GC 字段和内部所有权边；遍历以
显式工作表进行，访问去重，不按输入深度递归。

标记内部引用时定位拥有分配并保证其生命周期；扫描布局仍来自类型与构造协议，
不能反向通过堆内容猜测类型。多视图共享同一分配不重复释放，环不导致无限扫描。
函数代码通过稳定 ID 读取 demand 的依赖关系继续遵循 RFC 0305；不因为数据图里
没有普通指针就清除仍能读取的顶层值或 property。

### 4.3 String/Bytes、表与关联资源

- String/Bytes 内联表示继续支持。非内联内容采用可回收的稳定逻辑范围，每个
  视图保活所属原始分配，原始内容分配不含语言指针；范围、UTF-8 和共享切片契约保留。
  首版不要求把存活小切片裁剪成最小载荷；小切片可能保留整个大分配，须单独测量。
  当前连续 CONTENT_VIEW 与 Host 解码、快照格式需同时适配，不能只替换 RT 存储。
- 对象/资源表只提供索引，不意味着每个占用槽都是根。槽释放、空洞与复用必须
  有明确状态；扫除 Regex 时释放 Rust 所有权且恰好一次，不能无限增长 table.length。
- 动态来源由活跃值、反馈、解析任务和仍能产生该来源的执行任务保活；静态来源
  继续按现有制品策略保留。Rust 名称与 BOLs 所有者同样受内存边界约束。
- Rust 临时缓冲区和 Host ABI buffer 仍由各自所有者释放，不作为语言对象猜测扫描。
  它们占用 Guest 线性内存，不能以“不经过语言堆”为由排除资源测试。

## 五、回收调度、内存边界与停机

在发布完整根的安全点检查分配阈值，执行 mark/sweep 后继续计算和分配。
底层分配器不自行重入 GC；缺少合法安全点或申请仍无法满足时终止，不能在构造
状态尚未发布时强行扫描。这是首版的分配准入契约，不保证用尽每个可回收字节。
阈值按已分配量和上次存活量调节，接近硬限制时减少增长余量，避免每个小对象都
全量扫描。大块申请单独检查溢出、布局上限和可用空间；有空闲总量不保证存在足够
大的连续块，首版允许可诊断的碎片性分配失败，不声称消除了碎片。

标记位、工作表和扫描期间临时空间必须有预算/预留。工作表按可扫描对象/边界计量，
在分配时预留或通过有界分段增长保证回收可启动；不能等硬上限耗尽再为 GC 申请
另一份同等大小的堆。预留不足和分配器 OOM 允许作为终止陷阱，由 Host 统一分类；
不保证 OOM 后仍能分配并渲染完整的来源诊断。

mark/sweep 的 Wasm 循环消耗现有 fuel；清扫不调用用户代码。fuel/取消/陷阱可以
中断 GC，此时实例标为不可继续使用并废弃或按现有策略重建，不恢复半完成状态。
可恢复的语言失败必须正常清理根帧，随后可继续执行和 GC。这两类退出不可混同。

## 六、请求隔离、固化和兼容

长期堆不得通过 work 后缀位置定义存活性。现有服务仍保持每次请求从相同初始化
状态开始；GC 能长期运行不意味着自动引入跨请求可见状态。请求分配区可以作为优化，
但只有证明无逃逸引用时才能整体释放；否则使用完整根图回收。未来 cache 若跨
请求保留对象，必须由其独立 RFC 明确状态与 reset/snapshot 的关系。

初始化数据镜像、ready-service snapshot 与制品固化具有独立生命周期：

- 日常 GC 保持存活引用稳定，不需要移动对象以生成快照。
- 导出器可以从明确根图生成紧凑表示，但不得重定位仍在执行的实例；在副本中
  编码，或只在已证明没有外部旧引用的边界执行离线重定位。
- 导入须重建分配边界、扫描描述、根、资源表及空闲状态，并能立即执行下一次 GC。
  不能把一段恢复的 words 当作一个不可回收的大对象来通过短期功能测试。
- 本次堆/引用/字节内容表示可能改变内部 ABI。更新 ABI 与 snapshot/data image
  版本，Runner 拒绝不匹配制品；不要求读取旧实验格式，不允许静默误读。

第一版必须继续支持普通 build、snapshot build、source 覆盖触发重新初始化和独立
runner。修改运行时不能只验证源码执行路径。

## 七、备选与延后方案

| 方案 | 本期取舍 |
| --- | --- |
| 继续仅做边界 copy/reset | 无法回收单次长计算的临时对象；不作为主方案 |
| 执行中 copying / 分代 copying | 可优化短命小对象，但需要准确根、移动更新与额外空间；先测首版再决定 |
| RC + 周期性 tracing | 与长期方向兼容；当前 local 没有所有权协议，不能安全直接插入 retain/release，延后 |
| 仅对堆边 RC、locals 不计数 | RC 归零仍可能被 local 引用，必须有延迟释放与根保护；不作为无需根协议的捷径 |
| 保守扫描所有 Wasm 整数或把全部 locals 永久保活 | 可能误保留、不能可靠解释表 ID 或内联值，不满足本 RFC 的准确根及长循环验收 |
| 并发/增量 GC、弱引用、finalizer | 本期没有停顿目标，不增加调度与可观察时机问题 |
| Wasm GC proposal | 与目前线性内存 RT、Wasmi 和可移植制品契约不同，另行评估 |

## 八、实施顺序与阶段门槛

本 RFC 作为独立文档交付，文档验收只证明现状引用、方案、范围及验证计划完整，
不代表 GC 已实现，也不代表下列实施前问题已经有性能结论。RFC 合入后，后续代码
交付仍在独立工作分支按标准流程推进，阶段验收与结果记录在跟踪 issue。

实施第 1 步必须补齐以下决策，完成前不进入第 2 步：

- 逃逸闭包环境、递归 enum 的 box、Dyn/opaque 和独立 Rust 资源的具体所有者，
  以及它们与普通语言类型选择性 GC 规则的关系。
- 非 GC 标量/聚合的地址调用约定、scratch 回收及返回/尾调用转交布局，避免
  仅减少扫描却保留每次计算追加分配的增长路径。
- shadow frame、构造进度、内部引用和 Host pin 句柄的具体布局与安全点清单；
  必须覆盖原始数据物化、生成 helper 和 RT 回调，不能仅处理普通函数调用。

1. **根/扫描协议原型与基线**：定义引用角色、构造状态、interior root 和根帧 ABI，
   计算按类型的 GC 分类，列出内部存储所有权及非 GC 值的帧/scratch 生命周期，
   审计全部分配/回调/返回入口。记录小对象尺寸分布、存活率、现有峰值和耗时。
   用验证模式检查 live roots 与 local 复用、分支和尾调用；发现角色不明不得继续。
2. **分配器与扫描核心**：实现分页、空洞复用、大块分配、标记和显式工作表；
   用手工准确根验证共享、环和内部引用。新 collector 未接齐根前不得自动触发。
3. **执行中安全点**：接入 codegen、RT helper、构造过程、Host pin 和各阶段根；
   验证每个安全点都可强制收集，长期循环的根槽和临时存活量不随迭代数增长。
4. **完整资源生命周期**：接入字节内容、资源表、Regex、来源/反馈与 OOM/陷阱路径。
   到此必须能在单次调用内靠多次 GC 完成；仅 words 复用不算完成。
5. **制品和边界**：适配 data image/snapshot、runner、请求 reset 与 ABI 版本。
   移除日常回收对旧 copy-collector 的依赖；图固化如复用复制逻辑则独立命名。
6. **验收与文档同步**：执行下节验证并记录数据；实现落地时同步 LANGUAGE、
   IMPLEMENTATION 和受影响指南，随后更新本 RFC 状态。未实现方案不先写进 SSOT。

页大小、尺寸等级、root descriptor 编码与阈值常数在第 1 步给出数据和具体布局，
属于本 RFC 的实施细化。若改成保守扫描、依赖无限 no-GC 区域、删除内联值支持或
改变初始化/请求语义，必须修订 RFC，不属于实现自由度。

## 九、可执行验收条件

Rust 测试负责分配器、根协议、ABI 和精确来源，独立 `.telora` fixtures 负责语言
行为。强制 GC 开关为内部测试能力，不增加产品语言 API。测试至少覆盖：

| 契约 | 必须提供的证据 |
| --- | --- |
| 单次长计算可回收 | 同一次调用内分配总量显著超过可用堆，多次 GC 后成功，固定存活集；禁止测试驱动逐轮 reset |
| 类型选择性 | 标量与无 GC 字段聚合不生成 GC 对象/根；含引用聚合只扫描指定字段，enum 只扫当前分支；Array(Int) 存储可回收但元素不扫描；长标量循环的 scratch 有界 |
| 初始化/property | provider 和顶层求值中多次 GC；互相依赖、闭包结果、Running 环检测及来源均保持 |
| 活跃根 | callee/先前参数等待后续参数，分支合流、local 复用、operand stack 交接、构造中数组和递归闭包；在每个合法安全点强制 GC |
| 尾调用/失败 | 深尾递归根帧有界；返回与捕获失败释放临时根，之后再 GC/调用正常；燃料陷阱不继续使用损坏实例 |
| 对象图 | 共享子图不重复释放，环可回收；动态/内联/boxed 值、字段/数组内部引用与闭包环境均可达 |
| String/Bytes | 长字节串反复产生、切片和丢弃后复用存储；保留跨 GC 的切片内容和 UTF-8；单独量化小切片保留大块 |
| 独立资源 | 临时 Regex 及表槽可重复释放复用；有存活引用时不析构，回收恰好一次；来源/BOLs 与反馈生命周期正确 |
| Host 持有值 | 多次 Guest 调用之间 pin 的多个值仍正确；unpin 后可释放，旧句柄不能误指向复用槽；多参数输入组装不丢失前序参数 |
| 内存硬边界 | 存活图持续增长、大块和碎片申请、GC 辅助空间不足都受限制终止；不得依靠未计入的无限 RT 缓冲区 |
| CPU 可停机 | 用户循环与标记/清扫过程可耗尽 fuel；不会被 `should_fail` 当作成功捕获 |
| 镜像与服务 | data image/ready snapshot 导入后立即多轮 GC；source override、连续正常请求和 trap 重建保持现有结果与隔离 |
| 性能与占用 | 小对象、共享树、循环图、长字节内容、现有真实 fixture 各自记录耗时、fuel、GC 次数、存活量、提交量、碎片和线性内存峰值 |

长期稳定测试对固定存活集比较预热后的多个等长窗口，断言提交页数、根帧/槽、
资源表容量及来源记录在声明的余量内趋稳；仅断言“GC 后存活字节小”不够。
具体余量由固定 fixture 和分配等级确定，不能以随迭代次数线性增长的界限通过。
继续保留“真正在增长的存活图最终被内存限制终止”的反例。

建议验证入口（实施后执行并补充实际结果，不表示本次已经通过）：

```sh
cargo test -p telora-wasm
cargo test -p telora-wasm-shared -p telora-data
cargo test -p telora
cargo test -p telora-run
node scripts/build-run-smoke.mjs
node scripts/http-serve-smoke.mjs
bash scripts/check-source-size.sh
```

其中 CLI 语言验收需要按现有说明准备 Bash/jaq。新增 allocator 的纯 Rust 核心
若可独立测试，应增加可重复的分配/释放序列与整图扫描对照，不以只镜像实现的
单步测试代替上述生命周期测试。

## 十、阶段实施记录

### 2026-10-21：布局分类与分配核心原型

闭合布局镜像的 flags 区分普通 GC 引用、内部所有权和动态载荷。无运行期表示的
类型不参与值扫描，递归布局通过工作表传播分类；函数签名不冒充环境布局。

共享库新增独立分页 mark-sweep 存储核心，逻辑引用保持稳定。原型采用 16 KiB
小对象页、8 字节起的 2 次幂尺寸等级，超过 4 KiB 的申请使用独立块；元数据为
旁表，避免改变现有值载荷。页内槽可复用，空页释放物理缓冲区并合并空闲逻辑
范围，防止不同大小的临时大块持续消耗虚拟引用空间。这些参数是可测原型基线，
尚无真实工作负载的性能结论，不作为公开 ABI 承诺。

该核心接受显式扫描描述及准确根，支持内部引用保活所属分配，不自行猜测对象边。
目前未接入 RT 分配路径，也未启用执行中自动 GC；非 GC scratch、根协议和制品
适配仍需继续实施。此阶段不能宣称 RFC 已落地或类型选择性分配已在运行时生效。

### 2026-10-21：准确根生命周期原型

共享库的 `gc_roots` 定义帧和 Host pin 的生命周期模型，尚不是已经接入的 Wasm
调用 ABI。根角色区分封闭类型值、带类型/tag/初始化前缀的构造对象、独立来源拥有者。
帧记录 generation/base/count，槽位复用前显式替换或清空；只有栈顶帧可修改或退出。
普通返回和可恢复失败共用 LIFO 退出协议，返回交接期间禁止 GC。

尾调用通过受保护 transfer packet 原子替换当前帧，接管失败保留旧根；Host pin
采用 slot/generation，拒绝重复释放及复用后的旧句柄。空帧同样受数量边界约束，
unpin 所需的空闲槽容量在 pin 发布前预留，避免释放路径还要申请内存。

验证覆盖重入期间 caller 根、可恢复失败清理、99,999 次尾交接保持一帧两槽、
10,000 次 pin/unpin 后槽数保持稳定，以及配额与 generation 耗尽。
这些测试证明注册表协议，不证明 codegen 已完整发布根；自动 GC 继续保持未启用。

### 2026-10-21：扫描与重定位分离

共享库 `trace` 通过显式 visitor 输出 Value/Object/Content/Storage/Source/Function
边；布局遍历本身不分配、标记或改写对象。现有 RT 的复制收集器已接入此扫描器，
由适配层处理复制、重定位、资源与来源，不再维护另一套类型遍历分支。
内联 enum/Dyn 的载荷链改为迭代访问；对象图仍由 collector 的显式工作表去重处理。

扫描接口的 old/at 对应读布局与待写槽；mark visitor 可令二者相同且不改写槽。
来源事件携带 packed range 的地址，与对象引用分开；修正内联 enum 载荷和 Blame
subject 将 range 的低 32 位误当 source ID 的路径。新增 Guest 回归从 freeze 后
注册动态来源，仅通过 inline Option(Int) 的载荷保活，连续回收后来源仍可解码。

此阶段仍使用原有边界复制收集器，未切换 RT 分配器或启用自动 GC。构造中对象、
scratch 及根发布协议尚须接入，不能把共享扫描器通过验证当作执行中回收的验收。

### 2026-10-21：统一生成函数的普通退出边界

`Emitter::finish` 将所有生成函数的普通 return 汇聚到同一 epilogue，而不只处理
demand 函数。包括原始整数 helper 的输出均原样经过 i32 结果块；尾调用仍走
独立的 `ReturnCallIndirect` 路径。此阶段只准备帧清理边界，尚未释放 scratch 或
发布 GC 根，也未改变调用签名。

完整 Wasm 回归 106 项通过，覆盖正常/提前返回、捕获失败后继续、raw-result
helper 和深尾调用。新增的来源测试也已用旧扫描器反向验证，确实因缺少来源
元数据失败，恢复共享扫描器后通过。

### 2026-10-21：纯有限聚合的内联存储

布局求解器以最小固定点计算不含动态所有权的有限值；递归、容器、闭包和 opaque
不会仅因表面上没有 String 字段而被误判为纯值。纯 struct/tuple/newtype（包括
空 struct）的字段内联在完整值中，payload 使用固定标记，字段从偏移 16 开始。
标记可随值复制，不保存指向自身或原值的绝对指针；字段仍保留独立来源。

生成器的聚合构造、投影、Host 输入/输出与 debug 表示、trace image 同步支持此
布局，内部 ABI 更新为 29。含引用或内部动态所有权的聚合继续使用间接存储。
扫描内联字段输出 Inline work，由 collector 工作表遍历，不为字段生成独立
对象或递归消耗 Rust 栈。纯 tuple/record 构造直接写入结果区，避免额外的临时
间接载荷副本。

新增 Guest 测试验证嵌套 struct/tuple/newtype 与空 struct：复制进 Array 和返回值
后改写原输入区，副本仍独立；随后多次边界回收仍保持结果。共享扫描测试覆盖
10,000 层内联字段，用有界工作表遍历来源，不产生 Object 边。
当前固定值头仍使用原分配路径；frame scratch、caller 输出空间和自动 GC 尚未
接入，本阶段不能宣称单次长调用的内存增长已解决。

阶段验收：核心库 207 项、共享库 31 项、Wasm 107 项测试通过；共享库 Clippy
（包含测试、warnings denied）、格式与 diff 检查通过。产物使用隔离目录。

### 2026-10-21：调用方结果区与三参数调用 ABI

生成函数签名落实为 `(environment, arguments, output) -> i32`，普通退出在成功且
output 非零时复制完整值并返回 output，失败仍返回 0。结果类型来自每个封闭
Key 的显式契约：Equal/Json 为 raw helper，Property 使用 property 类型，
DecodeVariant 返回完整目标 enum。静态调用、array/dict 等高阶原语、property、
codec、诊断捕获、interpreter、Host 和 service 均提供有明确所有者的结果区。
尾调用复用调用方的 output，不在尾递归链上增加结果副本。二参数数据注入与
materializer 辅助 ABI 保持独立，Wasm local 编号按实际参数数量计算。

Ready demand 在复制到调用方 output 前保存自己的持久结果，避免把调用方空间
缓存进全局状态。初始化入口和旧直接 ABI 测试可传 0，仍返回当前持久分配器中
的结果；后续 scratch 接入时须将该入口落实为 demand 的固定结果区。
内部 ABI 更新为 30。新增回归验证 1,000 层尾调用后返回参数值、普通调用交接、
提前返回 Ready demand 均产生调用方独立的完整值副本，同时原样保留来源。

此阶段仍使用 append-only words 分配这些空间，尚未释放 frame scratch，未启用
执行中自动 GC；调用方 output 的存在不能当作长期堆内存有界的验收。

阶段验收：完整 Wasm 回归 108 项通过，格式与 diff 检查通过。额外尝试的
Wasm Clippy warnings-denied 检查被未改动的 telora-data lint（let_and_return、
byte_char_slices、needless_borrow）及 RT 公共 unsafe 函数缺少 Safety 文档挡住，
未将该检查记录为通过。

### 2026-10-21：可复用纯值 scratch 与实际帧交接

布局求解器公开 least-fixed-point 的 plain 分类。生成 callable 的普通纯值构造、
纯值调用输出和临时参数包迁入独立 scratch：每个编译期分配点有固定槽，循环复用，
帧入口预留完整空间，普通返回/可恢复失败在交接输出后释放。Equal/Json raw
helper 同样清理帧。初始化 demand 与二参数 materializer 暂用持久 words。
非纯 Ready demand 的引用借用 demand 自己的持久值头，避免每次读取全局函数
都产生新的堆值头。闭包捕获和延迟 debug 事件为纯值复制完整拥有的副本。
预留的前向函数头仍使用持久存储，未引入 RC。

尾调用打包时复制所有纯值参数，持久非纯值保留引用，包内纯值引用使用独立
transfer 地址空间。RT 复制包后退出旧帧；callee 入口将包采用到自己的帧并修正
引用，避免全局 ping-pong 包被嵌套调用覆盖祖先参数。参数包、scratch 帧容量均
可复用。尾调用构造器的零环境分支与普通 invoke 一致，接收 callee 值以保留来源。
普通调用的 output 槽在交接前不清零，保证 fold 的上轮输出仍可作为本轮输入。
逻辑 scratch 与持久 words 的命名空间分离，words 限制在 0x80000000 以下；
内存上限仍由 Guest allocator/linear-memory limiter 执行，fuel 覆盖临时区操作。
内部 ABI 为 31。

定向验收覆盖共享协议 50,000 次尾交接容量稳定、嵌套 tail 不覆盖祖先参数、非法
交接与 reset；Guest 单次 10,000 轮纯聚合相互尾调用 managed heap 增长小于 256 字节、
正常返回帧深度 0，并验证值和来源。回调、捕获、debug、失败后继续与资源停机
使用相关用例验收；依用户要求，阶段不再重复全量测试，完整 RFC 后统一全量验收。

该阶段解决实际纯值临时区与尾交接；动态存储、其值头及关联资源仍由 append-only
words/现有边界复制器持有。准确根和自动 mark-sweep 尚未接入，不能将纯值长调用
的稳定存储测试当作 String/Array/Dict 等动态对象长期存活/回收的完整验收。

本阶段验收：核心布局 4 项、共享 scratch 3 项、相关 Wasm 用例 11 项通过；共享库
Clippy（all targets、warnings denied）、格式与 diff 检查通过。未运行完整 Wasm
套件。Wasm 定向用例含 pure aggregate、array/dict 回调、闭包、诊断捕获、debug、
可恢复失败帧清理、fuel/内存停机、interpreter 捕获及 variant 来源。

### 2026-10-21：真实 shadow frame、标记审计与 Host pin

内部 ABI 32 增加 root enter/leave/set/safepoint。生成代码发布封闭类型参数、
表达式结果、辅助调用结果及原始 storage；复用 local 和释放表达式临时槽时清空
对应根。普通退出、语言失败和尾调用交接均有真实根帧清理。数组 map/filter/fold、
flatten 的值指针计划、字面量和 spread、dict spread 以及 codec 数组发布初始化
前缀；指针计划使用 References 角色，不能伪装成内联 Array 值。

RT 可选择在 callable 入口做非移动标记审计，使用共享准确扫描器发现活动根图、
Ready demand、service handler/初始化输入及诊断/debug 事件。审计记录 storage、
对象/资源槽、content payload 和来源 ID；原始 storage 不解释其字节，Regex 的
Rust 指针不当作语言地址。**此阶段仅发现可达图，没有 sweep，没有自动回收。**
仍须核对 parser/codec 上下文、其余部分构造和 RT 内部拥有者，再以真实 sweep
验证根完整性；审计能通过不等于不存在漏根。

`Session::pin_value/pinned_value/unpin_value` 接入 Guest 根注册表，句柄绑定 Session
和单调 generation。边界复制回收重定位 pin，reset 使 pin 失效；槽复用不能使旧
句柄复活。Host 长期拥有值用 pin，普通 Value 仍是借用句柄。

定向回归发现并修复 scratch 阶段的两个遗漏：延迟 Fmt 节点需复制纯值操作数；
诊断事件需复制来源范围，不能保留失败退出后已释放的纯值头地址。在上一阶段
提交的隔离副本中复现 codec 失败，修复后相关五个用例通过。

验收使用 /tmp 隔离产物：共享根协议 5 项、共享库 all-target Clippy -D warnings
通过；Wasm 定向 30 项通过（Host pin、数组回调、codec、parser）；独立的
20,000 层尾调用/失败用例通过并断言活动根帧和根数量归零。该尾调用用例约 317 秒，
后续阶段不反复运行；完整实现时再做全量验收和性能评估。Host pin 回归在初始化
前 pin 嵌套输入，多次无显式根的边界回收后输入/结果仍正确，并覆盖 unpin、槽
复用后旧句柄及跨 Session 拒绝。

下一阶段接入可复用分配及 sweep，完善内容/资源/来源回收和制品元数据，再启用
执行期间收集，完成固定存活集远超预算的单次调用验收。

### RT 存储接入决策：分页元数据与连续 words backing

已有 Host words 视图和 snapshot/data image 使用连续逻辑地址镜像。接入时保留
分页/size-class/内部引用所属块查询及非移动逻辑引用，将原型每页独立的 Vec
改为一个连续 words backing；分页只管理范围和槽状态。这样 Host 继续通过
origin+offset 读取，边界复制器仍能取出旧 words 镜像，避免并存两个堆或让 Host
逐字段调用 Guest 转换地址。

空页槽和元数据可复用/合并，backing 保留容量以服务后续分配，按实际保留容量
计入 committed，不把中间空洞或 Vec capacity 伪报为已归还内存。预算检查包括
backing 与分页元数据；极端碎片可触发受控预算失败。冻结镜像前缀作为明确的
不可回收区域，恢复/边界复制后重建动态页元数据；静态前缀不凭字节猜测分配边界。
此适配先单独验证分配/标记/sweep、冻结/恢复及内部引用，根覆盖完成前不自动触发。

### 2026-10-21：RT 分页分配和显式 mark-sweep

分页核心已成为 RT words 分配器，连续 backing 保留现有 Host 视图。内部引用
通过页/槽元数据标记所属分配；空页合并及不同 size class 重用时拆分剩余范围，
避免每轮丢失大空闲范围。预算为 backing 保留容量与页/槽元数据的总和，包含
Vec capacity；分配失败不重解释现有对象。空页保留可复用 backing，不能把空页
没有活动对象报告为 backing 已释放。已接入的 words 预算为 32 MiB，仍有整个
Guest 的独立线性内存上限，不能把该数字当作所有资源总量配额。

显式 `telora_gc_collect` 完成标记、资源槽 sweep/复用、Regex 析构、动态来源
登记清理、内容存活区间复制与 payload 更新，以及动态 words sweep。对象和
pin 的逻辑地址不移动，内容仍通过现有 offset 协议压缩。Failed demand 的诊断
身份单独保活。自动安全点仍只审计，尚未启用自动 sweep，不能据此宣称单次长
调用已可在小预算下完成。

ABI 33 的 snapshot/data image 保存分配字节计数；恢复时明确保护镜像前缀，
从其末端重建动态分页。冻结后旧页槽不再参加动态分配，防止新分配错误复用已
冻结的页内空位。service reset 清空动态分页元数据、截断 words 并保留 backing
物理分配。有效分配字节与实际 committed 分开统计，不能混淆存活量与保留容量。

验收使用 /tmp 隔离产物：共享堆 6 项及共享库 all-target Clippy -D warnings
通过；Wasm 的 Host pin、两项真实 sweep、compiled data roundtrip 定向 4 项
通过；初始化压缩、请求 reset/缓冲区复用和 source reader 用例通过；10,000 轮
纯聚合尾调用及返回输出所有权仍通过。本轮未跑全量。数据制品的功能测试燃料
改为 20M，以容纳根/分页记账；专门的燃料停机测试不在本次预算调整中。

真实 sweep 回归覆盖 12 个窗口，每窗产生 32 份嵌套临时输入，固定 pin 图的
存活字节/保留空间保持一致，unpin 后存活量下降；另一个用例反复创建临时闭包，
同时 pin 一个捕获 Regex/Fmt 的闭包，验证资源槽中间空洞复用及最终全部释放。
这些是跨调用的显式 sweep 验收；执行中每个 may-GC 点的根覆盖、RT 大循环安全点、
单次调用远超预算及增长存活集停机仍待完成。

### 2026-10-22：执行中收集与构造/解析安全点

ABI 34 增加 parser/codec context 和 parser plan 根角色。上下文显式保活 path、
原始来源、共享 rejection cell/message/Blame；不能按一个普通 TypeId 猜测 raw
上下文字段。postorder plan 描述 rows、raw span、数组/字典 entry 和各自来源。
materializer 的值指针计划发布已初始化前缀，内外循环均有安全点；codec 数组
循环发布输出前缀后触发安全点，record keys/values 在嵌套 helper 前发布进度。
Regex 捕获缓冲区保留原始 storage。checker 在读取 demand 前发布实际参数的
封闭类型根，variant 的 payload 不能误标为完整 enum。

制品静态来源索引改为由代码镜像明确拥有。强制执行中收集暴露了此前仅根据
当时值图筛选来源的问题：将来才执行的代码位置仍可用于诊断，即使当时没有值
携带那个来源。动态来源登记仍按来源根和既有冻结边界清理。

`telora_gc_interval` 显式启用按累计分配字节触发的自动收集，支持每个合法安全点
都强制收集；默认仍为关闭，Host 标准配置、trap 生命周期和完整 runner/反馈
验收完成前不作为默认路径。`telora_gc_heap_limit` 可设置 words 预算，累计分配
与当前 committed 分开记录。Rust 解析器产出 plan 的阶段不调用用户代码，plan
占用属于当时真实存活的构造现场；materializer 的长循环可以回收已丢弃的值头。

本阶段定向验收包括：每个 callable 安全点强制 sweep 的 50 轮单次调用；对比
无 GC 与强制 GC 的 codec、嵌套 encode/parse/decode、Regex parse/check、诊断、
Fmt、sequence 和 dict 结果；256 KiB 预算下增长存活 Array 在燃料耗尽前停止。
共享堆/根协议 11 项和共享库 all-target Clippy -D warnings 通过。

较长的单次调用在 512 KiB words 预算下完成 10,000 轮含 String/Array 聚合计算，
累计分配 9,530,441 字节，约为预算 18 倍。初次测试的 20 倍估算断言偏高，已改为
16 倍并独立复跑通过（189.13 秒）；保留空间始终不超过 512 KiB。
阶段内仍未运行全量。

剩余验收：默认策略及 Host 所有者、trap 后终止/重置、反馈和测试调度根、服务
初始化/请求/快照和 data image 在默认收集下运行，以及完整全量验收。不能因已
证明单次调用空间复用就提前结束本 RFC。


### 2026-10-22：默认策略、Host 与服务生命周期

默认每累计分配 64 KiB 在安全点执行真实收集；测试间隔 1 对每个安全点强制
收集。Host 通过 `GcConfig` 配置间隔与 words 预算，配置覆盖新建实例和服务
snapshot 重建。默认 words 32 MiB，整体 Guest 64 MiB；二者范围分别计量。

Guest trap 后 Session 拒绝执行、分配和根修改；普通语言失败仍通过返回状态
传播。配额调用记录 trap，其他 Guest 调用共用终止守卫。服务使用初始化
snapshot 重建陷阱实例，普通 Session 重新加载。收集本身也受 fuel 限制，
定向测试在收集中耗尽 fuel 后验证实例无法重入。

测试 runner 的队列和当前任务均由 pin 保活，完成任务后释放，日常回收改用
非移动收集。Host 可在复制完反馈后显式 `release_feedback`，释放 Guest 历史
和借用该历史的临时错误槽；Failed demand 保留其独立诊断身份。只读历史接口
保留原语义。baseline 的字节保持不可变，服务 reset 可恢复初始化反馈。

强制收集验证发现 service initializer 与尚未填充的 source 身份缺少阶段所有者，
补齐 initializer 的闭合类型契约及所有 source 身份根，内部 ABI 为 35。
强制 GC 的 service 初始化、snapshot、连续请求、fuel trap、重建测试通过。
服务恢复测试的普通请求工作区已超过旧 1 MiB 基线，将固定引擎边界调整为
2 MiB，保留持续增长触顶与 fuel trap 两种停机断言，256 次 reset 容量稳定。

本阶段定向根/资源/反馈/停机 9 项通过（跳过已通过的 189 秒长计算），service
原有 7 项在 2 MiB 基线下通过，新增强制生命周期用例独立通过。CLI 嵌套 fixture
factory 队列验收通过。默认路径已接齐，下一步进行 RFC 完整验收及性能/风险
记录；全量结果出来之前仍保持 Implementing 和 draft。


### 2026-10-22：完整验收发现与阈值摊销

首次 workspace 全量完成：CLI 94/95、Wasm 117/119；其余 workspace 用例均通过。
三项失败都是 fuel 耗尽。递归格式化与 10,000 轮纯聚合测试提高功能验收配额，
保留输入规模和空间断言，不修改专用燃料停机测试。跨执行复用的 Host 借用值
在纯聚合测试中显式 pin。两项独立复验通过。

汇总语言 checker 的 50 亿 fuel 被耗尽，暴露大存活图下固定间隔过密的问题。
阈值改为收集后按 managed 占用的一半增长，至少为配置间隔，并由剩余 words
预算的四分之一封顶；1 仍强制每个安全点收集，0 仍关闭。10,000 个长字符串
的 pin 图保活期间进行 1,000 轮小对象计算，只收集 3 次，输出与 pin 图完整，
words committed 1,661,984 B、managed 965,546 B、线性内存峰值 6,422,528 B，
调用 fuel 460,203,532。该定向测试通过（60.18 秒）。汇总 checker 独立复验通过，
全部观察结果为 True：fuel 4,871,628,975，线性内存峰值 30,277,632 B，
请求耗时 570.92 秒；未提高 50 亿的功能验收配额。

首版收集只在发布完整根的安全点触发，底层分配失败终止，不在未发布的构造中
隐式重入 GC；已据此细化第 5 节的分配准入约定。GC 辅助空间由整体 Guest
内存限制约束，空间不足时终止；不承诺在硬上限处总能启动收集或完成诊断。

独立构建/发布 smoke 已通过，覆盖 LF/CRLF/CR、snapshot、普通制品 source
替换、snapshot 拒绝替换、资源停止后重建和发布失败原子性。HTTP/Unix HTTP
两种传输、compiler/runner 两个入口的正常请求、错误请求及重建 smoke 通过。
旧脚本已同步当前 crate 根声明、ServiceCollection 请求与路由和 locs 诊断协议。

1,000 轮固定小存活图对照（累计分配均为 953,441 B、结果一致）：

| 分配间隔下限 | GC 次数 | words committed | 线性内存峰值 | fuel | 耗时 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 关闭 | 0 | 1,870,548 B | 3,342,336 B | 164,501,101 | 21.10 秒 |
| 16 KiB | 58 | 176,272 B | 917,504 B | 164,296,449 | 19.03 秒 |
| 64 KiB | 14 | 254,240 B | 1,114,112 B | 145,857,291 | 17.96 秒 |

该表在阈值摊销前测得，小存活图仍使用配置下限。存在并行验收竞争，耗时不作为
速度提升承诺。managed 占用含未清扫垃圾，words committed 含容量和元数据，
其差值不单独等于碎片。共享分配器随机图/环/内部引用、不同尺寸空页复用和内容
切片的独立测试量化稳定窗口；共享库 36 项与源码规模检查通过。


解析 plan 现为一次消费的拥有者：物化每行后，packet 的 rows/count 前移到
未消费后缀，已消费的临时文本/子索引不继续作为 plan 根。独立的子节点使用
计数只服务于 plan 物化，最后一个父节点复制完成后清空临时值头根；共享节点
在最后一次消费前保持可达，实际语言图仍由 mark-sweep 管理。每个安全点强制
GC 的 codec/parse/check/Fmt/sequence/dict 回归通过，手工共享 plan 在中间 GC
后被多个父节点使用、最后只保留结果图的独立验收通过。第二轮 workspace
全量已启动，使用新的消费协议与阈值，并将语言 fixture 工作区放在 /tmp。


### 2026-10-22：反馈释放与失败身份验收

真实强制 GC 对照扩大为 21 组 fixture/export，覆盖 record、enum、递归闭包、
递归 codec、捕获、来源、Dyn 检查、property route/source/plan witness；125.22 秒通过。
完整 workspace 的 440 个语言观察全部通过。并行编译导致 CLI HTTP 的 2 秒启动
等待超时，调整为 10 秒；隔离单项和并行服务组 12 项复验均通过。

收尾调查发现 Failed demand 自身保活错误包，但 Host 解码仍依赖诊断历史。
现在可直接解码独立错误包，`failure()` 优先读取活动失败；显式释放反馈保留当前
活动失败事件，其余历史可释放。恢复边界清空活动失败后，历史全部可释放；Failed
包经 GC 后再次传播仍可报告。活动失败期间 `collect_work` 拒绝移动压缩，非移动
GC 可正常运行。反馈释放导出显式接收活动错误地址，内部 ABI 升为 36。

新增回归在失败初始化后反复释放/GC/重试，并模拟恢复边界释放历史后重新传播
Failed 包；另复验后续 debug 事件与 test boundary。合计 5 项通过。


### 最终验收归档

第二轮 `cargo test --workspace --no-fail-fast` 已执行完成：Wasm 122、core 207、
data 46、shared 36 及其余单元/文档测试全部通过；CLI 94/95，440 个语言观察
全部通过，唯一失败为上述已修复的 HTTP 启动等待超时。该命令保留失败记录，
不将其记作整条命令通过。最终反馈修复后复验 Wasm 115 项（跳过已通过的 7 项
长测试）、shared 36 项和反馈专项；CLI 另行复验除已通过语言长测外的全部 94 项用例，全部通过（91.65 秒）。

最终 ABI 36 的独立 build/run smoke、HTTP/Unix HTTP 双入口 smoke 全部通过；
shared all-target Clippy `-D warnings`、格式、diff 与源码规模检查通过。全仓库
Clippy 仍受 telora-data 既有 lint 与 RT 公共 unsafe 函数 Safety 文档告警阻挡，
不声明全仓库 Clippy 通过。构建、fixture 工作区和日志均在 `/tmp` 隔离。

验收证据对应关系：

| 范围 | 可重复证据 |
| --- | --- |
| 有界长期分配 | `one_invocation_allocates_far_beyond_its_bounded_heap`，512 KiB words 预算内累计约 9.53 MB；`nonmoving_sweep_reuses_pages_and_content_with_a_fixed_pinned_graph` 多窗口稳定 |
| 选择性/纯值/尾调用 | core 闭合布局分类，`inline_aggregates`、`expression_locals` 与 `tail_calls`；纯值 scratch 有界、深尾调用帧清理 |
| 准确根/初始化/property | 21 组强制 GC 对照，`consumed_parse_plan_keeps_shared_children_until_their_last_parent`，service 强制初始化/重建 |
| 共享/环/内部引用/碎片 | shared `gc_heap` 独立可达性模型、随机图、环、跨尺寸空页复用；`gc_roots` generation/尾交接 |
| 内容/资源/来源/反馈 | content/source_ranges、Regex 空槽复用和析构；失败包脱离历史后 GC 与重报回归 |
| Host/制品/服务 | pin 的多调用/旧句柄/跨 Session 拒绝，ABI 36 build/run 与双传输 smoke、snapshot 后强制 GC/reset |
| 硬边界/停机 | 增长存活图触及 heap budget，执行及收集耗尽 fuel 后禁止重入，服务 trap 后 snapshot 重建 |

首版边界：baseline 与代码镜像固定存活；words/content 保留容量在 Guest 内复用，
不承诺归还线性内存。辅助空间受整体 Guest 限额约束；硬上限处可直接终止，不保证
总能回收成功或完成诊断。分配失败不在未发布对象中重入 GC。ref count 与语言缓存
均留待后续设计，本次实现准确非移动 mark-sweep。
