# 字段、捕获与循环：第三轮集合分析实验

跟踪 [telora #255](https://github.com/hh9527/telora/issues/255)，沿用 `explore/collection-aliasing`。承接 [跨函数实验](collection-interprocedural-space.md)。本轮仍为分析和测试后端探索，不合入、不部署，不启用正式自动 builder lowering。

## 摘要增加了什么

- record/tuple 的成员分别保存抽象值。`pick({left:xs,right:ys})` 返回 left 时保留 xs 的来源，不混入 ys；对符号输入能表达 `parameter:0 → [*] → right` 这样的字段路径。
- 闭包除了函数 Key，还保存自己的不可变捕获环境。同一个 `make` 产生的两个闭包可以指向相同代码、不同捕获值；分支中同 Key 的闭包合并时同时合并环境，不能丢掉其中一边。
- 用已有 Plan.captures 追踪捕获值。独立分析 callback 时，捕获根是具名符号路径；在调用上下文中可代入实际抽象值。这只表示借用来源，不表示捕获存储独占。
- 增加 array map/filter/flat_map 和 length 的可信标准库契约，跟踪数组元素及 callback；Map keys/values 保留共享视图，但尚未精确表示列内元素，元素仍保守 unknown。
- fold 从符号单步升级到最多 8 轮的保守不动点。三元组循环换位测试确认后续迭代出现的别名都被保留。到限不收敛就返回 unknown，并阻止快照省略。报告的更新事件仍只保留首轮证据，不把探测迭代当作多次真实执行。

形状摘要有界：最多 8 层成员/路径、64 个成员/来源路径、256 个成员节点；超界保留 unknown。递归/调用深度、未知 native、模式、record spread 等仍保守处理。`fresh` 仍只是逻辑构造，字段来源也不是精确的物理 points-to，更不是可写证明。

报告版本为 3，增加路径、成员、fold 收敛统计及独立 callback_sites。它们仅在 `TELORA_EXPERIMENTAL_ALIAS_REPORT` 开启时计算；默认编译不运行此分析。

## 不逃逸判断扩大到标量结果

上一轮要求输入和输出都是 Int/Float/Bool，排除了大多数 record 谓词。本轮只要求返回值是这些标量；输入可以是 record、tuple、String 等，但函数体及所有被跟踪调用必须通过受限的无发布效果检查。未知调用、debug/诊断/失败、未支持语义均阻止选择。

隐式构造校验也必须计入效果：`let checked: Checked = {n:x}; checked.n>0` 表面只返回 Bool，但 @check 可能发布 blame/subjects。原型对存在对应构造校验的构造、调整、调用等保守阻断，不因最终只投影一个字段就跳过效果。

测试后端的通用 filter 能对任意已支持元素布局压紧数组；map 仍限定 Int。只有所有对应 native 调用点的静态 callback 都通过判断，才省略稳定元素快照。未知转发 callback 或有诊断的 callback 保留快照。输入仍由既有 ArrayMut 构造器复制一次，不会自动接管调用者的存储。

这不是任意 Telora 程序的纯性/来源等价证明。正式启用前仍须审计所有隐式调用、配置/固化、来源、错误控制流及生命周期边界。当前自动选择只影响测试专用 native，正式标准库没有这些入口。

## 实测：有捕获的 Int 链

2048 项，同一组 helper 和 callback，逐层顺序不变，不内联、不融合。每点三个新 Session，首次 invoke 中位，不含编译、load、initialize、输入导入和 JSON 输出。256 MiB、20 亿 fuel，计时内关闭自动 GC，之后显式 GC 验完整值。

| 负载 | 保守快照 ms | 自动选择 ms | 累计托管分配 bytes：保守 → 自动 |
| --- | ---: | ---: | ---: |
| 原多层 helper Int 链 | 70.573 | 57.284 | 120,696 → 33,304 |
| callback 捕获 config record，并读取 offset/divisor | 55.371 | 42.764 | 120,808 → 33,416 |

有捕获的链耗时约减少 22.8%、分配约减少 72.3%。这表明捕获本身不必成为断点；只读捕获和跨 helper 的字段访问也可以分析。两条负载的 helper 数量不同，不能拿它们的绝对时间互相比较。

## 实测：带 String/Array 子对象的 record filter

输入行包含 key、label、score、trail；输入和过滤结果同时返回。谓词读取字段、访问捕获配置，并调用一个 helper。标准库、保守原地探针、自动选择探针使用相同谓词。输入 JSON 在计时前导入。

2048 行、三次中位：

| 路径 | ms | 累计托管分配 bytes | fuel |
| --- | ---: | ---: | ---: |
| 当前 std::array::filter | 31.779 | 65,792 | 48,168,253 |
| 保守 ArrayMut，逐元素稳定快照 | 36.165 | 66,168 | 53,444,632 |
| 摘要选择 ArrayMut，省快照 | 30.925 | 33,400 | 47,491,062 |

与保守探针相比约减少 14.5% 时间、49.5% 分配；与真正的标准库相比耗时接近（本次约少 2.7%，三个样本不足以确认稳定差异），累计分配约减少 49.2%。不能把前一个比例说成对现有标准库的加速。

record 快照复制的是 16-byte 值头，不是深拷贝 String/Array 子对象。2048 个快照正好是 32,768 bytes。两边 GC 后保留的托管数据都是 344,488 bytes；原记录和子对象都不变。当前标准库 filter 本来就从稳定输入写到新输出，不承担探针的元素快照成本；它的增长分配与探针预先复制整段输入的路径不同。

这进一步确认：Array 上主要是减少分配。若要评估接管已有 buffer 的空间，还需要证明 receiver 的底层存储、视图和 callback 捕获之间没有旧版本观察，不能仅凭 predicate 不逃逸就直接接管。

## IC：来源覆盖与真正的 filter 调用点分开统计

普通 build，不运行 staged 知识准备；分析开关的 Wasm 逐字节一致。没有 IC 运行期或初始化加速数据。

| 指标 | 上一提交最终报告 | 本轮 |
| --- | ---: | ---: |
| 非 std 函数实例 | 4,558 | 4,558 |
| 返回值根来源已知（result.unknown=false） | 466 | 1,425 |
| 跨函数更新上下文 | 329 | 359 |
| 接收值来源不含 unknown 的上下文 | 115 | 142 |
| 去重后的更新源码节点 | 25 | 25 |

上述计数含泛型实例和调用上下文，不能当成新增可写点。Map/String 的接收值还没有获得足够的来源证明，142 条已知更新上下文仍都是 Array。应用根分析中有 18 次 fold 不收敛，已保守退回；未支持的模式、未知 native 和捕获仍是主要缺口。

规则扩展后有 285 个函数实例通过受限标量结果判断；这个数字本身没有性能权重。因此又独立扫描实际 callback 调用位置：

- 本探针识别到 69 个 array filter 源码调用点，其中 50 个的静态 callback 可以通过判断。
- 这 50 个输入均非 Int/Float/Bool 简单标量分类，包含其他类型；不能统称为 50 个 record 点。
- 另有 any/all/find 的已通过 callback，分别为 124/16/94 个调用实例；这些是只读消费者，不应混进原地 filter 的收益估计。
- 其余 19 个 filter 的目标都能解析，但摘要仍有未知捕获/native、模式或可观察操作，继续保守阻断。

调用点扫描是受支持静态写法的覆盖，不是所有动态/显式实例化写法的完整统计。50 个点也不代表初始化热点：还需要对齐实际初始化/transform 的执行次数、分配与所有权条件，才能估算业务收益。

## 下一步有价值的探索

下一步应将这 50 个 filter 候选与 receiver 的存储来源、视图、捕获和最后一次观察结合，区分复制一次与真正可接管的连续区域。同时补 Map 列元素和 String 构造链的精确摘要，而不是只继续放宽纯函数数量。

Array 结果已经显示 callback 调用/返回值成本依然明显。来源与合法性证明、消除存储分配、改变调用边界应分别衡量；不应期待 alias 分析单独消除所有成本。MapMut/ArrayMut/StringMut 仍是统一内部目标，不增加用户可变对象。

## 验证与复现

Wasm 195 passed、4 个性能测试 ignored；只过滤此前 main 已复现的 guest-memory-growth 测试。性能测试另行执行通过；非测试 CLI 构建通过；源码规模检查只有已有 generics.rs 提示。

新增验证包括 record/tuple 字段分离、两个同代码闭包的独立捕获、分支合并、闭包返回的集合别名、未选字段的诊断、隐式 @check、三状态 fold 后续别名，以及 record 输入/结果/嵌套子对象的强 GC。source/provenance 完整等价和 A/B 仍未验收。

```sh
export CARGO_TARGET_DIR="$HOME/.cache/tmp/telora-column-map-target"
cargo test --release -p telora-wasm collection_summaries -- --nocapture
cargo test --release -p telora-wasm record_predicate_elision_preserves -- --nocapture
TELORA_COLLECTION_SPACE_CASES=interprocedural_array_chain,captured_record_array_chain \
TELORA_COLLECTION_SPACE_REPORT=/tmp/field-capture-space.json \
  cargo test --release -p telora-wasm measure_lazy_collection_space -- --ignored --nocapture
TELORA_RECORD_PROBE_REPORT=/tmp/record-space.json \
  cargo test --release -p telora-wasm measure_record_predicate_elision -- --ignored --nocapture
```

本机数据：`~/.cache/tmp/collection-path-capture-space.json`、`~/.cache/tmp/collection-record-predicate-space.json`、`~/.cache/tmp/alias-record-predicate-ic/{report,coverage}.json`。同一输入输出与燃料预算，累计分配不是 RSS，也不是峰值活跃内存。
