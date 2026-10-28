# 跨函数集合分析：第二轮空间实验

跟踪 [telora #255](https://github.com/hh9527/telora/issues/255)，沿用 `explore/collection-aliasing`，不合入、不部署。承接 [第一轮空间矩阵](collection-builder-space.md)。本阶段探索跨函数信息能多覆盖什么，不实现通用自动 builder lowering。

## 新增的分析

`collection_summaries.rs` 对封闭、单态化函数及局部函数值追踪返回值的参数来源、逻辑构造、可能更新和共享视图，递归检查已解析的 helper；直接可知的函数参数可以在调用点代入，例如 `apply(twice,x)`。Map/Array 更新以及 String join 记录接收值来源和调用深度。

native array fold 使用已知标准库契约，符号执行一次 callback，并将初始状态与单步结果合并。它能暴露“入口 → build helper → fold callback → update helper”的累加器链，**不是任意次数循环的不动点或所有权证明**。控制流两支保守合并；递归、未知 native、模式、未支持的投影和捕获保持保守。每个根最多检查 50,000 节点、32 层活动调用。

函数索引和绑定索引只构造一次、只读共享；不为每个摘要重建全图。报告仍由 `TELORA_EXPERIMENTAL_ALIAS_REPORT` 开启，默认不开启分析。报告版本为 2，保留旧局部候选并增加 `function_summaries`、更新上下文和阻断原因。

这些字段不是独占证明：`fresh` 只是逻辑构造来源，空容器、字面量、单片段 join 均可能共享底层存储；`parameters` 不能替代精确的嵌套字段 points-to；`updated` 表示可能更新而非必然执行；Map keys/values 明确标为 borrowed_view。String join 保留输入依赖。闭包保留可调用目标，但捕获存储来源标为 unknown，不能把返回闭包误认为新建独占值。

## 一个真正自动选择的收益点

上一轮的 Array noescape 只是人工 oracle。本轮测试后端先检查所有同名 checked-native 调用点，再递归检查每个静态标量 callback 的 helper；只有全部通过受限检查才省略稳定元素快照，否则整种操作保留快照。当前只允许受支持的 Int/Float/Bool 签名和无未知/可观察操作的函数体。递归、dbg/诊断、未解析函数参数转发、捕获读取等都保守退回。

测试 fixture 中保守和自动选择版本使用完全相同的 helper：map 调用两层加一 helper，filter 和第二个 map 也调用 helper。两边都保持逐层执行顺序，未内联、未融合、仍按通常方式调用 callback。探针本身只支持 Array(Int)，不推广到任意复合值。

三个新 Session 的首次 invoke 中位数，不含编译、load、initialize、输入导入及 JSON 输出。256 MiB、20 亿 fuel、计时期间关闭自动 GC；之后显式 GC 验完整值。强 GC 正确性测试另跑。

| 项数 | 保守快照 ms | 摘要选择 ms | 累计分配 bytes：保守 → 选择 | fuel：保守 → 选择 |
| ---: | ---: | ---: | ---: | ---: |
| 128 | 5.137 | 4.293 | 8,056 → 2,584 | 6,767,047 → 5,769,365 |
| 512 | 18.234 | 14.968 | 30,584 → 8,728 | 26,533,469 → 22,553,363 |
| 2048 | 71.370 | 57.483 | 120,696 → 33,304 | 105,593,614 → 89,701,561 |

2048 项约减少 19.5% 时间、72.4% 累计分配、15.1% fuel。GC 后保留托管数据两边均为 33,224 bytes；线性内存两边分别为 1,245,184 / 917,504 bytes。这些都不是进程 RSS。helper 调用仍有明显成本，不应把此结果解读成 alias 分析已经消除了函数调用。

正常测试同时断言选择条件确实为 true；给出转发 callback 和 dbg callback 的反例则为 false。强 GC 后旧输入保持不变，debug 事件保留。该受限测试判断不是通用来源等价证明；正式启用前仍需验证来源、错误短路、任意复合元素、递归及 A/B 恢复等边界。

## IC 覆盖探针

同一 IC 普通 build，不做 staged 知识准备；分析开关两份 Wasm 完全一致，因此本阶段 **没有 IC 运行期加速数据**。报告中的函数是单态化实例，调用上下文有重复，不能当成独立业务函数或优化点数量。

- 4,986 个函数实例，其中非 std 的应用/ontology 实例 4,558 个。
- 329 条跨函数更新上下文，对应 25 个不同的源码更新节点，最深 10 层。
- 按上下文计：Array 310、String 11、Map 8。
- 115 条上下文的接收值浅层来源不含 unknown，均为 Array；这不代表其中 115 处可原地改写，更不能与旧局部候选数直接比较。
- 当前严格标量 callback 判定在 IC 的应用实例中命中 **0** 个。受限 Array 实验的收益不能直接套到 IC。

主要阻断来自未支持的表达式、记录/模式投影、未知 native 调用及捕获。诊断/失败语义也是真实边界，不应为扩大统计而忽略。记录中的原因是各根展开后的累计上下文，存在重复，不代表相同数量的源码问题。

这说明跨函数追踪能找到局部探针看不到的连续更新链，但只补“函数返回来源”还不足以在 IC 安全启用优化。Map/String 也还没有取得足够的来源证明；不能因微基准显示空间就宣称业务已自动受益。

## 下一轮最有价值的问题

1. 针对实际阻断点补字段/tuple/模式的路径敏感摘要，保留“容器本体”和“元素/视图”的区别，避免整个 record 一个 unknown。
2. 为封闭调用图传播 callback 目标及捕获环境；一个未知调用不能只凭签名乐观匹配。实例级 specialization 可避免一个未知调用拖累所有已知调用点。
3. 对 fold 累加器求保守不动点，加消费、借用、逃逸与历史版本寿命摘要，才可以决定连续 builder 的接管/复制/冻结位置。
4. 跨层融合继续独立处理效果与来源顺序，不和“不逃逸所以省快照”混为一谈。

当前最有证据的方向是摘要支撑连续区域，而不是继续增加单函数 use-count 特判。内部仍统一面向 MapMut/ArrayMut/StringMut，不增加用户可变对象。

## 验证与复现

Wasm 回归 186 passed、3 个性能测试 ignored；只过滤此前 main 已复现的 guest-memory-growth 配额测试。跨函数正反例、fold 累加器链、Map 视图、String 输入依赖、旧输入与强 GC 均通过。正式 CLI 隔离构建通过；源码规模检查仅已有 generics.rs 的 1515 行提示。

```sh
export CARGO_TARGET_DIR="$HOME/.cache/tmp/telora-column-map-target"
cargo test --release -p telora-wasm collection_summaries -- --nocapture
TELORA_COLLECTION_SPACE_CASES=interprocedural_array_chain \
TELORA_COLLECTION_SPACE_REPORT=/tmp/interprocedural-space.json \
  cargo test --release -p telora-wasm measure_lazy_collection_space -- --ignored --nocapture
TELORA_EXPERIMENTAL_ALIAS_REPORT=/tmp/ic-aliases.json \
  "$CARGO_TARGET_DIR/release/telora" -C /path/to/lab-ontology/icloud_model \
  build icloud_model -o /tmp/ic-analysis.wasm
```

本机原始数据：`~/.cache/tmp/collection-interprocedural-space.json`、`~/.cache/tmp/alias-cross-function-ic/{report,coverage,comparison}.json`。自动选择入口和 Array 重写均为 cfg(test) 探针，未成为正式标准库能力。

## 后续实验

字段路径、闭包环境、fold 有界不动点和 record 谓词实测见 [第三轮实验](collection-field-capture-space.md)。本页保留第二轮时的规则与数据；第三轮增加了真实标准库 filter 对照，并单独统计 IC 的 filter 调用点。
