# RFC 0300：Host/Guest 服务 ABI 与内联来源范围

- 状态：实施中；服务 ABI 保留，撤销 LocId 方案，待将实现统一迁回内联 src/start/end
- 跟踪：[#209](https://github.com/hh9527/telora/issues/209)
- 分支：`feat/rfc-0300-guest-abi`
- 日期：2026-09-16
- 关联：RFC 0291、0292、0299
- 修订：替代 RFC 0293 的运行时位置编码及跨 EOL 制品一致性要求；保留 RFC 0294 的源码多行字符串换行语义

## 动机与范围

运行时以三个 u32 携带来源范围：SourceId、起始字节偏移、结束字节偏移。
此前为消除 EOL 差异采用打包行列，又因行数容量限制尝试 LocId；
位置表带来的登记、冻结、扩容和回收协调成本超过当前已证明的收益。
本次修订回到直接引用实际输入文本的 UTF-8 字节范围，不再压缩为位置 ID。

保留已经建立的 Host/Guest 内存所有权、数据源注入、服务创建和查询 ABI。
解析及数据树构建在 Guest 内，不把数据树来回复制到 Host。
来源名称和行首索引按来源保存，仅在生成诊断时将字节范围转换为行列。
LF/CRLF/CR 输入造成的制品字节差异不属于本 RFC 的一致性要求，
未来提供物化的 telora build 时再定义规范化、内容哈希及发布规则。

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
get-service-diagnostics(out: u32, out_cap: u32, result: u32);
// 写入 [out, out_len, out_cap]，返回初始化诊断 JSON 数组
```

result 指向调用方持有的 12 字节、4 字节对齐的可写区域，由三个小端 u32 构成。
该区域仅在同步调用期间借用，调用返回后 Host 读取结果；可在后续调用中重复使用。
它不能与输入借用或 move 的输出分配重叠。trap 时描述符内容不可用，不代表所有权返回。
名称指针是 Guest 只读借用；run-service 写回的输出指针则将分配所有权交还 Host。

服务生命周期、输入注入、Context 构造和状态管理用 Rust RT 实现并直接导出 C ABI。
codegen 只根据封闭 MIR 提供布局常量及函数入口，链接器将这份固定描述信息嵌入制品。
不为服务初始化手写 Wasm 状态机，不为这些接口维护 multi-value 包装层。

本协议不引入位置相关的 Host import，也不提供 set-locs 或逐位置登记接口。
fmt 使用固定编号：1=JSON、2=YAML、3=TOML；数据均为 UTF-8。
未知 fmt 是 ABI 违约，trap；合法 fmt 下的非法 UTF-8/语法错误属于输入诊断。
查询请求/结果使用下述 JSON 协议；初始化诊断通过 get-service-diagnostics 获取。
实现及 Host 消费者须遵循同一 ABI 版本，不兼容旧值布局。

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
4. Host 调用 set-data-source，Guest 同步解析并构建数据，把 src/start/end 直接写入 key/value 的来源字段。
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

初始化来源的名称及行首索引跟随服务实例存活，不在请求 reset 时释放。
服务仍遵循每次查询独立的 fuel/memory 限制及确定性起点；本 RFC 不承诺具体
heap truncate 算法，也不把多个查询累加成一个资源计费范围。

## 内联来源范围

### 运行时布局

```text
Loc = { src: u32, start: u32, end: u32 }

value header:
  +0  src:   u32
  +4  start: u32
  +8  end:   u32
  +12 ty:    TypeId
  +16 payload ...
```

三个位置字段按小端 u32 存储，Loc 共 12 字节，值头共 16 字节；
单 u64 payload 的标量共 24 字节，双 word payload 共 32 字节。
具体类型继续由封闭布局决定，不能假定只修改 HEADER_BYTES 就完成迁移。
src=0 表示无来源，此时 start/end 为 0；它与缓冲区指针禁止为 0 无关。

start/end 是实际输入文本中的 UTF-8 字节偏移，范围左闭右开；
要求 start <= end <= 输入字节长度，并检查 u32 容量。
不再划分行号位宽或行内偏移位宽，也不把位置编码成单个 u64。
值复制、来源传播和 blame 直接复制三个字段，不执行位置 interning 或查表。

### 来源元数据与诊断

只保留按 SourceId 索引的来源名称、文本长度及换行索引，不保留逐位置 Locs 表。
静态源码的名称和诊断所需换行索引随 Wasm 生成；Guest 解析数据时建立对应索引。
转换规则识别 LF、CRLF、单独 CR 为一次换行，CRLF 内部边界映射到前一行末尾。
索引须足以执行该规则，例如同时记录行首和去除换行符后的行尾。
这些辅助信息不要求保留整份原文，也不要求 Host 安装位置表或逐位置回调。

with_diagnostic 在 Guest 内读取内联范围，按来源索引转换为结构化诊断。
对外 SourceRange 继续采用
`{source: String, start: SourcePoint, end: SourcePoint}`，
SourcePoint 为 `{line: Int, offset: Int}`，使用零基行号及行内 UTF-8 字节偏移。
每个分量均为 u32 可表示的数值，JSON/JavaScript 无需处理打包 u64。

Host 负责源码摘录、终端显示宽度和 Web UTF-16 转换；逻辑来源名不包含本地路径。
非零 src 必须有相应来源元数据，缺失属于内部一致性错误。
Guest 返回的诊断不依赖 Host 在之后读取请求临时内存。

### 静态、注入和临时输入

静态位置由编译器直接写入；相同位置的多次执行不产生登记动作。
set-data-source 的来源身份与预定槽位仍由 Guest 提供。
Guest 将实际输入中的 key/value 字节范围直接写入值，不创建位置表项。
解析结果拥有必要的字符串内容及来源索引，不能借用 Host 即将释放的传输缓冲区。

普通字符串解析沿用输入字符串的整个 Loc，不为解析结果创建独立来源；
解析错误可附加字符串内部的相对坐标，blame 仍指向输入字符串。
run-service 请求不新增来源身份，没有继承位置时使用全零 Loc。

测试 fixture 可以在测试调度中加载新来源；来源元数据与仍存活的值一起保留或回收。
不需要位置表追加许可、解冻阶段或专门的 LocId 重定位。
服务创建后仍禁止替换初始化来源，这属于服务契约，不是位置编码约束。

### EOL 边界

解析器引用实际输入，不要求先把整份源码或数据转换为 LF。
相同逻辑文本使用不同 EOL 时，start/end 和编译制品允许不同；
本 RFC 不要求跨 EOL 的静态 ID、位置字节或制品哈希相等。
未来 telora build 的发布规约再处理这一问题。

源码多行字符串中的实际换行继续按 RFC 0294 产生 LF，这是字符串值语义，
与位置引用的原始字节和制品一致性分开处理。数据字符串遵循各自格式语义，
显式转义不因本次布局改动而改写。

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
不冒充持久来源，不新增来源身份。

## 初始化诊断与失败协议

`get-service-diagnostics(out: u32, out_cap: u32, result: u32)` 获取初始化诊断。
输出缓冲区沿用 run-service 的 move 语义和 align=1 分配契约；result 是调用方
提供的 12 字节、align=4 可写描述符，接收 `[out, out_len, out_cap]`。
返回 UTF-8 JSON 数组，每项遵循 std/_rt.Diagnostic 的结构。Guest 转换内联来源范围，
Host 无需读取语言值或解释运行时布局。读取不消费诊断，重复读取保留原有记录。

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

- 撤销 u32 LocId、静态/初始化位置表、最高位标记及逐位置登记接口。
- 不扩大为内联五个 u32 行列字段，不再使用 16/24 bit 打包端点。
- EOL 规范化、可复现发布及内容哈希由未来 telora build 设计处理。
- 不为旧位置布局保留兼容执行路径；提升 ABI 版本，拒绝不兼容制品。
- Rope 是否保留、运行时分配器进一步优化及发布容器均不属于本次修订。

值头相对 LocId 方案增加 8 字节，这是明确接受的成本。
收益是省去每个位置的表项、登记及生命周期协调，不预先声称总内存或性能改善；
以真实项目测量为准。

## 实施计划

1. 先修订本 RFC，保留已实现的服务 ABI、所有权和 Guest 解析成果。
2. 统一修改共享布局、codegen、Rust RT、复制回收和 Host/JS 消费者，
   恢复内联 src/start/end，并提升 ABI 版本。
3. 删除 LocId 分配、编码、位置表 bootstrap/append/freeze 及相关缓存；
   按来源保留诊断所需的名称与换行索引。
4. 完成静态数据、--source、临时 parse 和测试 fixture 的 Guest 加载与来源传播，
   验证生命周期；不以保留旧 Host 数据树构建作为兼容退路。
5. 更新相关文档和测试，运行真实 Ontology check --lib 与模型服务，
   记录初始化/查询时间、Host/Guest 内存及制品大小。

## 可执行的验收条件

- 内存 ABI 验证合法/非法 Layout、对齐空哨兵、零长度非零容量、扩缩容内容保留、
  容量溢出、范围、move 和 trap 后实例废弃；memory.grow 后重新获取视图。
- 所有运行时位置使用三个 u32，TypeId 位于 +12，payload 从 +16 开始；
  不存在 LocId 表、逐位置登记、冻结或旧布局兼容路径。
- 字节范围验证无来源、空范围、Unicode 边界、start/end 顺序及 u32 容量；
  覆盖超过 65536 行和长单行输入，大边界使用小型合成测试。
- LF/CRLF/CR 均能正确定位诊断，不要求制品字节相同；
  源码多行字符串仍产生既定 LF 值。
- Guest 自带静态来源名称及换行索引，注入来源在 Guest 解析时建立索引；
  with_diagnostic 无 Host 位置回调即可输出正确名称及行列。
- JSON/YAML/TOML 的 value/key 保存真实字节范围；非法数据保留多诊断和精确标签。
- 临时解析继承输入 Loc；请求不新增来源；enum literal 和普通引用的 blame 语义不回退。
- 来源元数据在初始化快照、请求 reset、复制回收和嵌套 fixture 工厂中保持有效，
  不回收仍被值引用的来源，也不残留指向已回收内存的索引。
- 独立 .telora 用例覆盖初始化、查询、语言失败和 fixture；Rust 测试集中于 ABI、
  所有权及布局，不使用 include_str! 嵌入大型语言资产。
- create-service 成功返回 0、失败返回 1 并提供诊断；失败不能查询或重试，
  成功后不能重复创建；不同实例相互隔离。
- Ontology check --lib 与真实模型服务通过，记录完整性能和内存数据。
