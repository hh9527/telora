# RFC 0281：MIR 驱动的候选类型布局计算与展示

- 状态：已实现并验证，布局计算与展示范围完成；运行时接入不在本 RFC 范围内
- 跟踪：[#177](https://github.com/hh9527/telora/issues/177)
- 后续布局闭合：[#178](https://github.com/hh9527/telora/issues/178)，分支 `feat/concrete-type-layout-closure`
- 独立分支：`feat/0281-mir-type-layout`
- 日期：2026-09-11
- 修订：2026-09-12，范围收敛为独立布局计算及隐藏 CLI 展示，不接入运行时
- 接口修订：2026-09-12，改为隐藏选项 `--dump-types-layout <filename>`，导出单份 JSON；移除旧选项，不保留别名
- 前置：[RFC 0280](0280-demand-driven-inference-materialization.md)
- 讨论来源：[利用已闭合 MIR 设计运行时数据布局](../discuss/mir-directed-runtime-layout.md)

## 动机与范围

本 RFC 的交付仅为：独立消费 SealedMir 的候选布局器，以及隐藏选项 `check --dump-types-layout <filename>`。该选项等同于 `check --only-types` 的静态检查，再增加布局计算和 JSON 导出。不创建 VM，不生成字节码，不实例化分类对象表，不替换现有运行时表示。

下文的值头部、对象表和生命周期讨论定义候选布局的含义及未来接入约束，不构成本轮 heap/codegen/VM 改造任务。运行时接入应另行提出后续 RFC。

SealedMir 已提供闭合的类型和引用信息。运行时数据表示应直接利用这些结果确定大小、对齐、字段偏移和元素步长，为机械 codegen、现有 VM 和未来 native/Wasm 后端提供共同基础。

此前讨论了多值宽、来源分列、静态类型标签省略等优化。第一版同时引入这些机制，会产生不同场景的来源寻址、动态标签存储及复制规则。本 RFC 选择先统一值表示：每个可独立携带来源的运行时值使用 `loc + TypeId + data`，前两项固定占两个 64 位 word，data 根据类型具有固定布局。

本轮优先减少表示规则的多样性，不承诺降低内存占用或获得特定性能收益。统一头部可能增加空间和带宽成本。

heap 按对象类别建立独立表。统一的是外层值头部，各类对象可以采用不同的内部存储；HeapId 是所属表内的索引，由 TypeId 决定选择哪张表及如何解释对象。

## 用户可见语义

新增隐藏选项：

```sh
telora check --dump-types-layout layout.json <selector>
telora check --lib --dump-types-layout layout.json
telora check --tests --dump-types-layout layout.json
```

选项不出现在普通帮助中。它隐含 `--only-types`，两者同时传入时按同一静态流程执行一次。模块选择、`--lib`/`--tests` 组合规则及静态诊断行为沿用 check。

流程为：模块图 → 符号求解 → 类型求解 → seal → 候选布局计算 → 展示。静态错误仍输出现有诊断并返回失败，不对未 seal 的 MIR 伪造最终布局。全流程不读取数据模块内容、不执行 property 或顶层值、不创建 VM。

布局文件至少展示最终 TypeId、可读类型、布局状态、data 实际大小和对齐、完整值大小及步长、字段/分支偏移、对象表类别与对象形状。按最终 TypeId 和稳定字段/分支顺序输出，注明这是候选布局而非当前 VM 的实际布局。

文件是一份 `schema: telora.types-layout/v1` 的 JSON，包含 roots、target、header、offset_bases、types 和 summary。target 记录 word、HeapId、TypeId 宽度。文件不包含耗时、时间戳或目标文件名等波动信息，相同输入应产生逐字节相同的导出。

stdout 只保留现有 check 诊断与 summary，不输出布局记录。目标相对路径以进程工作目录为基准，`-C` 只选择项目上下文。不自动创建父目录，也不提供 `-` 表示 stdout 的特殊约定。

在静态检查和布局计算成功后，先写入目标目录中的临时文件，再原子替换目标文件。静态失败、计算失败或文件写入失败时保留已有目标文件，未存在的目标不产生半成品；文件错误返回非零退出码并包含目标路径。待定布局不属于导出失败。

未实例化的泛型模板标为“不适用具体布局”；未定表示规则标为“待定”并说明依赖，不能静默使用旧 Val 的大小。已支持规则的计算错误或溢出返回失败。静态检查通过但含待定设计项时可以成功输出报告，同时明确展示未完成数量，不声称布局全覆盖。空模块集合输出空布局报告并成功。

不改变语言语法、类型推断、构造检查、更新、slice 或诊断含义。来源必须保留，不能仅剩当前执行指令的位置。字段的原始来源、整体构造位置以及规则位置应继续支持现有诊断。

静态求解、初始化和 entry 执行的阶段边界不变：布局构建不执行 Telora 代码，VM 不补做类型推断。初始化仍使用整图 Initialize WorkWorld，成功后统一发布到 MainWorld，再由新的 WorkWorld 执行 entry。失败时不发布成功的 session 结果。

## 决策：固定头部、按类型确定 data 宽度

```text
偏移 0： loc     [u32; 3]   12 bytes
偏移 12：type_id u32         4 bytes
偏移 16：data                N words
```

头部固定为 16 字节，数据起点按 8 字节对齐。位置三元组的具体含义及无来源哨兵须与现有来源语义对接，禁止截断或混淆源码位置与外部数据位置。

TypeId 沿用 SealedMir 的最终身份，不建立运行时重新推断得到的类型身份。对于静态已知的操作，codegen 直接使用布局，不要求执行时读取 TypeId。头部标签供动态操作、诊断和通用数据遍历使用。

这是一份内部运行时布局契约，不是持久化格式或稳定的 Rust/C ABI；不能直接依赖 Rust 默认结构体布局。边界读写使用显式布局接口，目标字节序和对齐由目标布局规则决定。

### data 描述与完整值必须区分

| 类型 | data 初步表示 | data 槽位 | 含头部的完整值 |
| --- | --- | --- | --- |
| Int / Float / Bool | 标量位模式 | 1 word | 3 words |
| String | inline 或 HeapId 与范围、标志 | 2 words | 4 words |
| Array | HeapId、start、end 各 u32 | 2 words | 4 words |
| Record | HeapId | 1 word | 3 words |
| Dict | HeapId | 1 word | 3 words |
| 无 payload enum | tag | 1 word | 3 words |

Array 的描述实际占 12 字节、对齐 4 字节，独立 data 区向 word 取整后占 16 字节。不得将“Array 两 word”误读为包含来源和类型头的总大小。

Unit 的 data 可为零字节，完整值仍有头部。Tuple、闭包及其他尚未确定表示的类型显式展示为待定，不允许缺失时退回旧 Val 表示。

## 布局表

```text
SealedMir + TargetLayout
          ↓
LayoutTable
          ↓
隐藏 CLI 的布局报告
```

布局表至少包含：

- 具体类型的 data 大小、对齐及完整值步长；
- 字段、分支及数组元素的布局；
- 内联数据与 HeapId 引用的位置，供复制、遍历和发布使用；
- HeapId 引用所属的对象表类别；
- 对象布局与引用该对象的值布局之间的关系。

Record 值只保存引用，对象内的字段按已确定布局存储。Array 值保存 slice 描述，元素存储按完整元素值布局具有固定步长。Dict 的键值仍是动态集合，不将其伪装成固定字段 Record。

布局按大小和对齐计算，最终向执行槽位取整。相同完整输入和目标规则必须产生确定的布局输出。布局器不依赖 VM，不读取 property 求值结果，不重新 resolve 或推断类型。

## 来源与嵌套值

首版不采用 `values` 与 `locs` 分列，也不在静态已知的位置省掉头部。

独立字段、数组元素以及具有独立来源的 payload 均保存完整值头部。聚合值的整体来源与内部成员来源各有意义，不以外层来源覆盖内部来源。

普通值复制保留来源和 TypeId；构造、字段更新和返回等操作按现有语义决定新值来源。`<~` 未更新字段的来源不得无故丢失。slice 不复制元素，只调整视图范围，元素来源留在底层存储中。

独立来源的嵌套值会产生额外头部，这是首版有意接受的成本。不能为了宽度目标将多个独立来源压成一个位置。

## enum 与 dyn 的边界

无 payload enum 使用一个 tag word。带 payload enum 使用显式 tag 与分支布局，统一步长由最大分支及对齐决定；递归路径必须间接存储。

早期讨论中的 `[tag:u32, HeapId:u32, start:u32, end:u32]` 能表达一个带 Array 的原始数据描述，但不包含 payload 的独立来源。在本方案下，不以“两 word enum”为硬约束：若 payload 是独立值，就连同其头部存储，或通过 HeapId 引用完整值。

dyn/未来 trait object 也不因增加 TypeId 头部就自动解决不等宽 payload。`Array(dyn)` 必须有固定元素步长。应优先避免已间接存储对象的重复装箱，但具体内联容量、超宽 payload 的间接规则及 trait 实现表身份仍需单独收敛。

这些未定规则不阻止实现具体类型的布局器；报告明确标记待定项及依赖它的布局，不能以通用旧表示作为隐含回退。本 RFC 不引入新的 dyn 或 trait object 语言能力。

## 分类对象表

```text
Heap {
    strings: StringTable,
    records: RecordTable,
    dicts:   DictTable,
    arrays:  ArrayTable,
}
```

HeapId 暂定为 u32，明确表示对应表内的索引，不是统一堆偏移或宿主裸指针。不同表可以使用相同编号，因此裸 HeapId 不能唯一确定对象。

```text
TypeId → 类型类别 → 选择表
HeapId → 表内条目 → 定位存储
TypeId → 具体布局 → 解释对象
```

表选择依据已闭合的类型信息，不匹配类型名称。静态已知时，codegen 直接生成对应表的访问；动态访问需要保留实际具体类型身份。inline String 不访问 StringTable，heaped String 才通过 HeapId 定位内容。

### 表内对象形状

| 表 | 存储方向 | 类型信息的作用 |
| --- | --- | --- |
| StringTable | 条目定位连续字节存储 | 选择 String 操作和表示规则 |
| RecordTable | 条目定位按字段排列的完整值存储 | 确定字段类型与偏移 |
| DictTable | 专门的哈希索引与键值存储 | 确定键值表示和操作 |
| ArrayTable | 条目定位连续元素存储 | 确定元素布局和步长 |

分类表不要求每个对象独立分配，也不要求所有表使用相同条目结构。表内可以保存必要的存储位置、长度等管理信息；不为了通用对象分派重复保存 kind 标签或统一 LayoutId。通用遍历接收带类型的值或相应类型上下文。

值头部记录这一次值的来源和类型，对象无需机械地再复制一份同样的头部。Record 字段和 Array 元素是独立值，仍各自保存完整头部；String 字节不逐字节携带值头部。

同一表内同一 HeapId 必须指向布局一致的对象。多个 slice 可以共享底层对象并具有不同范围、整体来源；类型擦除或 trait 访问不得使对象被解释成不兼容的布局。

### 尚待收敛的表划分

当前候选模型让所有 Record 共用 RecordTable，由具体 TypeId 确定字段布局；是否按具体类型进一步细分仍待决定。报告展示类别和布局，不实际分配表或 HeapId。Tuple、闭包及间接 enum payload 的表归属未定时显式标记，不引入隐含的通用旧堆回退。

## 未来接入约束：arena 与生命周期

HeapId 的表内索引容量、world 身份编码和溢出行为需要明确。u32 索引不直接等同于 4 GiB 总堆上限；表条目指向的存储大小由各表实现决定。

start/end 为 u32，超出可表示范围必须产生明确错误，不允许截断。slice 的底层存储必须在使用期间有效。

MainWorld 与 WorkWorld 的生命周期规则保持现有语义。类型描述可通过稳定 TypeId 共享，但对象 HeapId 的发布与重映射仍需处理。复制与发布使用布局表识别有效引用，保持共享关系，不读取未激活 enum 分支或 padding。

单个来源 heap 的复制去重键至少为 `(表类别, HeapId)`；跨多个来源 world 时还需包含来源 world 身份。发布后重写对应表的目标索引，不能仅按数字 HeapId 去重。具体 TypeId 随遍历上下文传递，用于解释布局；共享对象仍只复制一次。

本轮不顺带引入 arena 接管、冻结零复制或新的回收算法。

## 未来接入约束：codegen 与执行后端

所有布局访问集中经过布局表和统一接口，不在 VM、codec、原生函数适配层散布手写偏移或类型名称匹配。

已确定字段访问生成固定偏移；数组访问使用已知元素步长；函数参数、返回值和捕获布局使用已求解类型。运行时携带 TypeId 不授权 codegen/VM 再次猜测类型。

Record 字段访问按已确定的 RecordTable 选择和字段偏移执行；Array slice 按 ArrayTable 和元素步长执行。不因使用分类表再增加统一对象 kind 分派。

本轮只展示候选布局，不修改现有 VM 或 codegen。未来接入现有 VM、Cranelift/Wasm 时可以复用布局计算；具体物化值、调用边界和来源传播由后续 RFC 确定。

## 延后与放弃的方案

- 延后来源分列：先消除双存储寻址和来源映射差异。
- 延后静态 TypeId 省略：先让所有独立值具有相同头部。
- 延后 niche encoding、空闲位复用和组合专用 pack/unpack。
- 不要求所有值同宽，也不要求所有 enum/dyn 都能塞进两 word。
- 不统一将所有值额外装箱；间接存储依据类型布局明确决定。
- 不在本轮加入 native/Wasm 后端、拆箱优化或新的内存回收机制。

## 实施计划

1. **独立布局器**：消费 SealedMir，区分 data/完整值/对象布局，实现已确定规则及待定状态传播。少量单元测试核对大小、对齐、递归引用和确定性；不依赖 VM 或旧 Val 布局。
2. **隐藏 CLI 导出**：将 `--dump-types-layout <filename>` 接到现有静态检查成功后的 seal 结果，原子写入稳定 JSON，复用模块选择与诊断逻辑。
3. **验证与记录**：用小型 `.telora` 输入核对 CLI 报告，再对 ontology 等实际模块生成布局报告，记录覆盖范围和未定项。本轮无需运行时性能评估；不把预测布局大小解释为实际 RSS。

## 可执行的验收条件

- 自动验证头部偏移为 0/12/16、大小为 16 字节，标量完整值为 24 字节，Array 完整值为 32 字节。
- 布局测试覆盖标量、String、Array、Record、enum 及递归引用；已确定布局正确对齐、步长固定；未定的 Tuple/闭包等规则明确展示，不伪造大小。重复构建输出确定。
- CLI 测试验证选项在普通帮助中隐藏，支持 selector、`--lib`、`--tests` 和与 `--only-types` 合用；静态阶段只执行一次。
- 使用含初始化失败表达式及不可解析数据内容的静态合法输入，验证新选项与 `--only-types` 均不进入求值或数据解析；布局器接口不接收 VM。
- 静态错误保持原诊断与失败退出，不输出成功布局；空集合正常输出空报告；待定项与泛型模板状态明确区分，布局计算溢出不会截断。
- 用 `.telora` 输入验证字段偏移、数组元素步长和分类表归属；别名或导入重命名不改变同一类型的布局，不能按名字猜类型。
- 普通 check/eval/test/run 的现有执行路径保持原状，布局器只由新选项调用，不新增旧运行时对候选布局的依赖。
- 运行相关布局单元测试和 CLI 测试，检查实际项目的布局报告；无需证明运行时性能收益。

## 风险与接受边界

统一头部会使小值和嵌套聚合的空间成本明显增加，Array 标量元素也包含头部。正常运算可能加载不需要的 loc/TypeId。这是为了首版一致性而接受的设计方向，实际代价必须通过观测确认，不能宣传为已实现的性能优化。

本轮只计算与展示上述成本，不实际分配这些运行时值。报告中的字节数不是实际堆占用，也不能据此声称 CLI 运行速度或内存得到改善。新选项本身会增加布局计算与输出的时间和内存。

布局观察结果用于收敛后续运行时 RFC；来源分列等改动应显式修订候选规则。完成本 RFC 不要求完成运行时迁移或解决所有未来表示问题。

## 实施与验收记录（2026-09-12）

下面的 JSONL 输出及 layout_seconds 为 #177 首次合入时的历史记录。后续接口修订以“用户可见语义”中的单份 JSON 文件为准，移除 stdout 布局记录和导出中的耗时字段。

- 独立模块：`crates/telora-core/src/candidate_layout.rs`，入口只接收 `&SealedMir`。
- CLI 只在新隐藏选项启用且 seal 成功后调用；现有执行入口不消费候选布局。
- 报告使用 `type_layout` 和 `layout_summary` JSONL 记录，明确标记 `candidate: true`。
- `known`、`pending`、`template` 分别表示已知布局、未定规则及未实例化模板；泛型依赖沿已求解类型图传播。值布局与对象布局分别报告，Record 引用大小已知不意味着所有字段布局都已知。
- 字段 offset 相对于对象起点；已知 enum 的 payload offset 相对于完整值起点，当前为 24（16 字节头部加 8 字节 tag）。未定分支不报告已确定的 payload 偏移。
- Array 描述为 12 字节，完整值为 32 字节。完整 Int 元素步长为 24 字节；`struct {a: Int, b: Array(Int)}` 的字段偏移为 0/24，对象大小为 56 字节。
- Tuple、闭包、dyn、原生不透明对象、新类型包装等尚未规定的表示，以及需要间接规则的递归 enum，显式保留为待定；Dict 引用大小已知，哈希对象内部布局待定。
- `layout_seconds` 仅记录候选布局计算，不包含打印时间，也不计入原有 check_seconds。
- 隐藏选项不加入普通帮助、CLI 指南或当前实现文档；开发契约保留在本 RFC。

验证结果：

- `cargo test -p telora --test cli`：69 项通过，包含既有语言验收。
- `cargo test -p telora-core candidate_layout`：大小、对齐取整及溢出验证通过。
- 最后布局细节调整后重跑 `new_types_layout` CLI 用例与布局单元测试通过。
- CLI 用例核对确定性、字段偏移、Array 步长、enum payload、递归待定、泛型模板、重命名导入、隐藏帮助、组合参数、空集合和静态失败。
- 非法 JSON/TOML/YAML 用例同时验证 only-types 与新选项不读取内容；普通 check 仍拒绝非法内容。顶层除零在新选项下不执行，普通 check 仍失败。
- ontology 全库实际报告成功：3502 个类型条目，其中 394 个模板、2687 个含待定值或对象布局的条目；静态 unknown/conflict 均为零，execution_seconds 为零。待定条目是候选表示未定义，不是类型推断失败。

这些数据用于布局覆盖观察，不构成运行时性能或内存收益结论。

## 后续：具体类型布局闭合（#178）

#177 完成的是独立计算与展示能力。#178 的完成条件更强：所有需要物化的具体类型及其对象存储规则必须闭合，不能以部分 pending 留存作为终点。按分类审计、基础类型、聚合包装、递归 enum、函数/动态值、heap 对象、最终闭合的顺序推进；仍不接入运行时。

导出条目增加真实的 `constructor` 类别，独立于可读 type_name。尤其 Meta(T) 在可读名称中也显示为 TypeOf(T)，不能因此将其当成运行时 TypeOf 值。类型分类依据 MIR 构造器而非显示名称。模板、编译期项、不可构造类型的完整分类继续在 #178 中收敛。

### 首批确定：Type、TypeOf(T)、Bytes

- Type 与具体 TypeOf(T) 的 data 为一个 `represented_type_id: u32`，对齐 4 字节，完整值占 24 字节。值头部的 TypeId 表示该值自身的类型，data 中的 TypeId 表示其引用的类型；两者不能混淆。具体 TypeOf(T) 不依赖 T 的数据布局是否已知。
- Meta(T) 是静态类型表达式的构造器，不套用上述运行时规则。此批先明确区分，后续分类规则不得以运行时 TypeOf 的布局掩盖它。
- Bytes 的 data 为 `HeapId/start/end: u32`，实际 12 字节，对齐 4 字节，完整值占 32 字节。HeapId 属于独立 BytesTable，底层是连续原始字节，元素步长 1，不给每个字节加完整值头。
- BytesTable 的内容区域大小为 byte_length，slice 的有效区间满足 `start <= end <= byte_length`。本轮报告描述区和对象内容，不决定表条目的宿主管理字段或实际分配策略。来源仍属于外层 Bytes 值。
- Type/TypeOf 没有 heap 引用；Bytes 的 data 偏移 0/4/8 依次为 HeapId/start/end，相对于完整值为 16/20/24。

原生不透明类型、Never、聚合包装、递归 enum、闭包/dyn 和 Dict 的完整存储规则尚未完成，不将本批视为 #178 已完成。

### #178 闭合规则（后续修订，取代上文对应的待定项）

这些是独立候选 ABI 的确定规则，不是当前 VM 的实现声明。word 固定为 8 字节，值对齐为 8 字节，所有 padding 写零且不参与引用遍历。TypeId、HeapId、计数和偏移为 u32；所有大小计算检查加法、乘法和对齐溢出。

#### 类型分类与成功门槛

- `compile_time` 仅对应 Meta、Namespace、TypeFunction、TypeList、PropertyBound、Bound：分别是静态类型表达式、命名空间、类型构造器、类型列表、约束及绑定变量。不能把 Type、TypeOf 或函数值归入此类。
- 另有明确来源的静态 Record：模块 body 对应的导出记录可能包含 Meta/Namespace 等声明。通过 MIR 模块 body 的已求解 TypeId 识别，且要求成员包含上述编译期项；这种导出清单标为 compile_time。不能仅凭 Record 名称、字段名或“含未知项”分类。普通运行时 Record 含编译期成员仍失败。
- `template` 必须存在可追溯的自由 Parameter 或未被 Quantified 绑定的 Bound。自由参数沿真实类型参数和成员边传播。Quantified 只截断 Bound 的传播，不截断外层自由 Parameter；有闭合量化契约的函数本身是可物化值。
- `uninhabited` 由有限构造的最小不动点确定：Never 无值，聚合要求全部成员可构造，enum 要求至少一个分支可构造。无基例的递归类型无有限值；Array(Never)、Dict(Never) 仍有空集合值，函数仍可接受/返回 Never。
- 其他具体类型必须具有 `known` 布局；运行时成员不能含编译期类型。未注册 native ABI、缺失表示、非法组成或溢出直接导致布局导出失败，保留目标文件。
- 删除“pending 也视为最终成功”的完成标准。报告 summary 的 closed=true 只在所有条目分类及对象规则成功验证后产生，pending 为零。模板和不可构造项不是已知对象大小，不输出伪造大小。

#### 聚合与包装

Record、非空 Tuple、newtype 的 data 都是 HeapId:u32，完整值 24 字节，分别指向 RecordTable、TupleTable、NewtypeTable。所有具体 Record 共用 RecordTable，由 TypeId 解释字段；其余两表同理。

对象成员是连续完整值，每个成员有独立来源和 TypeId。字段按 MIR 的规范顺序，Tuple 按元素序号，newtype 保存其被包装的完整值。对象字节数为各完整成员大小之和，字段偏移为前缀和。空 Tuple/Unit 无 data，完整值为 16 字节。Unchecked 仅改变保证，复用其具体 owner 的 data 与对象布局。

#### enum：普通分支内联，递归边间接

enum data 的前 8 字节是 tag:u32 和 padding:u32。普通分支在 data+8（完整值+24）保存完整 payload；最大可构造分支决定大小，最后向 word 对齐。无 payload 的 enum 仍只用一个 data word。不可构造分支不占 payload 空间。

建立只含 enum payload 和透明 Unchecked 边的内联依赖图；对象引用会终止该图。对每条 payload 边检查是否处于有向环中，且仅把这些环内边改为 HeapId:u32，引用 ValueTable 中的完整 payload。所有环内边同时确定，不按 DFS 首次遇到的位置或 ID 顺序选择。

因此 `enum { End, More(Self) }` 的完整值是 32 字节，More 引用 ValueTable；互递归同理。`enum { Items(Array(Self)) }` 的数组已经打断内联依赖，不再额外装箱 Array 描述。嵌套但无环的 enum 继续内联。布局记录逐分支的 storage、table 和 offset；不读取非活跃分支。

#### String、Bytes 与数组

String data 固定 16 字节：tag=0 时为 tag:u8、UTF-8 字节长度:u8、inline:[u8;14]；tag=1 时为 tag:u8、padding:[u8;3]、HeapId/start/end:u32。StringTable 保存原始 UTF-8 字节，slice 必须位于字符边界。String 的编码独立于 enum/dyn，不复用其标签位。

Bytes 与 Array data 均为 HeapId/start/end:u32。BytesTable 保存连续字节；ArrayTable 保存连续完整元素值。表记录底层长度，范围满足 start<=end<=length。序列内容区域大小为 length*stride，Array(Never) 只能 length=0，不虚构 Never 元素大小。

#### 函数与捕获环境

Function 和已绑定的 Quantified data 为 function_id:u32、environment_id:u32，完整值 24 字节。无捕获时环境 ID 为 0；函数身份未来由已闭合代码实例分配，不按运行时参数重新推断。此处不生成实际 function ID 或字节码。

ClosureEnvTable 采用统一变长布局，允许同一函数签名对应不同捕获列表：对象头 capture_count:u32、total_bytes:u32；offsets[count]:u32 从偏移 8 开始；完整捕获值从 align8(8+4*count) 开始按稳定捕获顺序排列。offsets 相对对象起点，total_bytes 指向末尾，所有偏移必须可用 u32 表示。每个捕获值保留自身头部，可以据其 TypeId 遍历引用。

函数 TypeId 不足以确定某一个闭包的捕获数，但上述形状和大小公式是确定的；不因此标为 pending，也不声称捕获环境具有统一固定字节数。

#### Dyn 与间接值

Dyn data 固定 24 字节：concrete_type:u32、storage:u32、payload:[u8;16]，完整值为 40 字节。storage=0 时，data_bytes<=16 且 alignment<=8 的具体值直接复制原 data；来源由 Dyn 外层 loc 保留，具体身份由 concrete_type 保留。storage=1 时，payload 开头的 HeapId 引用 ValueTable 中的一个完整具体值，其余字节为零。

这让标量、String、Array、Record 引用进入 Dyn 时不额外装箱；更宽 enum/Dyn 通过 ValueTable 间接存储。Array(Dyn) 因而有固定 40 字节步长。内联 data 的 heap 引用仍按实际具体类型选择原对象表。

ValueTable 的每个对象从偏移 0 保存一个完整具体值，大小由其头部 TypeId 对应的已知布局确定。递归 enum 和超宽 Dyn 共用这一表。未来 trait object 不在语言范围内，不能借本 RFC 声称其语义已实现；若引入，需要另定实现表身份契约。

#### Dict

DictTable 对象头占 16 字节：len/capacity/buckets/reserved 各 u32。entries 从偏移 16 开始，capacity 个槽，每槽为 String 完整键（32 字节）加完整值；只遍历前 len 个已初始化槽。桶区从 align8(16+capacity*entry_stride) 开始，buckets 个 8 字节桶（hash:u32、entry_plus_one:u32，0 表示空桶）。

采用插入顺序 entries 和线性探测桶，buckets 是不小于 2*capacity 的二次幂；空表可取 capacity=0、buckets=1。桶只保存索引，不复制键值。哈希比较最终依靠 String 内容相等；具体 hash 算法不改变布局，不承诺当前 VM 已采用该结构。

对象大小公式为 align8(16+capacity*entry_stride)+8*buckets。长度不得超过容量。Dict(Never) 只能为空，不能实际存储无值类型。扩容、删除时桶重建等运行策略留给未来实现，不改变上述稳定存储形状。

#### Native 不透明资源

现有 native ABI 身份 (19,0)、(20,1)、(16,3)、(33,0)、(34,0) 采用固定资源引用契约：值 data 是 HeapId:u32，完整值 24 字节；NativeResourceTable 按 native module/slot 分区，每条目是 host_resource_token:u64。

宿主资源注册表拥有实际 regex/fmt/hash/test/blame 资源，负责 clone/drop/trace，包括资源可能保留的 Telora 值。token 不按 Telora 对象或宿主裸指针解释。固定的是语言与宿主之间的资源 ABI，不虚构第三方对象内部字节布局。未列出的 native 身份必须显式注册布局契约，否则失败。

#### 可计算的对象描述与引用遍历

报告的 storage 字段是带标签的 Fixed/Sequence/Dictionary/Captures/FullValue 描述；`Storage::allocation_bytes` 根据具体长度、容量、捕获值大小或完整值大小计算对象字节数并验证约束。storage_rule 提供对应文字说明，不以说明文本代替实际计算规则。

固定对象通过成员 TypeId 与偏移遍历，序列按元素类型和步长遍历，Dict 只遍历 live entries，闭包通过 offsets 和捕获值头遍历，ValueTable 通过值头遍历；native 通过注册 host trace。所有分类表的 HeapId 都需要所属 world 上下文，实际发布/复制仍不在本轮实施范围内。

### #178 验收记录

- CLI 全套 70 项通过，包含普通执行回归和语言验收；核心布局 4 项通过。补充嵌套 enum 和 Array(Dyn) 的断言后单独重跑布局专项。
- 布局专项覆盖普通/互递归/无基例 enum、Tuple、newtype、Unchecked、具体泛型实例、带捕获函数、已绑定的多态函数、Dyn 及其数组、Array(Never)、Dict 和 native 资源。
- 核心测试确认 Quantified 只绑定 Bound，不误消除自由 Parameter；未知 native 契约和非模块运行时 Record 中的 Meta 成员失败。对象公式测试覆盖合法容量、非法长度、u32 捕获偏移上限与 u64 乘加溢出。
- 导出保持逐字节确定，静态/写入失败保留目标文件；不向普通文档或帮助暴露隐藏选项。
- 四个实际项目分别运行全库 JSON 导出，全部 closed=true、pending=0；源码 Unknown/Conflicted 均为零，执行阶段均为零。

| 项目 | 条目 | known | compile_time | template | uninhabited |
| --- | ---: | ---: | ---: | ---: | ---: |
| ontology | 3502 | 1678 | 1627 | 196 | 1 |
| spider-model | 3779 | 1774 | 1753 | 251 | 1 |
| dog-model | 3761 | 1763 | 1746 | 251 | 1 |
| world-model | 3742 | 1747 | 1743 | 251 | 1 |

这些是完整图的分类与布局覆盖数据，不是运行时性能数据。具体布局缺失现已属于导出错误；新语言能力或 native ABI 需要显式增加规则。#178 的布局闭合范围完成，运行时接入仍需独立方案。

## 布局闭合后的隔离存储实验

新增 `experimental-layout-runtime` Cargo feature，默认关闭。启用后导出独立的 `layout_runtime` 模块，只消费 SealedMir 和候选布局；不依赖旧 heap/Val/VM，不进入任何现有执行入口。

实验实现的范围：

- 每个值按 loc[3]+TypeId+data 编码为 word 描述；arena 身份是访问上下文，不进入 ABI 字节。跨 arena 引用显式拒绝，尚无 world 发布或复制机制。
- Tuple/Record 共用固定字段的构造、读取和更新实现，仍使用不同分类表。空 Tuple/Unit 仅有 16 字节头部，无堆对象。
- Array 按完整元素步长连续存储，slice 只改变 HeapId/start/end 描述；索引返回借用的 ValueRef。更新生成新容器，保留旧容器和元素来源。
- Dict 按头部、entries、buckets 布局构造；支持内容查找、碰撞、覆盖、删除和稳定插入顺序。实验采用 FNV-1a 的 u32 hash，桶内仍以 String 内容确认相等。
- String 作为字段和 Dict 键的配套类型支持 inline/heaped；普通标量保存原始 u64 位模式。分类表采用 `Vec<Item>` 槽位，HeapId 直接索引 Item；每个 Item 独立拥有自己的变长缓冲区。
- Owned Value 只拥有值描述，克隆不递归复制引用对象；读取字段、数组元素和字典结果借用 arena 数据。持久更新目前重建容器的浅层描述，不是运行时性能优化实现。

存储策略修订：分类表管理固定宽度的 Item 槽位，不再通过 Span 管理共享大缓冲区。Tuple/Record/Array/Dict 的 Item 持有自己的 `Vec<u64>`，RawStringTable 的 Item 持有自己的 `Vec<u8>`；创建 word 对象时直接移入已构建的 Vec，避免再复制一次。表扩容只移动 Item 描述，不搬迁已有对象内容，HeapId 保持不变。槽位回收、复用及旧引用失效规则留待后续确定，本轮仍为追加槽位。

验证仅为新模块单元测试：

```sh
cargo test -p telora-core --features experimental-layout-runtime layout_runtime
cargo check -p telora-core --no-default-features
```

覆盖 Tuple/Record 字段偏移及表内同号 ID、空 Tuple、来源保留、嵌套 slice 与越界、持久更新、Dict 碰撞与重复键、长字符串和嵌套数组的浅层共享、跨 arena 拒绝。没有接入 codegen、初始化、CLI 或现有 VM，也未进行性能评价。本实验不实现语言级 @check、arena 发布/回收或全部候选类型。
