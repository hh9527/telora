# 集合内部可变性的收益空间实验

跟踪：[telora #255](https://github.com/hh9527/telora/issues/255)。分支 `explore/collection-aliasing`；本阶段只增加测试探针，不实现正式自动 lowering，不合入、不部署。整体方向见 [统一所有权分析](collection-aliasing.md)。

## 结论与边界

统一 MapMut / ArrayMut / StringMut 有价值，但收益不均匀。主要空间来自消除 Map/String 的连续版本重建；共享输入可以先复制一次再持续写，不必因为无法接管原值就放弃整段优化。Array push 已有尾部复用，强行转成 builder 反而增加成本。

已经使用 builder 的惰性路径，进一步融合主要减少分配，耗时未必显著下降。callback 分派、返回值构造及元素逃逸快照是另外的成本层，不能全部用 alias 分析解释。缓存热路径基本没有源计算可优化；缓存后的写必须保留旧快照。

这是代表性空间调查，不是完整编译器能力已经验收。未覆盖所有操作、递归/提前终止、复合类型逃逸及来源等价，也没有测得 IC/NS 总构建加速比例。

## 测量方式

128/512/2048 项，Map 共享覆盖与无效删除最高 1024 项；每点三个新 Session，取首次 invoke 中位数。使用 release、256 MiB 内存、20 亿 fuel。编译、load、initialize、输入导入、结果 JSON 导出不计时；自动 GC 关闭以分离累计分配，计时后显式 GC 验证结果。强 GC 的正确性测试另跑。下表是回归测试结束后单独运行的矩阵，不与回归并行。

allocated 是累计托管分配，不是峰值活跃内存或进程 RSS；不能据此宣称 RSS 同比例下降。缓存数据来自单独的三次样本实验。

所有变体对比完整结果，不只比较摘要；共享输入包含旧 Map、keys/values 视图、原数组及原字符串。测试期原地 Array 操作仅支持 Int；保守路径给 callback 稳定元素快照。`owned_noescape_oracle` 的固定算术回调由实验人工保证无逃逸，代表未来摘要成立时的上界，不代表编译器已经能证明。融合及 fanout 使用固定纯回调，不能推广到任意用户函数。

## 完整矩阵（各负载最大规模）

| 负载 | 模式 | 项数 | 中位 ms | 累计分配 bytes |
| --- | --- | ---: | ---: | ---: |
| array_chain | eager | 2048 | 25.493 | 120,416 |
| array_chain | pipeline | 2048 | 26.517 | 122,544 |
| array_chain | owned_stage_order | 2048 | 31.896 | 120,696 |
| array_chain | owned_noescape_oracle | 2048 | 24.792 | 33,304 |
| array_chain | fused | 2048 | 24.791 | 66,232 |
| map_chain | immutable | 2048 | 314.752 | 30,107,000 |
| map_chain | pipeline | 2048 | 83.980 | 637,112 |
| map_chain | fused_builder | 2048 | 79.919 | 515,280 |
| string_chain | immutable | 2048 | 103.777 | 15,332,248 |
| string_chain | pipeline | 2048 | 58.333 | 396,864 |
| string_chain | fused_builder | 2048 | 56.342 | 208,960 |
| shared_fork | eager | 2048 | 39.478 | 175,336 |
| shared_fork | pipeline | 2048 | 41.218 | 178,712 |
| shared_fork | fused_shared_snapshot | 2048 | 38.385 | 165,312 |
| shared_fork | fused_fanout_pure | 2048 | 37.747 | 99,728 |
| mixed_three_collectors | pipeline | 2048 | 109.783 | 601,568 |
| mixed_three_collectors | fused_builder | 2048 | 113.600 | 533,248 |
| distinct64 | pipeline | 2048 | 155.961 | 724,504 |
| distinct64 | fused_builder | 2048 | 140.172 | 569,472 |
| map_shared_input | immutable | 1024 | 292.884 | 33,718,824 |
| map_shared_input | copy_once_builder | 1024 | 36.444 | 304,248 |
| array_shared_input | immutable | 2048 | 24.247 | 164,032 |
| array_shared_input | copy_once_builder | 2048 | 26.624 | 180,808 |
| string_shared_input | immutable | 2048 | 236.169 | 101,269,872 |
| string_shared_input | borrow_chunks_builder | 2048 | 39.000 | 574,352 |
| map_missing_remove | immutable | 1024 | 293.526 | 33,718,496 |
| map_missing_remove | readonly_no_copy | 1024 | 41.157 | 106,720 |
| callback_dispatch_count | no_callback | 2048 | 0.348 | 112 |
| callback_dispatch_count | one_identity_callback | 2048 | 4.426 | 296 |
| callback_dispatch_count | three_identity_callbacks | 2048 | 12.619 | 664 |

## 如何解释这些空间

- **连续写区域**：Map 共享输入复制一次后，1024 次覆盖约 293 → 36ms，分配 33.7MB → 0.30MB。关键不是必须拿到原输入所有权，而是循环中不再逐次冻结和复制。Map 初始构建也计入此测试两条路径。
- **不发生修改**：反复删除不存在的 key，用 contains_key 判断后直接保留原值，约 294 → 41ms，分配 33.7MB → 0.11MB。这个实验无需 alias 证明；可先避免无效操作的复制，再考虑 BorrowOrOwned 的首次写入。
- **字符串构造**：保留旧字符串、将其作为初始 chunk，最后一次 join，约 236 → 39ms；这不是对任意字符串 replace 的原地优化。
- **Array 连续重写**：保持逐层顺序的保守实现 31.90ms，慢于 pipeline 26.52ms。省掉人工已知不逃逸回调的快照后 24.79ms，累计分配 122,544 → 33,304 bytes。收益重点是分配，而不是承诺数倍加速。
- **已用 builder 的终端**：pipeline 到 fused，Map 83.98 → 79.92ms、String 58.33 → 56.34ms；相对逐次不可变构造的大收益主要已由终端 builder 获得。三个 collector 同时运行并没有更快，不能以少分配代替耗时结果。
- **共享分叉**：保留公共快照仍可减少上游中间数组；纯 fanout 可以进一步省快照，但需要额外语义证明。
- **调用开销**：2048 项只 count 为 0.35ms，增加一个 identity callback 为 4.43ms，三个为 12.62ms；累计分配不足 1KB。此对照说明即便消除缓冲分配，也不等于消除调用/fuel 成本。

## Cache 冷热与写后隔离

2048 项，三次中位；warm1/warm2 在同一个 Session 中重复运行，阶段间显式 GC。

| 工作负载 | 模式 | cold ms / 分配 bytes | warm1 ms / 分配 bytes |
| --- | --- | ---: | ---: |
| 读取 cache | pipeline | 25.107 / 121,628 | 0.063 / 332 |
| 读取 cache | fused source | 24.342 / 66,892 | 0.065 / 332 |
| cache 后写 | pipeline | 45.285 / 198,764 | 19.280 / 77,468 |
| cache 后写 | fused source | 44.552 / 144,028 | 19.423 / 77,468 |

优化只影响冷源计算，cache 热读已经非常便宜；下游写不能接管缓存存储。每次结果同时包含缓存原值及变换结果，重复运行和 GC 后原值均不变。

## 合法性与下一步决策依据

已有测试验证完整值、历史输入、Map 视图、空/单元素/filter_map、强 GC、缓存重复写，以及诊断回调顺序。pipeline 与保序 ArrayMut 都是 A,A,B,B；seq 融合为 A,B,A,B。因此跨层融合需要对诊断、失败、来源及效果顺序做额外判断，不能只凭 alias/最后一次使用推导。

正常 invoke 路径仍参与来源生成，但本轮没有完整验证优化前后来源路径等价；noescape oracle 更不能作为来源正确性证明。通用复合元素、回调将元素留在闭包/缓存中、错误短路、递归及 A/B 恢复仍是正式增强前的验证项。

后续若决定实现，分析应分三层：

1. 无效操作与已有尾部复用：不依赖全局 alias 就能减少复制，不强行接管已有高效路径。
2. 函数消费/借用/返回别名及 noescape 摘要，跨循环维护连续 builder；共享输入按需复制一次，只有发布时 freeze。
3. 另行证明跨层效果顺序，才选择融合；共享和 cache 保留必要不可变快照。

这些仍可统一降到内部 MapMut/ArrayMut/StringMut，无需向语言暴露可变对象、sink 或赋值操作。当前先保留实验，尚未承诺完整自动优化的实现范围。

## 复现与验证

```sh
export CARGO_TARGET_DIR="$HOME/.cache/tmp/telora-column-map-target"
TELORA_COLLECTION_SPACE_REPORT=/tmp/collection-space.json \
  cargo test --release -p telora-wasm measure_lazy_collection_space -- --ignored --nocapture
TELORA_COLLECTION_CACHE_REPORT=/tmp/collection-cache.json \
  cargo test --release -p telora-wasm measure_collection_cache_space -- --ignored --nocapture
cargo test --release -p telora-wasm \
  -- --skip guest_memory_growth_failure_is_distinct_from_managed_budget_exhaustion
cargo check --release -p telora-wasm --lib
```

回归 180 passed、3 个性能测试 ignored；只过滤此前在 main 复现的 guest memory growth 配额测试。完整矩阵和 cache 性能测试另行执行通过。非测试 library check 通过，源码规模检查仅有既存 generics.rs 的 1515 行提示。

原始本机数据：`~/.cache/tmp/lazy-collection-full-space-report.json`、`~/.cache/tmp/collection-cache-space-report.json`。测试 fixture 仅由 harness 追加到可信标准库，原地探针在 `cfg(test)` 下，正式库未暴露这些入口。

## 跨函数后续实验

自动摘要选择元素快照、fold 累加器追踪及 IC 覆盖边界见 [第二轮跨函数实验](collection-interprocedural-space.md)。
