# RFC 0304：布局驱动的值与跨实例封闭

- 状态：实施中
- 日期：2026-09-21
- 跟踪：[#216](https://github.com/hh9527/telora/issues/216)
- 前置：RFC 0299、RFC 0300、RFC 0302、RFC 0303
- 讨论稿：[静态类型值、紧凑来源与 Wasm 栈式调用约定](../discuss/typed-values-locations-and-wasm-stack.md)
- 修订：替代 RFC 0302 的单 `Vec<u64>` 物理堆和 ABI 25 的逐值 TypeId；保留其保守初始化根与请求边界 reset 语义

## 摘要

sealed MIR 已经确定每个运行时值的类型和每个容器的字段布局。Wasm 后端不再为
普通值重复保存 `src/start/end/TypeId` 自描述头，也不再通过普通对象分类表解释
引用。codegen、Guest RT、Host sealing 和诊断共同消费一份封闭 layout image。

初始化仍允许单调分配并产生临时垃圾。service 就绪后，Host 从显式根集合驱动
一次跨 Wasm instance copy-collect：在新实例底部顺序重建存活图和 Rust RT 资源，
记录 frozen checkpoint，随后销毁旧实例。请求期间只在 checkpoint 之后增长；
正常请求结束恢复 checkpoint，trap 后以同一逻辑快照重建新实例。

本 RFC 不引入执行期 GC、逐对象 free、引用计数、写屏障或精确活跃栈图。

## 动机

当前 ABI 25 的每个值包含 12 字节来源和 4 字节 TypeId。结构化对象位于单一
`Vec<u64>`，由 table id 间接访问。初始化 collector 能把业务模型约 184 MB 的
临时语言对象压缩至约 2.91 MB，但旧 Wasm instance 已增长的线性内存不能缩小，
Host 还曾复制约 400 MB 整体内存作为 trap baseline。逻辑回收没有转化为物理
内存收益。

逐值 TypeId 也重复了 sealed MIR 已经知道的事实，并迫使栈上临时值、字段和数组
元素都采用统一大头部。table id 让每次字段访问多一次定位，且把普通对象与真正
需要资源管理的 Regex 混为一类。

## 决定

### 唯一布局事实

每个可运行 TypeId 对应一条 layout image entry：

```text
StorageLayout = value bytes/alignment + data shape + field/variant details
TraceLayout   = reference edges + dynamic tags + resource reconstruction rule
MachineShape  = Wasm parameter/result machine types
```

三者由同一 sealed 类型事实生成。编译期类型显式标记为 `CompileTime`，其
`value_bytes=0`，若 collector 或 codegen 尝试物化则立即失败。

layout image 是制品 ABI 的一部分。Host 和 Guest 使用共享解码定义，不分别维护
kind 数字、字段偏移或 variant flags。新 ABI 不兼容 ABI 25，加载器明确拒绝旧制品；
不保留按版本分叉的长期运行时路径。

### 来源与普通值

来源压缩为一个 `u64`：

```text
src-id: 14 bit, start: 25 bit, end: 25 bit
```

普通静态值的存储是 `{loc: u64, data<T>}`。TypeId 来自函数签名、字段布局、数组
元素布局或根描述，不逐值保存。Dyn 和未来 trait object 保留具体运行时类型身份；
`Type` 值中的 TypeId 是业务数据，不属于通用头部。

逻辑字段按自然宽度描述，物理布局统一按 8 字节对齐。首版来源与数据相邻存储，
不同时引入冷热分列。

### 容器与引用

普通引用是当前 instance 线性内存中的稳定非零 `u32` 地址。`memory.grow` 不改变
数值地址；Host memory view 仍不得跨 Guest 调用缓存。地址不要求跨实例稳定，
sealing 通过 forwarding map 修补。

```text
fixed container = { type_id, fields... }
RawArray        = { type_id, data, len, cap }
RawBytes        = { data, len, cap }
environment     = { environment_layout_id, captures... }
```

Record、Tuple、Newtype 和闭包环境直接按地址引用，不再使用普通对象定位 table。
Array/Dict/String/Bytes 的 view 保存 raw 引用及范围。普通元素和字段不重复 TypeId；
Array(Dyn) 的元素保留 concrete TypeId。Regex 等 Rust 对象继续使用专用 handle table。

首轮迁移只确定 `len/cap` 头并按 `len` 复制，不同时实现尾部原地 append。内部可变
buffer 是后续优化，不是新布局正确性的前提。

### 分配与生命周期

运行时只要求以下抽象：

```text
alloc(layout) -> u32
checkpoint() -> Frontier
reset(Frontier)
```

底层可使用连续 bump 或由普通 allocator 提供的稳定 chunk。普通对象没有独立
释放和析构。初始化实例只有单调区域；新 service 实例的 compact 数据形成 frozen
prefix，其末端是请求 checkpoint，同一地址空间后缀是 work region。

frozen 对象不得被请求写入。请求产生的引用不得写回 frozen 根或跨请求逃逸。
Array/String 的 frozen raw 首次增长必须 fork 到 work region。

### 跨实例 sealing

初始化完成后执行：

```text
old explicit roots
  -> TraceLayout traversal
  -> forwarding maps
  -> allocate/copy in fresh instance
  -> rebuild resource handles
  -> restore explicit state slots
  -> record frozen checkpoint
  -> discard old instance
```

根集合保守包含 service handler、Ready demand/property/top-level 槽、初始化诊断与
debug 状态以及 Host 显式根。静态类型元数据位于制品 image，不进入图遍历。

普通对象按布局复制并保持共享/循环。String/Bytes 复制可见 raw envelope。Regex
不能复制 Rust 内存：快照保存 pattern，新实例重新 compile，并以
`old handle -> new handle` 修补引用。

来源名称和 BOL、service source 描述、service phase、handler、mutable globals 与
资源表基线属于 `InstanceStateDescriptor`，不得依赖 Host 猜测任意 Rust static。

跨实例复制和持久 snapshot 使用同一图遍历，只替换 target sink。此次只承诺实例
迁移使用的内部逻辑快照，不承诺发布格式稳定。

### 请求 reset 与 trap

正常请求完成后：

```text
language frontier = frozen checkpoint
resource tables.truncate(frozen resource baseline)
restore generated mutable globals
```

不执行可达性扫描。trap 可能跳过 Guest 清理，因此 Host 丢弃 poisoned instance，
新建实例并导入初始化逻辑快照。两条路径必须得到相同 frozen 状态。

## 调用约定

内部生成函数最终采用具体 `MachineShape` 和 Wasm multi-value。来源参数先分组，
数据 words 随后按签名排列；需要报告计算位置的 builtin 使用独立 `compute_loc`。
Rust Guest RT/Host 边界继续使用显式参数、单 status 返回和 Guest 返回缓冲区。

调用约定迁移可晚于存储布局，但不得重新引入通用 boxed value 或逐值 TypeId。

## 实施顺序

1. 共享、版本化 layout image，现有 collector 先改为消费它。
2. Guest 导出 compact 逻辑状态；新 Guest 导入 heap/content/state，重建 Regex。
3. `TransformSession` 与 `telora-run` 使用跨实例 sealing，删除整块线性内存 baseline。
4. layout image 切换到 8 字节 Loc、无逐值 TypeId 的 StorageLayout。
5. codegen/RT 按类型族迁移构造、投影、比较、codec、诊断和 collector，删除普通对象 table。
6. ABI 升级并删除 ABI 25 兼容代码。
7. 用真实 service 验证时间、线性内存、RSS、快照尺寸和持续请求 reset。

步骤 1～4 与 ABI 26 的普通值迁移已在 `explore/216-cross-instance-collect` 通过
`telora-wasm` 全量测试：值头仅含 8 字节 Loc，普通值不再保存 TypeId；demand、service、
闭包环境和异构运行时容器显式携带所需布局身份。正式合入仍以普通对象 table 迁移、
workspace/真实模型验证和性能数据完成为准。

## 验收

- layout image 能完整描述所有可物化 sealed 类型；编译期类型不可误入运行时。
- 初始化前后值、诊断、来源和函数身份一致。
- shared/cyclic graph 只复制一次且引用全部修补。
- 初始化保留的 Regex 在新实例重建，旧 Rust 指针不进入快照。
- 新实例线性内存与存活数据相称，不保留旧实例初始化峰值。
- 正常请求 reset 不扫描对象；trap 恢复结果与正常 reset 一致。
- `telora-wasm`、CLI/service 测试和 ontology/imaster-cloud 真实模型通过。
- 记录 code size、构建时间、初始化/迁移时间、首请求与稳态请求、线性内存与 RSS。

## 不包含

- 执行期间的周期、并发或分代 GC。
- 精确裁剪 property 根。
- Array/String 尾部原地 append 与 alias/last-use 优化。
- 稳定发布 snapshot 格式。
- Wasmtime 或浏览器运行时迁移。
