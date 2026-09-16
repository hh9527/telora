# RFC 0300：Host/Guest 服务 ABI 与 Guest 位置表

- 状态：实施中，LocId 值布局、Rust 服务 ABI、CLI 字节协议及初始化诊断接口已接通；数据模块与测试加载路径、解析诊断质量仍待完成
- 跟踪：[#209](https://github.com/hh9527/telora/issues/209)
- 分支：`feat/rfc-0300-guest-abi`
- 日期：2026-09-16
- 关联：RFC 0291、0292、0299
- 修订：RFC 0293 的运行时位置布局；保留 RFC 0293、0294 的 EOL 语义

## 动机与范围

运行时传播来源身份，但通常不读取来源的完整坐标。目前值头包含 12 字节紧凑 Loc
和 4 字节 TypeId；紧凑位置又将每个端点限制为 16 bit 行号、24 bit 行内偏移。
扩大行号会挤占单行容量，直接扩大内联 Loc 则会增加每个运行时值的成本。

本 RFC 用 u32 LocId 替代运行时内联坐标，将完整位置集中存放在 Guest 线性内存的位置表中。
静态 Locs 和来源名称随 Wasm data segment 生成，实例化后即可查询，不需要 Host 安装。
同时定义 Guest 内存、数据源注入、服务创建和查询 ABI，使动态输入也能使用相同的
位置身份传播机制。解析及数据树构建仍在 Guest 内，不把数据树来回复制到 Host。

本 RFC 在独立分支实施运行时改造，不决定 Wasm 发布容器或旁文件格式，
不增加用户态 I/O，不改变 TransformService 的语言契约。Rope 是否保留是独立决策。

## ABI 总览

以下名称为拟定的 Wasm 导出/导入名称，u32 在 Wasm 中使用 i32 位模式，
多个结果通过调用方提供的可写结果描述符返回，不使用 Wasm multi-value 或 Rust tuple ABI。

Guest exports：

```text
mem-alloc(cap: u32, align: u32) -> ptr: u32;
mem-free(ptr: u32, cap: u32, align: u32);
mem-realloc(ptr: u32, old_cap: u32, new_cap: u32, align: u32) -> ptr: u32;

get-data-source-count() -> u32;
get-data-source-name(i: u32, result: u32); // 写入 [id, name, name_len]
set-data-source(id: u32, data: u32, data_len: u32, fmt: u32);

create-service() -> error: i32;
run-service(
    ptr: u32, len: u32, out: u32, out_cap: u32, result: u32
); // 写入 [out, out_len, out_cap]
```

result 指向调用方持有的 12 字节、4 字节对齐的可写区域，由三个小端 u32 构成。
该区域仅在同步调用期间借用，调用返回后 Host 读取结果；可在后续调用中重复使用。
它不能与输入借用或 move 的输出分配重叠。trap 时描述符内容不可用，不代表所有权返回。
名称指针是 Guest 只读借用；run-service 写回的输出指针则将分配所有权交还 Host。

服务生命周期、输入注入、Context 构造和状态管理用 Rust RT 实现并直接导出 C ABI。
codegen 只根据封闭 MIR 提供布局常量及函数入口，链接器将这份固定描述信息嵌入制品。
不为服务初始化手写 Wasm 状态机，不为这些接口维护 multi-value 包装层。

本协议不引入位置相关的 Host import，也不提供 set-locs；Locs 由 Guest 自主管理。
fmt 使用固定编号：1=JSON、2=YAML、3=TOML；数据均为 UTF-8。
未知 fmt 是 ABI 违约，trap；合法 fmt 下的非法 UTF-8/语法错误属于输入诊断。
查询请求/结果使用下述 JSON 协议；初始化诊断通过 get-service-diagnostics 获取。
这些未定项在 ABI 落地前必须补齐；本草案不声明已经可以独立互操作。

## 内存与所有权

### 指针、容量与长度

缓冲区指针非零；align 为非零的 2 的幂，使用 Rust Layout 可表示的 size/alignment。
cap、len 均以字节为单位，cap 无需为 align 的整数倍，len 不超过所属分配容量。
名称、输入数据均为指针/长度描述的字节序列，不要求 NUL 结尾。

零容量空缓冲区统一为 (ptr=align, cap=0)。这是非零、满足对齐的哨兵，不可解引用，
无需在该地址保留分配。字节缓冲区使用 align=1，空缓冲区即 (1,0)；
align=8 时仍为 (8,0)。有效长度为 0 的缓冲区也可保留非零容量。

Host 必须传递真实分配的容量及对齐。Guest 检查 Layout、指针对齐、范围加法溢出
和线性内存边界；非法参数、非法状态及分配失败 trap。范围检查不证明分配所有权：
精确容量、原始对齐以及禁止重复释放仍是调用者义务，不额外维护分配映射表。

### 内存函数

- mem-alloc(cap, align) 使用 Rust 全局分配器分配未初始化内存；cap=0 返回 align。
- mem-free(ptr, cap, align) 消费所有权，按原始 Layout 释放；零容量为空操作。
- mem-realloc(ptr, old_cap, new_cap, align) 消费旧所有权并返回新所有权，
  保留前 min(old_cap,new_cap) 字节，新增字节未初始化；对齐保持不变。
- 从零容量扩容等价于分配；收缩到零容量释放原分配并返回 align。
  如需改变对齐，调用者显式分配、复制和释放。

这些函数和 Guest 的 Vec/String 使用同一套 Rust 全局分配器。底层 arena 分配原语
只供全局分配器调用；不建立另一套 Host 专用分配器。free 是否立即回收物理空间
取决于全局分配器策略，现阶段仍按 arena 生命周期整体回收。

只有明确 move 的接口才允许接收方重建拥有所有权的 Rust 容器。重建 Vec<T> 时，
实际分配布局须与 T 的对齐和元素容量一致，所有有效元素均已初始化；
例如 Vec<u8> 使用 align=1，字节 cap 就是元素容量。不能仅因地址碰巧按 8 对齐，
就把按 Layout(cap,8) 分配的内存当成默认 Vec<u8>。
借用接口不转移所有权，不能据此使用 Vec::from_raw_parts 接管输入。

### 服务输入和输出

run-service 的 ptr/len 在同步调用期间借用，返回后由 Host 管理原输入分配。
out/out_cap 则是完整的 move：调用时 Host 交出所有权，Guest 可复用、释放或替换它。
正常返回时 Host 获得返回的 out/out_cap 所有权，out_len 不超过 out_cap。
输入借用不能与转移所有权的输出分配重叠。

服务输出是 align=1 的字节缓冲区，首次输出传 `(1, 0)`；后续可将返回缓冲区再次 move 给 Guest。使用结束后调用
`mem-free(out, out_cap, 1)`。返回指针是否与原指针相同不影响所有权语义。
trap 时没有所有权返回，Host 不能释放或重用旧输出指针。
草案采用 trap 后丢弃实例的恢复边界；是否优化成可恢复实例不在本次范围。

Guest 导出的名称是只读借用，不可 mem-free；正常实例生命周期内保持稳定。
Host 不跨 Guest 调用保存依赖旧 memory.buffer 的视图：Guest 调用可能导致 memory.grow，
需要重新获取视图。请求 reset 不得回收 Host 仍拥有的输入/输出分配，
输出缓冲区不能直接指向将被回收的请求临时对象。

## 数据源注入与服务生命周期

1. Host 枚举 get-data-source-count/get-data-source-name 得到稳定的来源 ID 和逻辑名称。
2. Host 按逻辑名称取得输入内容，绑定到预定的 id；不动态注册来源身份或位置表。
3. Host 调用 mem-alloc，将内容写入 Guest 线性内存。
4. Host 调用 set-data-source，Guest 同步解析并构建数据，在自己的 Locs 表中登记 key/value 的位置。
5. 返回后 Host 可重写同一传输缓冲区以注入下一个来源，最后统一释放；Guest 不保留对该传输缓冲区的借用。
6. 所有必要来源注入成功后 create-service 完成初始化，返回 0 表示成功，非 0 表示失败。
7. 初始化成功后 Host 多次调用 run-service，复用输出缓冲区。

一个 Guest 实例只承载一个服务实例。服务值及其类型完全保留在 Guest 内，
Host 不接收 TypeId、实例 ID 或服务句柄；多个服务由多个 Guest 实例承载。
错误码只表达初始化状态，详细诊断通过诊断协议获取，Wasm trap 由 Host 单独捕获。
初始化失败不能进入查询阶段；成功后重复创建属于非法调用状态。
初始化失败后不支持重试或替换数据来恢复；再次读取失败状态返回 1，重新初始化需要新实例。
服务随 Guest 实例销毁，不另设服务句柄表或逐服务销毁接口。

TransformService 声明哪些 source 可注入；相应 property 求值及注入信息生成到 Wasm。
模块初始化后，Guest 从该封闭 entry 的 property 结果建立来源清单并分配槽位 ID，
不要求编译器把普通 property 计算简化为语法常量；Host 不分配这些 ID。
来源清单遵循 RFC 0299 的稳定名称排序。静态模块来源与外部来源使用不冲突的
SourceId 空间；set-data-source 的 id 直接标识预定槽位，Host 只根据清单填充内容。
预定的是来源身份及注入槽位，不是未知输入中每个 key/value 的具体位置；后者在 Guest 解析时产生。
创建服务前拒绝重复或缺失来源；服务创建后不允许替换初始化来源。
枚举接口是否覆盖静态数据模块的外部供应场景，落地时需与制品打包策略明确区分。

初始化来源的位置表跟随服务实例存活，不在请求 reset 时释放。
服务仍遵循每次查询独立的 fuel/memory 限制及确定性起点；本 RFC 不承诺具体
heap truncate 算法，也不把多个查询累加成一个资源计费范围。

## LocId 与位置表

### 运行时布局

```text
LocId:  u32
TypeId: u32

value header:
  +0  loc: LocId
  +4  ty:  TypeId
  +8  payload ...
```

值头由 16 字节降为 8 字节；若 payload 仍占一个 u64，标量从 24 字节降为 16 字节。
这是拟定物理布局，不是已经测得的总体内存收益。容器、函数环境、诊断记录和
GC 的实际大小须逐一更新，不可假设只替换 HEADER_BYTES 就完成迁移。

LocId 0 表示无来源，其余 ID 索引 Guest 的位置记录；这与缓冲区 ptr 禁止为 0 无关。
LocId 不是线性内存地址，不参与类型推断。
复制、来源传播、blame 链只复制 ID；with_diagnostic 在 Guest 内查表展开坐标。

完整记录在逻辑上包含 SourceId 以及起止 `(line: u32, utf8_offset: u32)`，
行号和偏移从 0 开始，范围左闭右开。首版表项采用五个小端 u32，共 20 字节：
`[source, start_line, start_offset, end_line, end_offset]`，不使用 Rust 默认结构体布局作为跨边界约定。
来源名称使用单独的 SourceId 索引表，也生成到制品；不必嵌入源码全文。
不再使用 u16 来源数及 16/24 的行列位分割；仍需检查 u32 的可表示范围和资源配额，
不能用更宽的位置字段声称任意大小输入都可接受。

### LocId 编码与查表

```text
0                         无来源
0x8000_0000 | index        静态表，index 为低 31 bit
index + 1                 初始化表，最高位为 0 且 ID 非零
```

静态表最多可编码 2^31 项；初始化表最多可编码 2^31 - 1 项。
实际容量还受 Wasm32 地址空间及内存限制约束，不能按编码容量直接分配。
低位表达表项索引而非字节偏移，地址计算为 `base + index * 20`。
查表须检查索引范围和地址运算溢出；不能截断后访问错误记录。

静态表基址是编译时确定的常量。初始化表通过 Guest 全局基址及表长访问，
追加扩容只更新基址和容量，已有 LocId 不变。初始化完成后表固定，
不使用通用多区间查找，也不需要请求级第三张位置表。

### 静态位置

编译时按确定性顺序分配静态 LocId。同一源码位置经泛型实例化、指令生成、
运行时执行后仍复用身份，不按执行次数增加表项。
表项合并是构建期策略，不要求运行时每次传播时查询 HashMap。

静态位置记录直接生成到 Wasm data segment，实例化时进入线性内存。
Guest 持有并管理该表，不经 Host 的传输缓冲区所有权协议，也不调用 set-locs。
静态表及来源名称不可被请求 reset 回收；它们与代码属于同一制品并共享 ABI 版本。
本版不支持独立位置表文件或按需加载，Host 无需为普通位置展开维护镜像表。

### 注入数据的位置

--source 的身份、名称和可注入槽位在编译时确定，内容在 set-data-source 时才确定。
Guest 解析传入内容，将原始 UTF-8 字节范围转换成起止行/字节偏移，在初始化位置表
中追加记录，并把返回的 LocId 写入解析出的值和 key。该过程不调用 Host。

完整坐标在借用的输入缓冲区释放前确定；解析结果必须拥有所需的字符串内容，
不得继续借用 Host 即将释放的传输缓冲区。临时行索引可以在登记完成后释放。
初始化位置与注入数据、服务值同生命周期，不随单次请求 reset。

### 临时 parse

普通字符串解析不注册独立来源，不生成位置表项，也不访问 Locs 表：
结果直接继承输入字符串的 LocId。解析错误可附加字符串内相对行/偏移，
但 blame 仍指向输入字符串来源。后续 with_diagnostic 需要展开该来源时，
才根据继承的 LocId 选择静态表或初始化表。

run-service 的请求内容不新增来源身份或位置表；没有可继承来源时使用 LocId 0，
解析错误的请求内相对坐标作为诊断附加信息输出，不冒充已有来源坐标。
本版不提供请求级临时 Locs 表，也不需要位置槽复用或请求位置 ID 回收。
LocId 仅在所属 Guest 实例内有意义；连续请求及临时 parse 不增加两张表的表项数量。

### Guest 诊断生成

with_diagnostic 在 Guest 内捕获诊断、查询 LocId 对应的完整坐标和来源名称，
生成结构化诊断结果，再通过服务响应或初始化诊断接口交给 Host。不需要逐位置 Host 回调，
也没有“发生错误后再安装表”的时序要求。

语言诊断的 `SourceRange` 使用 `{source: String, start: SourcePoint, end: SourcePoint}`，
其中 `SourcePoint = {line: Int, offset: Int}`，各分量的有效范围为 u32。
不再暴露打包后的整数端点，不保留旧端点编码的兼容解码；
这也确保 JSON/JavaScript 在最大坐标下仍能精确表示两个分量。

Host 负责终端或 Web 展示。源码摘录、终端宽度和 UTF-16 转换如有需要，
由 Host 根据其保留的原文处理；Guest 的规范坐标始终是行号和行内 UTF-8 字节偏移。
Host 可将预定逻辑名称关联到实际文件显示名称，不将本地路径注入编译器身份。

### EOL 与诊断可用性

CRLF、LF、单独 CR 均为一次换行，CRLF 内部边界按既有规则映射到前一行末尾。
源码字符串的实际换行仍遵循 RFC 0294；不归一化数据字符串的语义内容。
静态 ID 分配不能依赖原始字节偏移的数值编码，确保等价 EOL 源码的 ID 和
逻辑位置表一致。原始字节范围只作为解析和动态登记时的桥接信息。

非零 LocId 必须可在 Guest 中解析，缺失表项属于内部一致性错误，不存在正常的未加载状态。
LocId 0 明确表示无来源，不伪造坐标。静态及初始化位置始终随实例保留，
输出的结构化诊断不依赖 Host 在之后访问 Guest 请求临时数据。

## 查询序列化协议

run-service 输入是 UTF-8 JSON，表示一个 std/value.Value；输出也是 UTF-8 JSON，
采用 `{schema: "telora.service/v1", ok: Value, error: Bool, diagnostics: Array(Diagnostic)}`。
成功时 error=false，ok 是转换结果；语言失败时 error=true，ok=null。
诊断遵循 std/_rt.Diagnostic 的封闭结构，包含完整 SourcePoint，不传递打包坐标。
返回长度界定 JSON 文本，不附加 NUL 或 JSONL 换行；流式 Host 自行添加行分隔。

内置语言 entry 使用 with_diagnostics 捕获转换诊断，并通过已封闭类型的 codec 和
json.stringify 生成响应。Host 只解析协议文本，不读取服务值、拆解 Result 或重建诊断。
Wasm trap 不保证产生响应，由 Host 捕获并丢弃本次实例状态。
输入解析失败和结果无法 JSON 编码时也形成失败响应，不能把普通语言错误当成 ABI 违约。
输入解析失败保留多条独立诊断。注入来源的标签包含来源名称及完整坐标；
临时请求的输入内相对坐标放入 notes，以零基行号及 UTF-8 字节偏移表达，
不冒充持久来源，不新增来源或 LocId。

## 初始化诊断与失败协议

`get-service-diagnostics(out: u32, out_cap: u32, result: u32)` 获取初始化诊断。
输出缓冲区沿用 run-service 的 move 语义和 align=1 分配契约；result 是调用方
提供的 12 字节、align=4 可写描述符，接收 `[out, out_len, out_cap]`。
返回 UTF-8 JSON 数组，每项遵循 std/_rt.Diagnostic 的结构。Guest 展开 LocId，
Host 无需读取语言值或解释位置表。读取不消费诊断，重复读取保留原有记录。

在来源枚举触发准备后、来源注入期间以及 create-service 完成后均可读取；
来源解析失败和缺失来源都记录为初始化错误。create-service 返回 0 成功、1 失败，
失败后不得发布初始化快照或开始查询，也不允许替换输入后重试。
该接口用于初始化阶段，不作为查询诊断接口；查询诊断由 run-service 响应携带。
Wasm trap 后不得继续读取诊断或释放所有权不确定的缓冲区，应丢弃或重置实例。

ABI 版本由制品元数据声明，Host 必须拒绝不支持的版本，不提供旧 ABI 兼容路径。
来源名称采用 UTF-8 字节序列，由 name_ptr/name_len 表达，不以 NUL 结尾；
不需要独立位置旁文件的配对协议。

ABI 违约和 Wasm trap 与可收集的语言诊断不同。配额继续以可停机为目标，
不追求精确计费，不借本次 ABI 改造增加复杂配额机制。

## 备选与延后

- 不采用扩大内联 Loc：避免每个值承担完整坐标的成本。
- 不采用将 u32 拆成 u16 文件号和 u16 文件内位置：大数据文件可能超过 65536 个位置。
- 首版不做位置表外置、懒加载、set-locs 或 Host 逐位置登记回调；以后有测量依据再独立设计。
- 不要求 Guest 永久保留输入原文或行起点索引；完成坐标登记后可释放解析辅助信息。
- 不把字符串解析结果转换成 Host 数据树，也不为临时字符串自动建立独立源文件。
- 不保留旧 12 字节运行时 Loc ABI 的兼容执行路径；旧制品通过版本检查拒绝。

LocId 不保证所有场景总内存减少：大数据树每个位置仅引用一次时，位置表可能增加
总空间。首版的表常驻 Guest 并计入线性内存限制；收益是值头和传播成本降低，以及
诊断生成自包含。不能再以“位置表不加载”作为本版收益。

## 实施计划

1. 补齐失败/序列化协议；统计真实项目不同来源范围数量和当前运行时位置携带量，
   区分静态代码、静态数据、外部初始化输入，记录基线。
2. 将确定性静态 Locs、来源名称及注入槽位嵌入 Wasm，接入 Guest 注入位置登记和
   with_diagnostic 查表；验证坐标、EOL 和生命周期，不更改语言类型语义。
3. 统一切换共享 ABI、codegen、Rust RT、GC/复制、诊断导出和 Host/JS 消费者到 LocId。
   调整所有依赖旧头偏移的指令，提升 ABI 版本并删除旧布局解码路径。
4. 接入内存函数、数据源枚举/注入、服务创建/执行，完成 CLI 使用同一 Host 协议的总装。
5. 更新当前设计文档并记录真实项目的初始化/查询时间、Host/Guest 内存、Locs 表项数量、
   常驻字节数和制品大小；不将本次验收依赖于未来发布格式或懒加载设计。

## 可执行的验收条件

- 内存 ABI 覆盖对齐对应的空哨兵、合法/非法 Layout、零长度但非零容量、扩缩容内容保留、
  容量溢出、错误范围、move 后所有权及 trap 后实例废弃；memory.grow 后 Host 正确更新视图。
- Wasm 自带静态 Locs、来源名称和注入清单；不依赖 set-locs 或 add-source-loc，
  Guest 的 with_diagnostic 可以独立输出包含逻辑来源名称及完整行列的结构化诊断。
- 新表覆盖超过 65536 行及长单行 JSON 输入；大值边界用小型坐标测试验证，避免构造数 GiB 测试资产。
- LF/CRLF/CR 等价源码产生一致的静态 ID、逻辑位置表和程序结果；Unicode 使用 UTF-8 字节偏移。
- Guest 解析外部 JSON/YAML/TOML 时，value/key 位置登记在 Guest 表中并可精确展开；
  同一预定槽位接受不同内容时仍给出正确坐标，非法数据保持既有诊断质量。
- 字符串内解析沿用原 LocId，不按节点新增来源；enum literal 和普通值的 blame 传播不回退。
- 验证最高位静态标记、初始化索引加一、无来源 0、索引及地址溢出；初始化表扩容后旧 ID 仍有效。
- 静态及初始化位置在请求 reset 后有效；连续请求及临时 parse 不新增位置表项，
  解析过程不查询 Locs，而诊断生成仍正确展开继承的位置。
- 独立语言用例覆盖服务初始化、查询成功、语言失败；Rust 测试集中于 ABI、所有权和布局边界，
  不用 include_str! 嵌入大型语言资产。
- Ontology 的 check --lib 与真实模型服务可运行，记录端到端性能和 Host/Guest 内存变化，
  不以局部值头缩小代替实际测量结论。
- 验证 create-service 成功返回 0、失败返回非 0 并提供诊断；未成功初始化不能查询，
  成功后重复创建被拒绝；不同 Guest 实例的服务相互隔离。
