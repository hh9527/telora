# Scoped write-only sink 实验

以下是最初 Dict-only 原型及当时的测量记录。当前分支已改为不可变 plan 创建受限
DictMut／ArrayMut，详见 [RFC 0320](../rfc/0320-scoped-mut-builders.md) 与当前设计文档。
旧的直接集合初始化语法和私有入口已替换；下表性能不是新 plan 路径的复测。

状态：`experiment/scoped-write-only-sinks` 工作分支上的原型，未合入；跟踪 [telora #251](https://github.com/hh9527/telora/issues/251)。
语法子模块 `tree-sitter-telora` 使用同名分支。

目标是验证：显式、局部、只写的 sink 可以去糖为内部可变 builder，语言外部仍只能观察不可变值。
本实验未修改 ontology / IC，也未更新正在使用的 telora CLI 二进制。

## 表达与去糖

```telora
use std::pipeline::{merge_update, merge_thunk};

let plan = pipeline {
    sink types: Dict(Int) = original;
    for n in input {
        types <- merge_thunk(`key\{n}`, fn() { n * 10 });
    };
    types <- merge_update("count", 42);
    let types = *types;
    types
};
let result = plan();
```

`pipeline { ... }` 在原型中是普通的零参闭包，创建时不执行；每次调用重新创建私有 builder。
内部顺序与源码一致，没有自动 DAG 调度、重排或公共子表达式消除。

概念去糖为：

```text
let plan = fn() {
    private builder = dict_new(original)
    array::fold(input, unit, fn(_, n) {
        dict_accept(builder, merge_thunk(key(n), fn() { n * 10 }))
    })
    dict_accept(builder, merge_update("count", 42))
    let types = dict_finish(builder)
    types
}
```

`merge_update` / `merge_thunk` 返回普通的 `Edit(V)` 枚举值，不是方法调用或可变句柄。
`Update` 覆盖已有值；`Thunk` 只在 key 缺失时计算值。
重复 key 仍然构造操作值和闭包，但不执行 thunk 内容。

本实验只实现 Dict consumer，尚未抽象出 `Extend(T)` 协议。
`*sink` 是一次性的关闭和物化，不是任意对象的解引用运算。

## 不可变边界

- sink 名称不是普通值：不能读取、别名、放进容器、返回或被源码闭包捕获。
- 只允许在所属 pipeline 中投递；独立的嵌套 pipeline 可以拥有自己的 sink。
- 物化后不能再投递或再次物化。分支结束时合并状态；非所有分支都保持开放时，拒绝后续使用。
- 循环可以向外层 sink 投递，不能物化外层 sink；循环内部可以创建和物化本次迭代自己的 sink。
- 外部初始 Dict 复制进入 builder；共享的嵌套不可变值不会被修改。
- builder 入口不从 std 导出，编译器生成的卫生名称才能使用；运行时还检查 sealed 状态。
- 未物化的 builder 可以丢弃，由正常 GC 回收。失败时不发布结果，沿用 Session 发生 trap 后不可继续执行的规则。

当前控制流检查是保守原型，不是通用的线性/仿射类型系统：不支持 pipeline 内的 pattern binding、match、if-let、局部 def，或循环中的提前 return / propagation；for 暂时只支持 Array。
普通分支、局部 let、普通值闭包、多个 sink、相同名称的普通值遮蔽均已验证。

## 实现

- CST 分析跟踪 sink 的作用域、捕获边界、循环边界和 Open/Closed/Conditional 状态。
- HIR 去糖使用私有 std/pipeline native；源码无法获得 builder 的 Array(State(V)) 表示。
- entries 保存插入顺序，容量按需增长；哈希索引保存整数位置，碰撞用完整 String 比较。
- 可含引用的所有值仍在普通的带类型 Array / tuple 中，由现有 GC 跟踪；无裸值指针索引。
- finish 使用现有 Dict from_pairs 排序并复制到 Dict 的列式布局，**不是零复制物化**。
- 增加一个内部字符串 hash ABI helper；原型改变了编译器/RT 导入函数序号，必须成套生成。

主要实现文件：

- `crates/telora-core/src/hir_lower/pipeline.rs`
- `crates/telora-core/modules/std/pipeline.telora`
- `crates/telora-wasm/src/pipeline.rs`
- `crates/telora-wasm/src/tests/pipeline.rs`

## 实验与复现

```sh
cargo test --release -p telora-core --lib
cargo test --release -p telora-wasm --lib
cargo test --release -p telora-wasm --lib scoped_pipeline_sinks_measure -- --ignored --nocapture
(cd tree-sitter-telora && npx -y tree-sitter-cli@0.25.10 test)
```

性能样本为 `tests/fixtures/scoped-sinks-bench.telora`，比较同一批不同 key 的 Int 值：

1. persistent：每项建立单元素 Dict，再 dict::merge 到累计结果。
2. buffered：每项构造 Update，提交到私有 builder，最后物化。
3. batch：array::map 生成全部 pairs，再一次 from_pairs。

每组使用新 Session，初始化并导入输入后，预热同规模调用、显式 GC，然后只计量第二次执行。
计时包含默认自动 GC，不含编译、加载、初始化、输入导入和输出 JSON 导出。
数据是单机小样本，不代表 IC 全流程收益或 RSS；线性内存是引擎已增长的内存容量。

本轮结果（release，1000 个不同 key，预热后的单次执行）：

| 路径 | 执行时间 | 累计分配 | GC 次数 | 保留线性内存 |
| --- | ---: | ---: | ---: | ---: |
| persistent | 487.76 ms | 20,518,002 B | 270 | 917,504 B |
| buffered | 94.99 ms | 620,674 B | 9 | 1,114,112 B |
| batch | 54.37 ms | 330,066 B | 5 | 983,040 B |

同一个 RT 下制品尺寸分别为 1,491,437 / 1,501,268 / 1,487,475 B。
相比逐项 merge，buffered 在本样本约快 5.1 倍，累计分配少约 97%，但保留线性内存多约 192 KiB。
所以不能把累计分配降低解释为所有内存指标都降低：builder 的 entries、index、增长预留以及最终转换都有成本。
直接 batch 仍然优于 buffered；当业务可一次给出全部唯一 pairs 时，应优先用现有 from_pairs。
需要分支、多路投递、按 key 去重且避免重复计算时，sink 才提供额外的表达与执行价值。

验证结果：core 208 项、Wasm 164 项（另 1 项手动性能实验）、tree-sitter 36 项均通过；手动性能实验通过。
覆盖重复 thunk 不执行、初始 Dict 不变、嵌套聚合值、重复执行 plan、独立嵌套 pipeline、分支物化、循环内局部 sink、强制每个安全点 GC，以及 12 类非法用法诊断。
格式、diff 空白检查和源文件规模检查通过（仓库已有 generics.rs 1515 行软提示）。

尚未完成通用 consumer trait、完整控制流和所有 owned AST / IDE 工具链适配；本说明不是语言正式规范。
下一步建议先确认表达与去糖，再以 RFC 固化规则，随后把真实知识类型收集迁移成一个业务实验，测量 IC 总构建时间与峰值内存。
