# 静态类型值、紧凑来源与 Wasm 栈式调用约定

- 日期：2026-09-18
- 阶段：讨论稿；不是已接受的 ABI 或实施承诺
- 背景：[#213](https://github.com/hh9527/telora/issues/213)、main `5b1833f1`
- 关联：[早期布局讨论](mir-directed-runtime-layout.md)、[RFC 0300](../rfc/0300-host-guest-abi-and-location-ids.md)、[RFC 0302](../rfc/0302-single-vector-language-heap.md)

本文记录布局迁移的设计推导。RFC 0304 已接受该方向；Issue #216 的分支正在实施，
以下“当前实现”小节保留的是提出方案时的 ABI 25 基线，不代表分支最新状态。

## 1. 问题不止是 local 数量

#213 的已复现用例在一个初始化函数中构造 13 个适配器，每个 operand 构造并捕获 600 元素数组。数组逐项求值、通过操作数栈立即消费结果并复用临时 locals 后，该函数从 31,607 个 locals 降至 355，结果仍为 7800。

这验证了缩短临时存储生命周期的价值，但没有消除大多数元素的临时堆对象。当前表达式通常先构造完整语言值，再将其地址传给消费者；存入容器时可能再次复制。

提出方案时的关键事实：

- 共享 ABI 版本为 25，值头部为 `src/start/end: u32 × 3 + TypeId: u32`，共 16 字节；标量为 24 字节，String 为 32 字节。
- 普通调用路径构造值地址数组，经统一调用接口传递；这不是直接的类型化 Wasm 参数签名。
- 结构化对象位于语言堆 `Vec<u64>`，外置字符串/字节内容位于 `Vec<u8>`；分类表仍存在。语言引用必须经堆地址换算，不能当作跨分配稳定的物理指针。
- 回收器遍历 record、array 等对象时，目前从每个值头部读取 TypeId 和宽度；环境中保存的值引用也依赖自描述值。
- #213 的改动先覆盖无 spread 的数组元素消费，尚不是所有表达式的栈式改造。

对应实现入口：`crates/telora-wasm-shared/src/abi.rs`、`crates/telora-wasm/src/functions.rs`、`crates/telora-wasm/src/stack_values.rs`、`crates/telora-wasm/rt/collect_trace.rs`。

新的目标是：类型在静态阶段确定；计算时直接使用机器值；持久保存时才按布局物化；来源随值传播，但不强制与数据组成统一堆对象。

## 2. 三个相互独立、可以组合的方向

1. 将位置压缩为一个 64 位字：`src-id:14, start:25, end:25`。
2. 调用时将来源与数据分组：`loc-a, loc-b, loc-c, data-a, data-b, data-c`。
3. 静态已知的值不普遍携带 TypeId，栈上和堆中采用同一原则；优先考虑容器保存一次类型，普通元素不重复保存，Dyn 元素除外。

不能把三者混成一个“新 Val 大小”的决定。来源编码影响来源容量；调用约定影响计算调度；去掉 TypeId 影响存储、动态边界和回收。讨论中已接受 14/25/25 的容量边界，其他方向仍需分别验证。

## 3. 紧凑位置：保留字节范围，不重做 LocId 表

候选编码如下，位序只是便于讨论的定义：

```text
bits 63..50: source id
bits 49..25: start byte offset
bits 24..0 : end byte offset（exclusive）

packed = (u64(src) << 50) | (u64(start) << 25) | u64(end)
```

| 字段 | 编码容量 | 需要明确的边界 |
| --- | --- | --- |
| src-id | 0～16,383 | 保留无来源/临时来源标识后，可注册来源数更少 |
| start/end | 0～33,554,431 | 必须满足 start ≤ end；EOF 也要能表达 |

若要求来源任何位置都能表达，文件最大长度是 `2^25 - 1` 字节，不是恰好 32 MiB：长度为 `2^25` 时 EOF 已不可表示。空范围允许 start == end，不与“没有来源”混同。

### 来源分类与容量

静态源码、静态数据模块和 `--source` 保留现有来源语义。源码保留 BOLs，注入数据解析并保留对应来源槽位的 BOLs；诊断显示时再将原始字节范围转换为行和 UTF-8 字节列。

临时字符串 parse 仍可以只报告相对 start/end，不为每次解析注册长期来源。其特殊 source 编码、无来源编码及零值必须统一约定；不能将不同临时输入的相同偏移误当作同一已注册文件的位置。

静态来源和注入槽位应共享明确的容量预算。服务反复接收请求不意味着不断增加来源 ID；重置及替换注入内容仍应遵循已有生命周期限制。

讨论中已确认：约 32 MiB 的单来源容量和约 16K 个来源槽位足够当前定位使用。采用 `2^25 - 1` 字节及 14 位来源编号的明确边界；特殊编号会减少可注册来源数。该限制不是项目总大小或运行时堆大小的限制。超限明确报错，不静默截断，不引入溢出位置表。

所有来源接入路径都要检查：静态文件、数据模块、注入数据、动态 parse。失败应是明确的容量诊断，不是位操作回绕。BOLs 的元素、长度也不能因此隐含缩成 25 位。

### ABI 和显示

Wasm 内将位置作为不透明 i64，移位和掩码按无符号位模式处理；落盘/线性内存字节序明确为 little-endian。JavaScript 通过 BigInt 或两个 u32 解码，不通过 Number 承载完整 64 位值。Host JSON 诊断仍可输出独立的 source/start/end 整数，不必暴露 packed 数值。

该编码不会归一化 EOL：偏移仍相对于编译或解析实际接收的字节串。build 输入的 EOL 归一化与这里是两个问题，BOLs 必须和对应输入一致。

## 4. 区分逻辑值、计算表示和存储表示

逻辑上，一个静态已知类型 T 的值仍包含：

```text
TypedValue<T> = { origin, data<T> }
```

T 是编译器知道的上下文，不意味着每个值都保存 TypeId。逻辑上有来源，也不意味着每条指令都要读写一块完整头部。

建议由 sealed 类型布局导出三种描述：

| 描述 | 内容 | 消费者 |
| --- | --- | --- |
| StorageLayout(T) | 大小、对齐、字段来源位置、数据偏移、引用字段 | 容器构造、字段访问、复制 |
| MachineShape(T) | Wasm 参数/结果的有序机器类型序列 | 函数签名、表达式 lowering |
| TraceLayout(T) | 存活边、动态标签解释、内容切片及资源追踪规则 | copy-collect、根重定位 |

它们来自同一份封闭布局事实，而非三套独立猜测。机器表示与存储表示可以不同，但转换必须是确定的 pack/unpack。

例如 Int 的数据可以是一个 i64，Float 是 f64，Bool 可在机器栈上用 i32；无需为“统一 word”强迫全部 locals 使用 i64。String/Bytes 维持既有短内容内联和长内容切片语义，机器形状可以是多个 word。数组、record 的数据则是明确布局的引用或切片描述。

Unit 没有数据 payload，但可能仍有来源；Never 表达没有正常返回路径，不分配一个假值。不能因为二者“都没有业务数据”而混同。

## 5. 参数来源与数据分组

假设 foo 的三个参数均为单 word 值，候选调用约定为：

```text
foo(loc_a, loc_b, loc_c, data_a, data_b, data_c)
```

多 word 参数按签名展开，例如第二个参数为两个 word：

```text
foo(loc_a, loc_b, loc_c, data_a, data_b0, data_b1, data_c)
```

接收者不能根据值内容猜测宽度。隐藏闭包环境参数和必要的计算位置参数必须在签名描述中另有固定位置，不能混入最后一个业务参数而靠运行时猜测。

### 为什么有帮助，为什么不能承诺“无需 locals”

对于直接内置运算，数据连续位于栈顶，可以直接使用 Wasm 指令：

```text
loc_a, loc_b, a, b  --i64.add-->  loc_a, loc_b, sum
```

但此时不能直接删除 sum 下面的两个位置。成功结果通常要使用计算位置；溢出等错误可能要保留输入来源。Wasm 没有任意 swap，仍需要规划输入位置的保存和结果组合，或使用少量 scratch locals。Telora 的 checked 算术也不能用一次会回绕的 i64.add 取代全部语义。

如果表达式 a、b、c 自然产生 `(loc, data)`，顺序求值首先得到的是交错排列。分组参数不自动由这个排列得到。不能先遍历所有位置、再遍历所有数据而重复执行子表达式；函数返回位置可能依赖运行时分支。

建议将“边界 ABI”与“表达式生成调度”分开：

- 已知的常量来源可以在需要时直接生成。
- 转发来源可从已保存的绑定取得，不需要重新执行原表达式。
- 运行时才确定来源的子表达式只执行一次，必要时将其结果短暂保存，再组成调用栈。
- 命名绑定、多次读取、跨分支存活的值允许使用 locals；目标是少而必要，不是零 locals。

考虑 `foo(f(), g())`：必须遵守现有顺序执行 f、g。若 f 失败，g 不执行。即使数据操作是纯的，诊断、失败和资源陷阱仍可观察，不能借参数分组任意重排。

### 已确认方向：独立传递计算位置

需要调用点位置的内置计算/RT 接口，使用独立的隐藏参数，逻辑顺序为：

```text
compute_loc, loc_a, loc_b, data_a, data_b
```

compute_loc 表示此次计算的位置，参数 Loc 表示输入来源。产生新值时结果采用 compute_loc；转发输入、字段或已有子值时保留其原始来源，不因经过接口就改写位置。失败诊断分别记录计算/报告位置与相关输入来源。

普通用户函数内部的计算位置通常直接由源码生成常量，不向所有函数普遍增加 compute_loc。需要此参数的内置能力在封闭的 ABI 描述中明确声明，不能到运行时猜测。同一个语言函数类型若既可指向用户闭包又可指向内置函数，需由类型化胶水提供一致的间接调用签名，并在确实需要调用点来源的路径上明确传递它，不能直接混用不同 Wasm 签名。

该决定确定了参数的语义与使用范围；内置函数作为一等值经过闭包/高阶调用时，调用位置如何由统一调用入口传至适配器，仍需在调用 lowering 原型中验证。

### 直接调用、间接调用与 Rust RT

直接调用可采用具体 MachineShape；同一封闭函数类型的所有闭包必须具有相同调用签名，才能通过 call_indirect 调用。不同目标函数的环境布局由目标身份确定，不能只凭公共函数签名解释捕获环境。

Rust 编译的 RT 不必为每一个 Telora 泛型实例生成实现。小而固定的 RT ABI 配合 codegen 生成的类型化胶水即可；确定类型的调用不应为了复用旧接口再次普遍装箱。

讨论中已确定：codegen 生成的内部函数普遍使用 Wasm 原生 multi-return，直接返回来源和具体类型的数据 words；直接调用、间接调用及其适配器使用一致的封闭签名。涉及 Rust 编译的 Guest RT 或 Host/Guest 边界时，采用 single-return 接口，不假定 Rust extern C 自动匹配原生多结果签名。

### 边界返回栈：输入仍是显式参数

边界使用 Guest 线性内存中的固定容量返回栈。它只承载多值返回，不承载输入参数，不借用 Rust 编译器的栈指针，也不替代 Wasm 操作数栈或语言堆。

```text
调用前：return_top = base
调用：  status = boundary(arg0, arg1, ...)
成功后：[原有返回栈内容 | result0, result1, ...]
                       ↑ base                ↑ return_top
```

输入通过普通 Wasm 显式参数传递。成功时，被调用方在返回栈追加已知签名规定的结果；调用方读取后将栈顶恢复到 base。新增高度是返回数据的 ABI 存储宽度（包括对齐），不涉及扣除参数宽度。不能把任意一段新增内存解释为结果：偏移、字段机器类型及大小都由签名确定。

约定如下：

- 每个 Guest 实例独立持有返回栈，首版按 8 字节槽位/对齐制定布局；总容量待原型测量后确定。
- 边界原生单返回值表达成功/失败状态，语言值的多 word 结果放在返回栈中。
- 预留空间先检查容量，不能越界写入；失败或 trap 后由调用方恢复保存的 base，不消费不完整结果。
- 正常读取结果后释放这次新增区域；指向该区域的借用不能逃逸。变长内容只返回描述符，其所有权由具体接口规定，恢复栈顶不自动释放描述符指向的内容。
- 嵌套调用分别保存和恢复基线，内层返回区不能覆盖外层仍存活的结果。返回栈支持嵌套不等于服务状态自动支持任意重入。
- Host 不跨可能导致线性内存增长的 Guest 调用缓存内存视图；所保存的偏移与访问时取得的视图区分开。

codegen 胶水负责将原生 multi-return 与返回栈协议互相转换。结果不需要先构造成通用语言堆对象，也不因边界采用 single-return 而限制内部函数。

### 已确认方向：统一显式失败状态

内部正常可返回的函数首版统一采用：

```text
(status: i32, loc: i64, data...)
```

status 为 0 表示成功，非零表示失败。调用方只在成功时消费来源和数据；失败时传播状态，保留已记录的诊断，不重复报告。不能继续用空引用作为所有返回值的失败哨兵：Int 的 0、Bool 的 false 和 Unit 都是合法成功结果。

失败路径仍按静态 Wasm 签名提供各结果槽的占位位模式，但它们不是语言值，不应为此分配堆对象、创建来源或执行构造检查。调用方即使需要用 locals 暂存 multi-return，也必须先判断状态再解释数据。Unit 成功返回 status 与 Loc；Never 不存在成功返回，其失败传播如何落到具体机器签名需在 lowering 中明确，不制造假的 Unit 成功值。

边界 single-return 使用相同的 0/非零状态含义，成功结果（Loc 和数据）写入返回栈；失败时不消费新增区域，恢复调用前基线。Wasm trap 仍是独立的引擎退出路径，由调用边界恢复状态，不能假定 trap 会返回这个 status。

首版不按“可能失败/不可能失败”拆分调用约定。若未来有充分的静态证据，可以单独优化无失败函数；具体非零状态码的分配不在此决定。

## 6. 去掉普遍的 TypeId，但保留真实动态边界

| 场景 | 布局依据 | 是否逐值保存 TypeId |
| --- | --- | --- |
| 已封闭函数参数/返回值 | 函数签名 | 不需要 |
| record/tuple 字段 | 外层类型的字段描述 | 不需要 |
| Array(T) 元素 | T 的布局与步长 | 不需要 |
| Dict(K,V) 两列 | K/V 布局及既有有序键规则 | 不需要 |
| 静态 enum | enum 布局 + variant tag | 不需要重复 enum TypeId |
| 闭包捕获 | 目标函数对应的环境描述 | 不需要每项自描述头部 |
| Dyn / 未来 trait object | 实际具体类型/实现身份 | 需要足够的动态身份 |
| Type 值 | 所表达的类型身份 | TypeId 本身就是数据 |

Value 是具有已知定义的语言类型，不应只因名称为 Value 就等同于任意 Dyn。其 variant tag 足以选择已定义分支，具体分支是否有动态 payload 取决于类型定义。

不同名义类型可以有相同物理布局，但不因此具有相同类型身份。类型比较、反射、property 查询和动态适配仍使用 MIR 的稳定类型身份。去掉冗余存储不意味着运行时再次推断类型，也不意味着取消动态检查。

逐类讨论已确定 Dyn 使用 16 字节内联 payload，超出时装箱；enum 按最宽分支安排 payload，仅在尚无容器引用打断的内联递归环上增加间接存储。具体约定见下文。本轮不引入 niche encoding，也不假定任意 payload 都能塞进两个 word。

## 7. 堆布局和直接构造

### 优先考虑的折中：容器自描述，元素由容器描述

不必一步走到“堆对象完全没有类型标签”。可以让容器保存一个 TypeId 或明确的布局身份，容器内的普通元素不再逐项保存类型。动态元素则保留各自实际的具体类型。

```text
Array(Int) 容器：{ container_type, length, storage, ... }
元素区：         [loc0, int0] [loc1, int1] [loc2, int2] ...

Array(Dyn) 容器：{ container_type, length, storage, ... }
元素区：         [loc0, concrete_type0, payload0] ...
```

这里的 Dyn 图示是语义示意，具体容量见下文逐类布局。container_type 表达完整 Array(Int)/Array(Dyn) 类型，或等价的固定布局身份；无需再同时重复保存可由它查得的 element_type。

| 容器 | 保存一次的信息 | 元素/字段如何解释 |
| --- | --- | --- |
| Array(T) | 完整容器类型 | 所有普通元素按 T 的固定布局 |
| Record / Tuple | 完整容器类型 | 每个字段按字段表，允许异构但静态已知 |
| Dict(K,V) | 完整容器类型 | 键、值分别按 K/V；沿用有序键表示 |
| 闭包环境 | 环境布局身份，或可唯一映射到它的函数身份 | 每个捕获项按环境描述 |

“除非是 Dyn”针对的是具体类型身份：Array(Dyn) 的容器只知道元素是 Dyn，无法知道每个元素装的是 Int、String 还是某个 record，因此各元素还需要实际类型标签。静态 enum 仍需要 variant tag，但不是每个分支再附上冗余的 enum TypeId。将来的 trait object 同理需要足够的动态分派身份。

嵌套容器允许各自在自身对象头部保存类型。例如 Array(Array(Int)) 外层描述其元素是数组引用，内层数组对象各自保存自己的容器类型；不要求外层的每个引用再重复附带内层 TypeId。这接受少量冗余，换取每个容器可独立解释。

需要区分底层存储对象与 slice 值。共享同一底层数组的 slice 可从同一个容器头取得类型，只在视图中保存范围和自身来源；不必复制整个容器头。已确定以对象头作为容器类型信息的权威位置，不在分类表另存一份相同的类型标签。

这一方案的主要收益是：类型标签成本按容器数增长，而不是按元素数增长；同时回收器到达容器后可以直接获取布局。小容器的头部成本更明显，大数组的收益通常更有意义，具体字节数必须在对象布局确定后统计。

### 需要重审：heap 直接使用稳定的 Wasm 地址

讨论早期确认过 `heap: u32` 是单一语言 `Vec<u64>` 内的逻辑字节偏移，由当前
基址换算成物理地址。基于新的生命周期假设，这个物理选择需要重审：阶段内只
单调分配，service 就绪时只做一次跨实例 compact，请求结束整体 reset，因此普通
对象可以直接使用稳定 arena 分配得到的 Wasm 线性内存地址。

```text
heap: u32 = 当前实例线性内存中的对象地址
```

Wasm `memory.grow` 不改变已有数值地址，只会使 Host 取得的 memory view 失效。
生成代码可以直接使用 `heap + field_offset`，不再经过 `WORDS_ORIGIN` 和
`telora_heap_address`。跨实例 compact 或 snapshot restore 时，collector 通过
`old_address -> new_address` forwarding map 修补全部存活引用；地址从不要求跨
实例稳定。

对象仍按 8 字节对齐，0 保留为空引用。静态 image 地址在同一 module 的不同实例
中可以保持，动态地址必须重定位。Host 只把地址视为带实例语境的 u32 offset，
不能转换为跨 Guest 调用长期保存的 Host 裸指针或 memory slice。

候选 allocator 直接管理 Guest 线性内存中的 chunk，并在 chunk 内永远向后排。
它不要求每个对象进入 Rust global allocator，也不是一个会整体扩容搬迁的 Vec。
这里的三个名称首先表示生命周期阶段，不要求实现成三个通用 allocator：

```text
InitializationArena: 单调增长，迁移完成后随旧实例销毁
FrozenPrefix:        新服务实例中 compact 后的只读地址前缀
WorkRegion:          frozen 末端 checkpoint 后单调增长，结束时整体 reset/reuse
```

初始化实例只有一个单调 arena。跨实例 compact 时，存活图直接分配到新实例的
arena 底部；迁移完成后记录 frontier 作为 frozen checkpoint，随后同一个地址空间
的尾部就是请求 work region。请求结束只恢复 frontier，并清理相应资源表尾部，
不遍历普通语言对象，也不逐对象析构。因而“frozen/work”是一个实例中由 checkpoint
分开的前缀和后缀，不是两套需要相互协调的堆。

chunk 只解决 bump 区域扩展时已有对象地址不能移动的问题。chunk 元数据、空闲尾部
和跨 chunk 对齐均不可被语言观察；实例销毁或 checkpoint reset 才批量回收。若原型
证明可直接从线性内存连续增长且永不搬移，则 chunk 甚至可以退化为一个 frontier，
不应为了抽象完整性预先引入逐对象 allocator。

普通 Record、Tuple、Newtype、closure environment 和 raw container header 可以
直接用地址引用，不再需要 RECORDS/NEWTYPES/ENVIRONMENTS 等普通对象定位 table。
Array/String 的 backing buffer 可独立分配，但由稳定 header 间接引用。VALUES 等
动态装箱能否同样改为直接地址，由 Dyn/enum 的最终布局决定。Regex 等具有 Drop
或外部状态的 RT 资源仍使用专用 handle table；根表、类型布局表和资源表不会因
普通对象 table 消失而一并取消。

这是对早期“逻辑 heap offset”物理选择的修订候选，尚未接受为新 ABI。原型必须
先证明 arena 与 Rust allocator/交换缓冲区的线性内存区域不冲突、reset 后地址不可再达，以及
跨实例 collector 能完整修补共享和循环关系。

### 已确认方向：空容器保留有效引用

空 Array、Dict、Record 等仍使用有效的底层容器引用，不将 0 解释为空容器。空数组保存类型和 `len=0`；空字典的两列分别指向有效的空数组容器；零字段 Record 仍有类型头。头部大小另计，不能因为没有元素而省略解释对象所需的信息。

零长度 slice 保留有效的底层引用，并满足 start=end 及范围约束；回收必须仍能追踪和修补该引用，不能将其当作空指针跳过。是否裁剪不再可见的底层存储属于独立回收优化，不能留下悬空引用。

Unit 是明确的例外：只保存 Loc，不分配空容器。首版不为其他空容器建立跨构造共享的单例；以后若考虑共享，必须先核实可观察身份、来源和生命周期语义。

### 已确认方向：容器头部保存类型

```text
固定形状容器：{ type_id: u32, fields... }
数组 raw：     { type_id: u32, data: u32, len: u32, cap: u32 }
字节 raw：     { data: u32, len: u32, cap: u32 }
闭包环境：    { environment_layout_id: u32, captures... }
```

这些是逻辑字段，具体偏移与 padding 按统一的 8 字节对齐规则生成。固定容器的字段数和大小来自类型布局，不重复存储；raw header 使用稳定地址，data 指向可替换的 backing，分配大小由 cap 和元素步长计算并检查溢出，只有 len 范围已经初始化。

闭包环境使用明确的环境布局 ID，不将其冒充用户态 TypeId；函数身份仍可映射到该布局，并应与实际环境头一致。回收器由进入对象的引用种类区分普通容器和环境，再读取对应身份空间中的布局，不能将两种 u32 ID 无条件混用。

值的整体 Loc 位于引用值中，字段 Loc 位于各字段中；容器头不再重复保存一份整体来源。copy-collect 可以从对象头取得遍历所需布局，但仍需保持共享对象去重、引用种类和动态值身份的既有约束。普通对象采用上述偏移引用直接定位，类型信息以对象头为权威来源。

### 已确认方向：来源与数据相邻存储

首版采用相邻存储：保留值的来源，将通用头部从 16 字节改为 8 字节的 packed loc，数据按静态布局紧随其后。调用时来源与数据分组，堆内暂不分列。此处记录讨论中已确认的方向，不表示运行时已经完成修改。

```text
计算中： origin + data words       （Wasm 栈或必要的 locals）
容器内： [origin: u64][typed data]  （类型来自容器布局）
容器头： TypeId / 布局身份 + 必要管理信息
```

这里的 8 字节头部是元素/字段的来源，不包括每个容器额外保存一次的类型和管理信息。容器自描述方案并不要求每一个临时标量也独立装箱成一个自描述对象。

在继续按 8 字节对齐、现有数据宽度不变的假设下：

| 值 | 当前大小 | 候选大小 |
| --- | ---:| ---:|
| Int / Float / Bool | 24 B | 16 B |
| 16 B 数据描述的 String/Bytes | 32 B | 24 B |

这些仅是单值布局算术，不是整个程序内存收益预测。容器元数据、共享对象、仍需保留的资源表、Rust 资源和容量余量仍占内存。Bool 已确定逻辑数据为 u32，完整值经 8 字节对齐后仍占 16 字节。

每个字段和数组元素仍需自己的来源，容器也保留整体来源。`b = a.field` 转发字段来源；新建容器的整体位置不能覆盖元素来源。无 payload variant 从类型域物化时记录使用位置；`raise!` 的报告位置与被报告值的来源分别保存。

物化应发生在有必要的边界：被捕获、写入容器、保存为顶层/属性结果或进入动态容器。若最终目标已知，可直接写入目标字段/元素，而不是先分配完整临时值再复制。转发绑定和字段访问不应为了新接口自动产生新的来源或堆对象。

递归类型仍需间接边界；“不带 TypeId”不等于把递归 record 全部内联展开。数组切片、字符串内容共享、字典有序键和函数可观察身份都必须保持。

### 已确认方向：闭包环境直接保存捕获值

闭包环境是固定布局的容器，按已知偏移直接保存各捕获项的来源与数据，不普遍保存指向独立值包装对象的地址。标量直接复制来源和数据；array、record 等间接类型复制引用描述，继续共享底层对象，不深复制容器内容。

环境布局由具体函数身份取得，不能只由函数签名推断。同一个签名的不同闭包可以有不同捕获布局；若环境独立成为根，则该根或环境头必须提供布局身份。每次闭包构造的可观察身份也必须保留，不能仅因捕获内容相同而合并。

递归及相互递归的局部函数需要稳定的引用边界。可以先预留闭包身份和环境存储，再填充相互引用；捕获边指向这些稳定对象，而不是复制一个尚未初始化的函数占位值，也不按值递归展开环境。构造完成后才形成可调用的闭合环境，初始化前调用的诊断语义仍须保持。这种构造期填充不意味着允许任意用户态可变捕获。

具体采用独立闭包对象还是递归组共享环境，留待 Fn 数据布局确定；该例外不要求普通标量捕获也增加一层间接访问。回收器需能按环境布局追踪这些共享、成环的引用。

### 逐类确认的逻辑布局

先描述逻辑字段，物理存储统一按 8 字节对齐、补齐。下表大小包含一个 8 字节 Loc，但不包含引用所指向的容器、环境或外置内容。机器栈上的字段类型按计算需要选择，不因物理对齐而一律扩大为 i64。padding 不参与语言值比较、哈希或身份判断。

| 类型 | Loc 以外的逻辑数据 | 逻辑总大小 | 物理总大小 |
| --- | --- | ---:| ---:|
| Int | i64 | 16 B | 16 B |
| Float | f64 | 16 B | 16 B |
| Bool | u32，规范化为 0/1 | 12 B | 16 B |
| String / Bytes | 16 B 数据描述 | 24 B | 24 B |
| Array(T) | heap/start/end，各 u32 | 20 B | 24 B |
| Record / Tuple | heap: u32 | 12 B | 16 B |
| Newtype | heap: u32 | 12 B | 16 B |
| Dict(K,V) | keys/values，各 u32 | 16 B | 16 B |
| Fn | function/env，各 u32 | 16 B | 16 B |
| 无 payload enum | tag: u32 | 12 B | 16 B |
| 带 payload enum | tag + 最大分支 payload | 依布局确定 | 按 8 B 对齐 |
| Dyn | concrete_ty: u32 + 16 B payload | 28 B | 32 B |
| Unit `()` | 无数据 payload | 8 B | 8 B |
| Type / TypeOf(T) | type_id: u32 | 12 B | 16 B |
| Regex 等 RT 资源值 | handle: u32 | 12 B | 16 B |

物理字段偏移由布局器按对齐规则确定，不直接把逻辑字段宽度相加作为访问偏移；尤其 enum 的 tag 与 payload 之间可能需要 padding。来源和数据在机器调用中的分组也不等同于它们在堆中的字节顺序。

**标量。** Int 在 Wasm 上使用 i64，Float 使用 f64，Bool 使用 i32。Float 的非有限值及算术失败语义保持现有语言规定，不借布局改变运算规则。

**Unit / Never。** Unit 只保存来源，不分配空容器；Never 不存在正常值或正常返回路径，不生成占位存储。二者不能因为没有数据 payload 就混同。

**Type / TypeOf(T)。** 两者首版采用相同布局，TypeId 是所表达类型的身份数据，不是通用头部。TypeOf(T) 即使静态已知 T，也不另建第二种存储表示；纯计算路径上的常量消除可以随后优化。

**Rust RT 资源值。** Regex 等采用 `{loc: u64, handle: u32}`，物理大小 16 字节，资源对象本身的内存另计。handle 指向相应资源表，具体表由静态类型确定，不为普通值重复保存 TypeId。复制只复制句柄，不能按语言堆字节复制实际 Rust 对象。copy-collect 保活资源并在句柄重编号时修补全部引用；reset 释放请求期间新增且不再需要的资源，初始化保留资源遵循冻结基线。资源的构造、销毁与可观察身份沿用各自语义，不把有状态资源隐式当成可变的共享冻结对象。进入 Dyn 时保留具体类型及句柄。

**String / Bytes。** 数据描述统一为 16 字节：少于 16 字节的内容内联，最多 15 字节，另保留长度和 inline/heaped 标记。早期候选的长内容使用 `{start: u32, end: u32, raw_start: u32}` 指向统一 content Vec；在稳定 arena + internal mutable buffer 假设下，应改为与 Array 对称的 `{raw: u32, start: u32, end: u32}`，raw 指向 `{data, len, cap}` 的稳定 RawBytes header。String 路径保证 UTF-8 边界，Bytes 没有该约束；inline 转 raw、String/Bytes 是否允许共享同一 raw，以及标记位编码仍需原型确定。

**Array(T)。** heap 指向底层 RawArray，raw 保存一次类型和已初始化元素数。start/end 是半开区间的元素索引，满足 `start <= end <= raw.len`；步长由 T 的物理布局确定。slice 共享底层 raw，普通元素只保存来源和数据，Dyn 元素额外保存具体类型。

#### 待验证候选：不可变 Array 视图与内部可变 raw storage

语言层的 Array、slice 和所有别名继续是不可变值，但这不要求其底层 raw
storage 在运行时也完全不可变。一个值得原型验证的表示是：

```text
ArrayValue = { loc, raw, start, end }
RawArray   = { type_id, data, len, cap }
```

其中字段正式沿用头部定义中的 `len/cap`；下例中的 `raw.len` 就是已初始化前缀，
不是另一个需要同步的 frontier。`start/end` 属于不可变的语言值；`len/cap` 只属于 RT，不参与相等、
哈希或任何语言观察。RT 只允许在所有既有视图都不可见的尾部写入。于是：

```text
old = raw#7[0..2]
new = push(old, x)

old = raw#7[0..2]
new = raw#7[0..3]
```

只要 `old.end == raw.len`，写入 `len` 所指向的下一个元素不会改变 old 可见的 `[0, 2)`，
所以不需要通过引用计数或线性性证明 old 已经没有别名。容量不足时可以分配
更大的 backing storage、复制已初始化前缀并更新稳定 RawArray header 的 data；
所有旧视图仍通过同一个 raw 地址读取相同前缀。

从历史视图分叉时必须退化为复制：

```text
old = raw#7[0..2]
a   = push(old, 3)  # raw#7.len = 3
b   = push(old, 4)  # old.end != raw.len，复制 old 的可见范围到 raw#8
```

该规则也适用于 `start > 0` 的尾 slice：只要 end 等于 raw.len，就可继续共享
追加；复制分支时只复制 `[start, end)`，新 raw 从零开始。被放弃的尾部暂不
回退 len，因为没有唯一性或活性证明；它只造成有界于该 raw 历史的空间
保留，不影响不可变语义。

现有 ABI 25 已具备部分形状但不能直接完成此优化：Array 值的 `DATA` 区已经是
`array_id/start/end/unused` 四个 u32，ARRAYS table slot 却只有
`payload/bytes`，其中 payload 直接指向等长元素区，bytes 同时承担分配大小和
GC 扫描范围。每次 `array_result` 都创建新的 table slot，`array.push` 则分配
`n + 1` 元素区并完整复制。因此不能只把 unused 字段改名为 cap；cap
和 len 必须属于共享 raw，而不是属于某个 ArrayValue。

新的稳定容器头正好可以承载 type_id、data、len 和 cap。几何扩容时
raw 地址保持稳定，只更新其 data；元素访问每次经 raw header 解析当前 backing，
不能跨可能扩容的调用缓存 backing 地址。header 自身位于单调 arena，不会移动。

生命周期还需要一个严格边界：静态镜像或初始化后 frozen 的 raw 不得在请求
期间原地扩展。reset 只恢复 work checkpoint，不撤销 frozen header 的
data/len 变化。因此 push 遇到 frozen raw 时必须先 fork 到 work region；后续
沿最新尾视图的 push 才可在该 work raw 上追加。初始化 compact 前创建的 raw
可以使用内部追加；迁移到新服务实例后它们成为只读基线。

copy-collect 应按 raw identity 去重，只扫描并复制 `len` 以内的已初始化元素，
不读取或追踪 cap 空间。目标 raw 的 cap 是独立策略：可以保留原 cap、
收缩到 len，或按增长策略重新选择；保留 cap 只分配目标余量，不复制未
初始化内容。多个 slice 指向同一 raw 时只复制一次 raw，并修补稳定的 raw 引用。
进入不可再追加的 frozen/发布基线时保留余量通常没有收益，可以收缩；仍处于可变
work 生命周期且预期继续追加时则可以保留余量。该选择不能改变语言可观察结果。
元素仍按 `TraceLayout(T)` 追踪，append 写入的是包含元素 Loc 的完整存储布局。

这个候选体现一条更一般的实现原则：HIR/MIR 保持不可变值语义，codegen/RT
可以使用不会被语言观察到的受约束 mutable instruction。候选指令可区分为：

```text
raw_array_append_tail(raw, element) -> new_end
raw_array_fork_append(raw, start, end, element) -> (new_raw, new_end)
```

普通 `array.push` 的 lowering 或 RT 胶水根据 `end == len` 和 frozen 条件选择路径；
Mutable raw handle 不暴露为可保存、比较或返回的 Telora 值。分配、quota 和宽度
检查必须在提交 len 前完成，失败不能发布半初始化元素。

这一方向尚未确认。原型需要验证 table/container 的具体物理头、几何增长策略、
trap 前后的提交顺序、初始化 collector 与请求 reset，并用历史 slice 分叉、冻结
数组首次 push、嵌套 Array、Dyn 元素和来源转发覆盖别名语义。它不应作为旧布局
上的局部 `DATA + 12` 补丁实施。

**Record / Tuple。** 二者共用容器机制，容器保存一次类型，字段按静态布局排列并分别保存来源。Record 字段身份和 Tuple 位置都在 codegen 时转换为偏移，不运行时查名字。复制值只复制引用描述，不深复制字段；字段读取转发其来源。Unit 单独决定，不要求分配空容器。

**Dict(K,V)。** 值内保存 keys/values 两个数组容器引用，不额外分配 Dict 容器，也不附带切片范围。两列保存各自类型和长度，长度必须相等；键有序且唯一，二分查找后按相同索引取值。键和值各保留来源。字典类型的键值类型可以从列类型获得，静态已知时无需查询；第一版不支持字典切片视图。

**Fn。** function 是具体函数实例的稳定身份，可映射到 Wasm 调用目标及捕获环境布局；env 是捕获环境引用。复制函数值保持身份，独立构造保持现有身份区别；无捕获闭包也不能一律用同一个空环境抹掉构造身份。函数签名仍由静态类型提供。递归闭包按上一节处理稳定引用和构造期填充。

**Enum。** 无 payload 时只有 tag；有 payload 时按所有分支所需的最大空间形成统一布局。payload 包含它自己的来源，而不是借用整个 enum 的 Loc：`Some(x)` 的来源与 x 的来源分别保留，解包转发后者。不做 niche encoding。首版接受非递归 payload 直接内联，不设置按大小自动装箱的阈值；最宽分支增加其他分支存储步长的成本是明确接受的取舍。整个 enum 进入 Dyn 且其数据部分超过 16 字节时，仍按 Dyn 的统一规则装箱。后续只有实际空间或性能数据支持时，再讨论 enum 自身的大小阈值。

递归判断针对物理内联边，而不是一般类型依赖图。例如 `Tree = enum { Empty, More((Int, Tree)) }` 已经经过 Tuple 容器引用；经过 Array、Record 或 newtype 容器的路径同样已被打断，无需 enum 再装箱。`Chain = enum { End, Next(Chain) }` 这类直接或相互经过 enum payload 的内联环，才需要增加间接边界。布局阶段应从内联依赖图确定该处理，不因遍历顺序或临时大小选择不同表示。现有 `candidate_layout.rs` 已有内联环检测逻辑，可作为新布局实现的参考。

**Dyn。** 保存原值的 Loc、具体 TypeId 和 16 字节 payload。payload 存具体类型的数据部分，不再重复外层 Loc；嵌套字段/enum payload 的独立来源仍属于其数据表示，不能删掉。上述标量、String/Bytes、Array、Record/Tuple、Dict、Fn 的数据部分均可内联；超过容量时 payload 存装箱引用，剩余字节不解释为数据。内联还是装箱由 concrete_ty 的确定布局判定，不新增动态标记。装箱体的描述可由 Dyn 的具体类型取得，不要求重复保存同一个外层 Loc。解包保留原来源和底层共享身份，不重新猜测类型。具体 payload 对齐及装箱体的管理信息留给物理布局器与回收协议明确。

### 已确认方向：Newtype 作为单字段容器

当前 `enums.rs` 的 NewtypeConstructor 将原始参数完整保存到 NEWTYPES，再单独构造包装值；`patterns.rs` 的 NewtypePattern 及 `.0` 投影读取原 payload。因此目前存在包装值与底层值两层独立来源，解包转发底层值的位置，不能在布局改造时悄悄合并成一份 Loc。

讨论已确定：newtype 沿用 Record/Tuple 的单字段容器机制。包装值逻辑上为 `{loc: u64, heap: u32}`，物理大小 16 字节；底层容器保存一次 newtype TypeId，并保存底层完整值（包含底层 Loc）。容器大小另计，包装 Int 不意味着总内存只有 16 字节。

包装值与底层值的来源分别保留，解包和 `.0` 转发底层来源，构造检查语义不变。复制包装值只复制引用；进入 Dyn 时保存 newtype 的具体 TypeId，payload 内联其 heap 引用，不新增一层仅为类型擦除而存在的装箱。名义类型不与底层类型混同。

这样接受一次间接访问，换取来源、字段访问、递归边界和回收统一使用容器规则，不为 newtype 另建来源透明转换或特殊内联表示。

## 8. Copy-collect：从自描述对象改为带布局的遍历任务

这里的 copy-collect 不是传统的持续 GC。Telora 的目标生命周期中没有执行期
周期回收、并发回收或因内存压力触发的 safepoint：一个阶段内的语言内存只会
单调增加。允许回收或重排的边界只有：

```text
初始化阶段：单调分配
service 就绪：一次 copy-collect/跨实例 compact，形成 frozen 基线
请求阶段：从 work checkpoint 单调分配
请求结束：整体 reset 到 checkpoint
```

这个假设带来一个比“选择哪种 GC”更强的简化：普通语言对象没有独立释放操作，
也不需要 mark bit、free list、写屏障或析构协议。分配器状态原则上只是当前
frontier（采用 chunk 时再加当前 chunk）；服务实例再保存 frozen checkpoint。
请求 reset 的正确性来自“任何 work 引用都不得写回 frozen 根或跨请求逃逸”，
而不是来自一次请求末尾的可达性扫描。codegen/RT 必须在可能写入长期状态的边界
阻止这种逃逸；如果语言语义以后允许服务状态随请求演化，这个假设必须重新讨论。

因此不要求逐对象 free，也不要求为普通执行点生成活跃根图。Array/String 的
几何扩容可以遗留旧 backing，连续增长的累计废弃容量仍为 O(最终容量)，在
service compact 或请求 reset 时统一消失。Regex 等具有析构行为的 RT 资源仍由
专用资源表在 reset、迁移或实例销毁时处理，不能据此反推普通语言对象需要传统
GC。跨实例 compact 与 snapshot 导出是同一次根遍历的不同目标，不增加执行期
回收机制。

当前回收器不能直接应对无类型头部的值。改造后的概念任务应包含：

```text
Trace { reference_or_slot, layout, destination }
```

采用容器级类型后，任务到达容器时可以从容器头取得 layout；扫描其内部普通元素时再沿布局传递。这样不必要求所有跨容器引用都另外携带类型，改造边界也更清楚。独立的无标签标量根、内联字段根仍需由根记录提供类型，或在确实需要动态容器的边界打包；裸地址本身不能说明值的宽度。

根提供布局；record 沿字段描述传播布局；array 沿元素布局和长度遍历；enum 根据 tag 选择分支；Dyn 读取具体类型；闭包根据函数目标身份取得环境描述，再访问捕获项。引用不再需要每次重复保存类型，但解释引用所需的信息必须有明确来源。

同一个函数签名可对应不同捕获环境。若环境可在没有函数对象的情况下独立成为根，就必须显式附带环境布局身份，不能假设从一个裸环境地址就能推断出来。

初始化 demand 表、顶层导出、property 结果、诊断/debug/test 对象和 Host 显式根都要覆盖。特别是诊断 subject 或 debug 中的异构值，可能需要在边界保存具体类型，不能因为常规字段已静态化而删掉它们的动态信息。

共享与循环去重继续按存储身份进行；同一对象从不同边到达时应验证布局相容，而不是因传入不同类型身份就随意复制成两个对象。slice 只改变可见范围，底层内容身份仍遵循现有约定。

继续保持 RFC 0302 的保守初始化根集合、固定元数据身份和请求 checkpoint/reset
语义，但不再把单一 Vec、`work_base` 数值或 table 间接层视为必须保留的物理
实现。此次不同时要求精确裁剪 property 根，也不引入执行中可移动 GC，因此不
需要为每条 Wasm 指令提供活跃栈根图；若将来允许执行中回收，必须另行设计
safepoint 和机器值重定位。

首选 sealing 路径是 Host 驱动的跨实例 copy-collect。Host 将旧、新 Guest 地址
分别视为带实例语境的 u32，通过 TraceLayout 从明确根集合遍历，在新实例 arena
的底部顺序分配，并记录 `old address -> new address`。遍历结束时记录 frontier，
其下成为 frozen prefix，其上供请求分配。目标换成确定性编码器
时，同一遍历直接形成 snapshot；不需要先把整个旧线性内存复制到 Host。

普通语言对象按布局复制；Regex 等 Rust 资源按资源种类执行 clone/rebuild 回调，
例如从旧实例取得 pattern、在新实例重新 compile，并记录 old handle -> new
handle。service phase、handler、demand/property/top-level 槽、source/BOL 状态和
少量 mutable globals 必须由显式 InstanceStateDescriptor 列为根或恢复槽，不能
要求 collector 猜测任意 Rust static。

mem-alloc/realloc/free 所管理的 Host 交换缓冲区不因此变成语言 arena。Host 不
跨可能 `memory.grow` 的 Guest 调用缓存 memory view；数值地址保持有效，但每次
访问重新取得对应实例的视图。

因此此方案真正需要追踪的不是每个分配，而是三类边界状态：

```text
language frontier / frozen checkpoint
resource table length / frozen resource baseline
InstanceStateDescriptor 中的显式根与状态槽
```

普通对象、数组 backing 和字节 backing 都服从前两阶段的批量生命周期；只有
Regex 等非字节可搬移资源需要按资源种类 rebuild/drop。这里不再建立一套通用
finalizer 机制。

## 9. 建议先验证的纵向样例

优先选一条不依赖大型业务的完整路径：

```text
Int 算术 → 带来源的函数参数/返回值 → Array(Int) 直接构造
        → 闭包捕获 → 初始化 copy-collect → 请求执行 → 来源诊断
```

随后加入多 word 参数、record 字段转发、动态分支返回、enum/Dyn、局部泛型闭包、间接调用及数据 parse。原型要走真实的 RT 接口和回收路径，不能只比较一段脱离运行时的 Wasm 算术。

需要记录的指标：

- 每函数 locals 峰值、调用参数与结果 word 数、最大操作数栈压力。
- 语言堆分配次数/字节、临时值物化次数、复制字节数。
- Wasm 代码与静态数据体积、codegen 和引擎加载时间。
- 初始化时间与保留堆大小、持续请求时延、请求后 reset 成本。
- 来源保存是否完整，以及动态边界需要多少打包/解包。

对照包含 #213、world-model 真实查询和既有诊断测试。代码尺寸、调用 word 数、引擎内部寄存器压力之间可能有取舍；不能仅凭 locals 更少判定整体更快。

Array 原型还应单独记录连续 `fold + push` 与历史 slice 分叉两条曲线。前者预期
通过 tail append + 几何扩容将累计复制从 O(n²) 降为摊销 O(n)，后者必须保持值
语义并在分叉点明确付出复制成本。初始化 freeze 前、请求 work arena、frozen
基线首次 push 和 copy-collect 后继续 push 都要分别测量。

## 10. 需要讨论后再决定的事项

1. 在已接受的 14/25/25 边界内，明确无来源和临时来源的保留编号及输入端诊断。
2. 边界固定返回栈的容量、访问接口和非零状态码如何确定？内部已统一返回 status/Loc/data，0 成功、非零失败；Never 的机器签名需明确。compute_loc 语义已确定，内置能力经过一等函数间接调用时的位置传递机制仍需验证。
3. 在已确认的值与容器头部逻辑布局上确定物理字段偏移和标记编码。普通容器头保存 TypeId，RawArray/RawBytes 候选头保存 data/len/cap，闭包环境头保存环境布局 ID；来源和数据首版相邻存储已确认。
4. 哪些值可始终停留在机器栈，哪些必须物化？如何机械地消费 MIR 结果而不再增加类型猜测？
5. 根表、闭包环境、动态诊断值和 RT 资源如何提供完整 TraceLayout？普通捕获直接保存值已确认，递归闭包的稳定身份和构造期填充如何落实？
6. 如何为新制品升级 ABI 并明确拒绝不匹配版本，而不长期保留旧布局兼容分支？
7. Array raw storage 是否采用 append-only len + cap 的内部可变模型？若采用，容器头、frozen 判定、增长因子和失败提交协议如何确定；这一原则是否只用于 Array，还是随后扩展到其他构造期 transient？
8. 普通对象引用是否从单一 Vec 的逻辑偏移改为稳定 arena 的直接 Wasm 地址？若采用，哪些普通对象 table 可以删除，arena 如何与 Rust allocator/交换缓冲区划分区域，连续 frontier 是否足够，还是确实需要 chunk？
9. Host 跨实例 collector 的 InstanceStateDescriptor 包含哪些根和恢复槽？资源 clone、确定性 snapshot 编码与 module/ABI 身份如何统一？

当前倾向是先确立静态布局驱动、必要位置传播和少量暂存的原则，以“稳定 arena 直接地址、容器保存一次类型、普通元素不保存 TypeId、Dyn 元素保留实际类型、Host 跨实例 sealing”为联合原型。原型应同时验证 Array/RawBytes header、请求 checkpoint/reset、资源迁移和 snapshot sink，避免分别实现随后互相冲突的堆、collector 与发布格式。位宽压缩、数据分列和字面量静态化均不应成为验证这一原则的强制前置条件。
