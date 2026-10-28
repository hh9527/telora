# Map / Array / String 的统一所有权与 builder 探索

- 分支：`explore/collection-aliasing`
- 跟踪：[telora #255](https://github.com/hh9527/telora/issues/255)
- 起点：main `9218cc12`
- 状态：分析与收益探针，未启用自动原地更新，未合入或部署

## 目标

在语言的不可变值语义下，编译器将一段连续计算降到内部 MapMut、ArrayMut、StringMut。alias 分析负责判断共享与所有权，liveness 判断旧版本的可观察寿命；容器操作统一交给对应 builder。优化单位是一段计算区域，覆盖函数调用、分支及循环中的所有权传递。

现有局部单次使用分析只是探针，报告 `local_transfer_candidate`，不是内存独占证明。参数、闭包捕获、共享视图、未知函数返回值和多次使用均保守报告，不能据此开启破坏性更新。

## 内部表达

三种后端共用以下编译器内部协议，不增加语言类型或可变操作符：

```text
into_owned(value, ownership_evidence) -> internal_builder
borrow(builder, query)               -> read_result
edit(builder, operation)             -> internal_builder
freeze(builder)                      -> immutable_value
```

`into_owned` 接收共享输入时复制一次，接收已证明独占且可写的输入时接管存储。冻结的输入、构建期固化数据、内联短值等由后端选择新缓冲，所有权证据不等于任意地址均可改写。

```text
源程序：
    result = fold(items, initial, fn(acc, item) { update(acc, item) })

目标 lowering（仅当 callback 的消费与逃逸分析成立）：
    builder = into_owned(initial)
    for item in items:
        builder = edit(builder, item)
    result = freeze(builder)
```

Map 使用分列 K/V 和有序整数 index；Array 使用连续元素缓冲；String 使用字节或 chunk 缓冲，由操作和最终物化需求选择。统一的是所有权和操作协议，各后端保留自己的布局及增长策略。逐次读写可以借用 builder；不会因为一次 length/get 就强制物化整个容器。

## 分析必须覆盖的边界

1. **值身份与存储身份**：别名复制、Map 的 keys/values、String/Array 视图和嵌套容器形成 points-to 关系，单个 value head 最后一次使用不能证明 buffer 独占
2. **只读借用与逃逸**：读取元素、length、get 可以借用；返回视图、保存到容器、闭包捕获、未知函数或诊断发布可能延长旧版本寿命，须物化或保留共享路径
3. **分支与循环**：互斥分支的使用不能简单相加；phi/join 只有在相容的所有权状态下才能合并。循环和尾递归携带 builder，退出时才 freeze
4. **函数摘要**：参数区分借用与消费，返回值区分新存储和参数别名；标准库 wrapper 不应成为分析断点。无法证明的调用保持不可变路径
5. **GC、来源与固化**：内部缓冲按已初始化前缀精确追踪；操作来源仍按原求值点产生。外部可观察事件必须保留原值，跨 A/B 发布边界输出不可变值，不能依赖未追踪的内部指针

原地 map/filter 还要证明 callback 不观察被改写的输入；变长 replace 需要容量与布局检查。接入统一后端不会自动让所有操作都适合原地修改。

## 收益探针

运行：

```sh
CARGO_TARGET_DIR="$HOME/.cache/tmp/telora-column-map-target" \
TELORA_ALIAS_BENCH_REPORT="$HOME/.cache/tmp/collection-alias-baseline-report.json" \
cargo test --release -p telora-wasm measure_collection_aliasing_potential \
  -- --ignored --nocapture
```

128/512/2048 项，每项 3 个新 Session；不计编译、load、initialize、输入导入和输出 JSON 时间，计首次 invoke。两个路径使用相同输入并核对结果摘要（数量、总和或字符串长度/前缀）。配额为 256 MiB / 20 亿 fuel；关闭自动 GC 以观察分配压力，计时结束后显式 GC 校验结果可用。此数据不是稳态重复调用、RSS 或 IC/NS 总构建速度。

Map 的 builder 对照使用既有 pipeline collect_map；String 使用 collect_string；Array 在测试专用的可信 std 模块里组合既有 ArrayMut create/extend/finish，生产标准库未增加 builder 接口。所有测试程序均使用不可变语言表达。

2048 项的中位时间与单次托管分配：

| 工作负载 | 不可变路径 | builder 路径 | 时间比（不可变 / builder） | 托管分配：不可变 → builder |
| --- | ---: | ---: | ---: | ---: |
| Map 插入 2048 个独立 key | 532.55 ms | 90.54 ms | 5.88× | 67,305,752 → 689,976 bytes |
| Map 反复覆盖 64 个 key | 109.03 ms | 73.97 ms | 1.47× | 4,293,912 → 308,888 bytes |
| Array 逐项 push | 30.74 ms | 33.40 ms | 0.92× | 163,992 → 147,952 bytes |
| String 反复 join 8 字节 chunk | 115.47 ms | 35.67 ms | 3.24× | 33,980,552 → 362,384 bytes |

Array push 已有安全尾部容量复用，独占 builder 没有在该负载中进一步加速。String 普通 join 每次重新构造，StringMut 将片段收集后一次 join；内部 codec 的 content_append 已支持尾部复用，但普通 join 未走这条路径。Map 普通更新反复复制整列，批量 builder 显著减少中间版本。

这些是等价高层计算选择不同内部构造路径的测量，不能宣称编译器 alias 分析已经获得上述收益。pipeline 自身含有闭包、memo 和 builder 调用成本；真正的 lowering 仍需实现并重新测量，不能承诺所有应用都达到这些比例。

## 当前原型与后续实现顺序

`TELORA_EXPERIMENTAL_ALIAS_REPORT=/path/report.json` 让编译器输出候选及源码位置；默认不计算候选、不写文件。报告明确为 analysis_only，统一 region lowering 尚未实现。它用于校验局部探针遗漏了哪些消费链，不承担优化合法性判定。

下一阶段建立与三种容器共用的所有权及 operation IR，先接入有明确消费语义的 fold/尾递归链，避免在每次 edit 后 freeze。随后补函数摘要、分支 join 与存储别名关系，再分别扩展 Map 更新、Array 重写和 String 变换。每种后端都需与未优化版本进行完整结果、旧版本、GC、来源和 A/B 对照；默认是否启用由实测决定。

## IC 覆盖度探针

同一 IC 源码以普通 build 编译两次，不执行知识准备，分别关闭和开启候选报告。两份 Wasm 字节完全一致；耗时 13.09s / 12.83s，单次测量不解释为性能差异。

共报告 91 处实例，其中应用模块 86 处（ArrayMut 84、MapMut 1、StringMut 1）。63 处存储来源未知或共享，18 处来自参数/捕获/全局，5 处有多次引用或逃逸；没有一处通过局部 transfer 候选判断。此清单仅覆盖当前识别的标准操作，不代表应用里只有这些容器计算。

结果说明局部探针不足以支持业务模型自动优化：标准库和应用 wrapper、callback 累加器及组合函数都需要跨函数消费摘要，循环内部还需要维持未冻结的 builder。后续统一 region lowering 不能以这个局部单次使用探针作为最终证明。

原始报告：`~/.cache/tmp/alias-report-smoke-omsdyli7/`。

## 首轮硬性验收：标准惰性组合器完整覆盖

标准惰性写法是统一 builder lowering 的首要覆盖目标。受支持组合器由编译器和标准库提供消费、借用、共享与物化摘要，不能依赖应用偶然满足局部单次引用启发式。当前 seq 有融合执行路径；pipeline 的 Node.evaluate 返回并 memo 一个 Array，map/filter 各节点仍会物化。终端 collector 已使用 builder，不代表整条 pipeline 已满足目标。

验收覆盖以下场景：

- 单消费 map/filter/filter_map 链与 collect_map、collect、collect_string 三种终端，内部状态稳定使用 MapMut/ArrayMut/StringMut，连续操作之间不反复 freeze/rebuild
- distinct_by 的去重状态和结果缓冲均由内部 builder 管理，不反复构造不可变 Dict/Array 中间版本
- 同一上游分叉到多个下游，按既有运行语义共享求值与必要快照；下游写入不修改其他消费者的结果
- 显式 cache、重复 run、提前终止、异常和递归节点保持既有次数、顺序、错误及来源语义；缓存发布不可变快照，不保存可继续写的共享 builder
- 受支持的线性 fold 累加器和循环传递内部所有权，循环体不因标准库 wrapper 而退回逐次复制；任意用户 callback 仍需证明其消费与逃逸行为，不能仅因外层惰性就假设独占
- 在优化开关两侧核对完整结果、历史版本、GC、来源和 A/B 恢复，并记录 builder 复制、增长和 freeze 次数，防止只看耗时误判是否命中

完整覆盖表示在语义允许的范围内，所有标准惰性组合器均获得相应的内部可变实现；共享缓存边界需要的快照和不可避免的数据物化仍然存在。融合执行必须维持现有可观察求值顺序，不能因为少分配数组而擅自改变 callback/错误/调试事件顺序。

## 扩展实验

共享输入、无效修改、保序重写、融合、分叉与 cache 冷热路径的实测及边界见 [集合 builder 收益空间](collection-builder-space.md)。这些仍是测试专用探针，不是正式自动 lowering。

## 从手写参考反推自动优化（本轮）

先兑现手写参考中能证明的部分，不把 TypeId 专用算法自动归功于所有权分析。

### 固定键临时数组的跨函数消除

`TELORA_EXPERIMENTAL_LITERAL_PAIRS=1` 开启原型，默认关闭；无需
`experimental-mut-oracle` feature，也不增加任何语言接口。

编译器识别 `dict::from_pairs([("key", expression), ...])`，以及任意模块中的
闭合、无捕获、单参数、非泛型包装函数，其完整函数体仅为
`EnumVariant(dict::from_pairs(parameter))`。包装函数按结构识别，不匹配 ontology 的函数名。

只有参数单次消费、键均为互不重复的字符串字面量、无额外绑定或未知调用、
没有需保留的中间类型调整时，才直接生成最终 K/V 列。值表达式按原顺序计算一次，
结果保持普通 GC 根。键排列在编译期确定，不需要临时 pair/array 及运行期排序。
重复键、动态键、共享参数数组、捕获或其他包装形式保留原实现。
未对任何用户共享存储作原地改写。

构造器仍进行原有检查；字典来源、键/值来源、构造器物化来源保留。
原型保留 callee 的求值与未初始化检查。错误和显式 GC 回归另有测试。
可用 `TELORA_EXPERIMENTAL_LITERAL_PAIRS_REPORT=1` 输出 collection-lowering JSON 行，
记录源模块、命中位置、键数量和是否跨函数，不读取模型业务名称。

### 可信私有 builder 的稳定头复用

`TELORA_EXPERIMENTAL_PRIVATE_BUILDERS=1` 让既有 pipeline 的 ArrayMut/MapMut
复用内部数组头与 backing-store 描述。初始输入已复制；内部状态不交给用户 callback；
finish 在发布不可变视图之前 seal，后续写入检查仍存在。因此正常 Node/Output 用法不暴露写对象。
String collector 的 chunk ArrayMut 同样走这条内部实现。

这一项依靠已有私有 builder 合约，不是通用 alias 推断。未知用户状态不会因局部单次引用而
获得破坏性更新权限，也没有新增 sink、写入操作符或公开 XxxMut。

### 应用对照

lab-ontology 的 `scripts/measure-mut-ceiling.py --variants baseline automatic sealed`
用同一编译器比较：原始源码、自动优化原始源码、手写封闭参考。
`automatic` 不改 ontology 源，既有 TypeMemorizer 保持原样。
自动版不使用 oracle 函数；开启实验 feature 只是为了同时构建手写参考。
完整发布 JSON（含数组顺序）必须相同。

剩余 TypeId 去重/闭包遍历是专用算法选择，不能从任意 `Fn(Type)->String` 推断键单射，
也不能省略该回调的失败或其他可观察行为。当前不作这种不安全的替换。

最新结果与原始报告存放在 lab-ontology 的
`benchmarks/collection-mut-ceiling/automatic-lowering.md` 及 `automatic-results.json`。
