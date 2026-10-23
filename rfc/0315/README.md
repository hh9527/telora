# RFC 0315 实现阶段记录

## 独立分配与保活原型

`allocation-prototype.rs` 通过 Rust 默认分配器逐对象分配，使用原始 Layout 配对
释放。登记结构先预留再获取对象存储，避免登记失败遗留未登记分配；内部引用定位到
所属分配，由 mark/sweep 决定保留和释放。对象地址在其他对象分配后保持稳定。

```sh
rustc --edition=2024 --test rfc/0315/allocation-prototype.rs \
  -o /tmp/telora-rfc0315-allocation-prototype
/tmp/telora-rfc0315-allocation-prototype
rustfmt --edition 2024 --check rfc/0315/allocation-prototype.rs
```

2026-10-23：4 项宿主原型测试通过，覆盖独立分配、对齐和零初始化、内部根、
资源释放一次、长期 actor/property 根跨 100 次临时对象收集保活、撤销根后回收、
范围及配额拒绝、登记失败原子性。原型不使用 freeze 或 truncate。

这是分配/生命周期语义原型，不是已接入 Wasm 的实现，不证明全图类型扫描、
真实 actor 调用、地址编码、性能、GC 工作内存预算、静态镜像或制品已验收。
线性登记查找只用于验证所有权，生产实现需选择可测量的区间索引；对象范围只接受
真实字节，one-past 地址不能用于保活原对象，即使它可能是另一有效对象的起始地址。

## 接入审计

- `rt/heap.rs` 与 Host `output.rs` 当前通过 ORIGIN/WORDS_VIEW 转换逻辑引用；
  两端必须同时迁移，data image/transport 不能继续解释为单 words 偏移。
- `frame_scratch.rs` 使用 0x80000000/0xc0000000 逻辑命名空间。原型使用宿主 usize，
  尚未冻结 wasm32 动态/静态/scratch 引用编码，不能直接将原型地址强转 u32 接入。
- `service::execution_roots` 已提供 handler/initializer 根。初始化完成时释放
  initializer、注入暂存根应从 copy collector 中移至显式阶段动作。
- `gc_mark` 当前保守保留 Ready demand；首版保持其保活语义。
- `tables::reset` 混合资源释放、baseline 恢复与 heap truncate。迁移时拆分调用状态
  清理与对象回收，普通调用不依赖 baseline 回滚维持内存正确性。
- `collect_trace` 已共用准确布局扫描和函数 demand 依赖，可演化为独立镜像重整器。
- snapshot 当前序列化整个 words，恢复需要与新分配登记、根和引用编码一同迁移。

下一阶段：冻结 Wasm 引用/区间登记原型，验证强制 GC 与 Host 访问，再接入运行时。

## 生产分配登记与 wasm32 释放验证

新增 `telora-wasm-shared::direct_heap`：默认分配器逐对象零初始化分配，原始 Layout
配对释放；分配登记追加写入，在 tracing 边界原地排序后用二分区间查询内部引用。
登记数组的增长先于对象分配，配额包含显式对齐后的 Layout 与登记 capacity。
默认分配器内部的隐藏开销不计为精确 managed 字节。尚未切换生产 rt/heap。

共享测试 39/39 通过，其中新增 6 项检查稳定地址、零值、内部根、mark 去重、
死亡资源释放一次、分配/GC 交替后的区间定位、长期根、padding 拒绝和预算拒绝。

实际 wasm32/default allocator 探针执行 1000 次各分配 128 KiB 的调用，无 reset、
truncate 或 compact；每次 sweep 只保留 actor/property 两个普通根（56 字节），
撤销根后归零。线性内存从第 20 次起稳定在 1,376,256 字节。此结果验证存储复用，
不代表正式 Telora 服务、全图 scanner 或真实模型性能已经通过。

```sh
CARGO_TARGET_DIR=/tmp/telora-rfc0315-target CARGO_BUILD_JOBS=4 \
  cargo test -p telora-wasm-shared --offline
rustc --edition=2024 --target wasm32-unknown-unknown --crate-type=cdylib -O \
  rfc/0315/wasm-allocation-probe.rs -o /tmp/telora-rfc0315-allocation-probe.wasm
node rfc/0315/check-wasm-allocation.mjs /tmp/telora-rfc0315-allocation-probe.wasm
```

运行时接入仍需同时改造 Host 地址解释、scratch 命名空间、根生命周期以及镜像
重整/重定位。不会在这些边界未完成时将独立分配组件当作完整 RFC 实现交付。

## 生产接入阶段（2026-10-23）

动态对象采用 `direct_heap` 独立分配，普通准确 GC 保持 Guest 地址稳定。
当前 ABI 38 保留 scratch 的高位命名空间：动态地址及对象末端须小于
`0x80000000`，分配时显式检查；不声称支持完整 4 GiB 对象地址空间。
受管配额包含对齐后的对象 Layout 与登记表容量，不包含默认分配器隐藏开销。

Ready demand 与 service handler 保活初始化图。正常 reset 清理调用状态与反馈，
不截断对象存储。快照与数据镜像通过独立连续目标重整，导出后恢复源图及其根；
导入重定位为普通受管分配。Host pin 保活运行时对象，但不自动成为发布根。

隔离验收命令（target 均在 `/tmp`）：

```sh
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target cargo test -p telora-wasm-shared --offline
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target CARGO_BUILD_JOBS=4 cargo test -p telora-wasm --lib --offline
cargo fmt --all -- --check
bash scripts/check-source-size.sh
git diff --check
```

已观察到的性能问题：相同的 10,000 次分配周转、512 KiB 受管配额用例
消耗约 2,470,300,420 fuel、累计分配 10,980,586 字节，超出原 20 亿 fuel
测试预算。当前保留周转规模并提高测试 fuel 至 40 亿，尚需基线对照和优化。
增长序列的 256 KiB 配额用例先耗尽 2 亿 fuel；低配额停止测试改用 64 KiB，
仅证明配额能停止执行，不作为原性能等价证据。

后续验收仍包括结构化分配失败、镜像跨分配历史的确定性、全部语言 fixtures、
CLI/runner/serve 对照及至少三轮真实模型性能测量。本阶段不代表 RFC 完整交付。

### 诊断与镜像补强

ABI 字地址 24 指向六个 u32 的分配记录：阶段、分类、申请字节、受管配额、
当前申请字节总量、对齐存储与登记容量。危险分配前发布记录，成功后清空阶段；
Host 在 trap 后只读线性内存，区分 capacity-overflow、managed-budget、
allocator-failure 和 interrupted，不从 Rust panic 字符串推断原因。阶段包括
普通动态分配和镜像重整。未经过受管分配入口的 Rust 临时分配仍由 Guest 限额控制。

快照中的执行统计保留字写零，避免源实例的分配历史进入制品。测试在额外分配并
pin 64 KiB 以上无关内容后比较快照字节，并验证源 pin 地址稳定。
显式复制回收也保留待消费诊断/debug 事件，直到调用者显式释放反馈。

### 分阶段性能探针

`runtime-probe` 使用固定 wasmi 2.0.0 与相同 fuel 参数，直接调用两版共有的 Guest ABI。
参数依次为普通 Wasm、请求 JSONL 文件、响应 JSONL 输出路径。每次进程创建源实例和
恢复实例；分别记录 initialize、pack、restore、request、restored_request。
request 包括响应复制、ABI 缓冲释放和 reset。每阶段单独设置 2500 亿 fuel；这只是
测量上限，不改变产品默认配额。受管配额为 Guest 默认 32 MiB，总内存限额 64 MiB。
报告当前 managed/committed、累计 allocated_total/collections 与 Guest 内存高水位；
不同实例的累计计数需分别解释。编译 Wasm 的时间不包含在阶段时间中。
探针逐次比较源实例和恢复实例的原始响应，`measure.py` 再比较各版本的 JSON 响应。

```sh
CARGO_TARGET_DIR=/tmp/telora-rfc0315-probe-target cargo build \
  --manifest-path rfc/0315/runtime-probe/Cargo.toml --release --offline
python3 rfc/0315/measure.py \
  --probe /tmp/telora-rfc0315-probe-target/release/telora-runtime-allocation-probe \
  --root /tmp/rfc0315-perf --samples 3 --output rfc/0315/measurements.json
```

测量目录中包含 `baseline-{empty,single,many,strings,ic}.wasm` 和对应 candidate 制品。
微用例采用 `rfc/0314/make-micro.py` 生成的同一源码：空/单元素/稠密 filter 和长内容。
`micro-requests.jsonl` 为八条 `{"method":"bench","input":42}`；ic 输入采用
`rfc/0313/check-ic-model.py` 的八个请求，保存为 `ic-requests.jsonl`。
基线源码为 RFC 合入提交 `92058f4c`，隔离 archive 的 parser 子模块取相同
`8bc3d9a17926d506a9d41f35947b72ff6125ce31`。正式三轮必须在其他构建/验收结束后串行运行；
探索性并发样本不纳入墙钟对照。

## 最终验收材料

- [性能报告](results.md)：阶段中位数及范围、fuel、镜像、高水位与已知回退。
- [验收记录](validation.md)：覆盖与命令，区分全量验收和后续增量回归。
- [主测量原始数据](measurements.json)、[周转原始数据](churn-measurements.json)、[溯源](provenance.json)。

周转使用 `churn.telora` 编译两版普通制品，运行同一测量脚本并追加
`--workloads churn --heap-bytes 524288 --allocation-interval 16384`。
主数据探针源码归档为 `runtime-probe-v1.rs`；当前探针增加可选策略参数，默认行为相同。
