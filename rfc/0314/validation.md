# 验收口径与复现

实现依据 RFC 0314；候选代码为 `9087e6e7`，基线为 `dae6de25`（RFC 0313 +
codec 准确根修复）。应用源码固定为 lab-ontology `5a870224d56208fad97bdc37e72d5f82a0ca6276`，
复制至 `/tmp/rfc0314-ic/source`。所有构建、制品、数据和插桩均隔离于应用使用的工具。

## 正确性

`CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/telora-rfc0312-target CARGO_BUILD_JOBS=4 cargo test --workspace`
完成 CLI 95、core 207、data 46 及其他先行 crates；CLI 包含全部语言验收的 441 项观察。
Wasm 原轮 139 passed / 1 failed，唯一失败是搬迁用例期望未根化的原始 Host marker
跨 GC 保留。`9087e6e7` 为该专测物理搬迁的用例显式关闭自动 GC，定向重跑通过。
这是测试约束修订，不放宽 runtime 的准确根规则。其余 139 项包含强制 GC、
Single 子引用、临时聚合、尾调用、冻结图、快照与请求恢复、资源终止等。

补跑 `cargo test -p telora-wasm-shared`（33/33）和 `cargo test --workspace --doc`
（含 tree-sitter 文档用例）通过。全量覆盖与修订后的定向结果共同构成验收证据；
原始 workspace 命令有一次测试失败，不将它写作单次零失败运行。
最后补充大 Raw 的中等切片保留/释放，以及稀疏 Single 脱离原输入的占用断言；
只重跑相关 content 5/5、small_arrays 4/4，运行时代码与性能测量制品一致。

应用制品按 `rfc/0313/check-ic-model.py` 中相同的 8 个请求测试。
ABI 36 / 37 分别使用对应的旧 / 新 `telora-run`，比较每项 response，全部一致。
`field_roles` 27/27、`ontology/knowledge_address` 4/4 通过。
field_roles 整组共享执行配额，使用 `--request-fuel 100000 --report-usage`；
默认配额及 1000M 均不足，不能将验收配额误认为产品默认值。
具体 fuel 见 [application-validation.json](application-validation.json)。

## 构建与测量

`initialize-probe` 固定 wasmi 2.0.0，开启 fuel，批量复制每 64 字节计一个 fuel，
翻译/校验不计执行 fuel。每个样本创建新进程和实例，先配置 Guest 64 MiB、
managed heap 32 MiB、GC interval 64 KiB，再单独计时 `create-service`。
它直接实例化同配置的普通制品，不包含编译和 Host 实例压缩。
同一探针兼容两版 ABI，不在 Host 执行语言回调或 GC。

```sh
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target CARGO_BUILD_JOBS=4 \
  cargo build --release --manifest-path rfc/0314/initialize-probe/Cargo.toml

python3 rfc/0314/make-micro.py
# 分别用保存的基线、候选 compiler 构建：
# compiler -C /tmp/rfc0314-micro build @src/empty -o version-empty.wasm
# 同样构建 single、many、strings。
# compiler -C /tmp/rfc0314-ic/source/icloud_model build icloud_model -o version.wasm

python3 rfc/0314/measure.py \
  --probe /tmp/telora-rfc0312-target/release/telora-raw-initialize-probe \
  --output /tmp/rfc0314-measurements.json \
  ic-baseline=/tmp/rfc0314-ic/baseline.wasm \
  ic-candidate=/tmp/rfc0314-ic/candidate.wasm
```

实际批次同时加入四组 micro 的 baseline/candidate，共 30 个样本；在构建与测试
结束后单进程串行测量，各组 3 个独立样本。运行环境中其他常驻进程未停机，
因此墙钟只作为本机对照；fuel、分配和 collections 同组各轮应一致。
微基准分别为 4096 次过滤长度 64 的输入（0/1/64 命中）和解析 4096 个不同的长字符串。
每次 predicate 次数和输入相同，不将 dense filter 微基准当作 ic-model 的分布。

## 统计含义

- `memory` 是单实例最终 Guest 线性内存；Wasm 实例内存只增不减，因此也是这段
  初始化的线性内存峰值。它包含静态镜像、Rust allocator 和已保留的空闲页。
- `telora_heap_bytes` 是结束时存活 payload；不是 managed 峰值或 allocator 总量。
  基线为 words payload + 独立内容长度，候选为受管 payload（含 Raw 头/容量）。
  managed 总量始终由 32 MiB 配额约束；未增加逐分配的 managed 峰值采样。
- `telora_heap_committed` 为 word arena 的 committed + metadata；基线不含独立
  content Vec 容量，候选包含 Raw，因此不能将两版差值直接解释为全部内存增减。
- `allocated` 是累计 word heap 分配请求字节，不含 allocator 元数据。基线的独立
  content Vec 不进入该计数，候选 Raw 的头和容量进入；包含字符串时不是同口径总分配。
- `collections` 是回收次数，不等同 GC 总 CPU；较少的分配不保证次数或墙钟同比下降。

## 高命中过滤的成本调查

全命中时，已知长度的原实现一次分配 64 个完整元素；按需构建器依次分配
1、2、4、8、16、32、64 个槽，总计 127 个槽，并复制已初始化前缀。
Int 完整宽度 16 字节，4096 次过滤的缓冲累计由 `4096*64*16` 增为
`4096*127*16`，差值为 4,128,768 字节，与探针累计分配增量相符（其余小差异来自表示）。
这解释了 collections 从 69 增到 132 以及 CPU 回退；并非谓词多跑一遍或失去停机控制。

首版保持按实际命中数有界增长，优先保证 0/1 命中不预分配输入规模的缓冲。
Dense 场景的额外复制与 GC 是明确成本，不宣称 Array.filter 普遍更快。
后续可单独比较增长因子、容量提示和已知高密度的适应策略；需要明确额外保留容量上限，
不能通过第二次命中就无条件预分配全部输入来掩盖稀疏成本。
仅需要 count/fold 的业务可使用现有 Seq 标量消费者，省掉结果数组。

稳定 Raw 同样不是逐个长字符串的免费优化：每个长字符串现在有受管分配、8 字节头
及 allocator 元数据；它换取稳定引用与独立回收，消除普通 GC 的内容搬移和切片修补。
字符串密集样本的 CPU/存活尺寸变化与这一设计成本一并报告。

## 频率插桩复现

使用含 tree-sitter 子模块的独立源码副本，在副本内执行
`git apply --unidiff-zero rfc/0314/publication-profile.patch`，隔离编译另一份 compiler，再对同一份
应用源码构建 ordinary 制品。用上述 `initialize-probe` 运行，结束时可选地读取三个
`rfc0314_array_*` global。最终编译器不应用此补丁；性能表使用未插桩制品。
结果与覆盖范围见 [publication-profile.json](publication-profile.json)。

最终追加验证还包含 `cargo test -p tree-sitter-telora`：unit 3/3、doc 1/1。
fmt、diff 与源码规模检查通过；规模脚本仍报告既有 generics.rs 的 1515 行软提醒。
