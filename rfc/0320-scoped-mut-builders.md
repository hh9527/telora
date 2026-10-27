# RFC 0320：隐藏可变状态的惰性 pipeline

- 状态：已实现，首版以数组物化节点
- 跟踪：[telora #251](https://github.com/hh9527/telora/issues/251)
- 应用：[lab-ontology #66](https://github.com/hh9527/lab-ontology/issues/66)

## 动机与范围

构造过程的中间状态无需作为普通不可变值被观察。通过受限的绑定，标准库可以复用
缓冲区，而应用只能观察完成后的结果。首版只支持 DictMut／ArrayMut，不开放通用 mut、
用户自定义可变 consumer 或 SortedArrayMut。

## 表达与去糖

```telora
use std::pipeline as p;
let recipe = p::dict({seed: 1}.ty!(Dict(Int)));
let work = pipeline {
    sink d = recipe;
    sink a: Array(Int) = p::array([1]);
    d <- p::merge_update("second", 2);
    a <- 2;
    (*d, *a)
};
let result = work();
```

recipe 是普通不可变计划，可以选择、传递、捕获和重复使用。每次执行 sink 声明创建
新的私有状态，不共享计划的可变资源。pipeline 暂沿用零参闭包语义。

概念去糖：

```text
mut d = create_mut_builder(recipe);
d.extend(edit);
let result = d.finish(); // 消费，关闭 d
```

MutBuilder 不是普通可变变量。只能由 sink 声明创建、通过 <- 投递、通过 * 消费；
不能读取、复制、放入普通值、传参、捕获、返回或在 finish 后继续使用。
类型标注表示结果类型，即使丢弃 sink 也必须与计划匹配。

## 封闭实现与类型

计划携带 Item／Result／Implementation 三个类型参数，标准库的 dict／array 构造器
产生合法计划。实现状态只由编译器生成的私有入口触及；私有 MutBuilder 包装保存
类型化状态数组。DictMut 保持已有哈希索引和 update／thunk 操作值；ArrayMut 接收
普通元素，保留初值和投递顺序，不排序、不去重。

代码生成核验受信任的标准库类型身份，以及 Item、Result 与实现的一致性；伪造
phantom 类型参数不能使错误布局进入运行时。普通源码不能调用私有 builder 入口。

初值复制到独立可变存储，嵌套不可变值可以共享。finish 后不再存在可写句柄；
Array 可以转移缓冲区，Dict 暂时排序并复制为现有列布局。状态和中间值由现有精确
GC 管理；失败不发布完成结果，trap 沿用 Session 不可继续执行的规则。

## 生命周期和限制

沿用作用域、分支状态合并、禁止跨闭包捕获和循环内关闭外层 sink 的检查。
循环可以创建自己的 builder。未物化的 builder 允许丢弃。
首版仍不支持 pipeline 内 match／if-let／pattern binding 或循环中的提前 return。
尚不是通用仿射类型系统，也不开放 mut 表面语法。

排序去重应属于未来 SortedArrayMut 的完成策略，不能偷偷改变 ArrayMut 的结果。
通用 Extend、计划适配器、StringMut 与完整 IDE 支持留待后续。

## 计划与验收

1. 先将现有 Dict sink 改成 plan -> 私有 MutBuilder，保留控制流边界。
2. 加入 ArrayMut，验证计划复用、初值隔离、顺序、物化后普通 Array 操作及 GC。
3. 负例覆盖读／别名／捕获／逃逸／关闭后访问、错误结果标注和伪造计划；完整回归。
4. 本阶段不迁移 ontology，不部署，也不以旧性能数字作为新原型的测量结果。

## 写入句柄原型验证（历史）

原型支持省略结果标注的 Dict／Array sink；Array 初值隔离、普通计划的传递与复用、
分支与循环、物化后普通数组操作及强制每个安全点 GC 均通过。
错误结果标注（包括未物化的 sink）、错误元素及伪造 implementation／phantom 参数被拒绝。
Core 210 项、Wasm 167 项（另有 1 项手动性能实验）、语法语料 37 项通过（包含受限 builder 专用语料）。
本轮未重复执行旧 Dict 微基准，不将旧计时归因于新的 plan／MutBuilder 包装。
阶段实现和验收结果在 telora #251 汇报；分支保持不合入。

## 设计修订：以惰性节点替代写入句柄

公开接口采用 RxJS 风格的节点与操作符组合；`let` 只绑定普通不可变描述。
不再提供 `sink`、`<-` 或物化操作符。上面的作用域写入方案保留为实验记录。

```telora
use std::pipeline as p;
let a = p::from_array(input) |> p::filter(fn(n) { n > 0 });
let c = p::merge(a, p::from_array([1, 2])) |> p::distinct_by(fn(n) { `key\{n}` });
let f = c |> p::map(fn(n) { n * 2 }) |> p::collect();
let g = c |> p::collect_dict(fn(n) { `key\{n}` }, fn(n) { n });
p::run(p::both(f, g))
```

操作符与 collect 都只建立计划。`run` 定义一次执行边界，`both` 将不同结果组合到
同一次执行。同一节点的多个下游按节点身份共享计算，不做表达式公共子表达式消除。
分开调用 run 默认重新计算；显式 `cache()` 保留一次成功结果，可跨 run 复用。
失败不得发布缓存，递归求值必须报错。缓存只改变执行成本，不改变值语义。

首个原型以类型化数组物化每个被访问节点，验证 DAG 共享、执行边界与缓存语义。
这不是流式融合执行器，不宣称中间分配更少。后续可把单消费者链融合、分支按需缓冲，
而不改变公开接口。本轮以真实业务测量判断成本，性能回退不能用旧基准掩盖。

ArrayMut、DictMut、StringMut 均为标准库内部状态，应用只能得到不可变结果。
DictMut 支持哈希去重与覆盖，distinct_by 保留首个元素及其输入顺序，collect_dict
对重复键保留最后值。ArrayMut 保持顺序；StringMut 首版收集字符串块，完成时一次
join，不通过逐次不可变字符串拼接实现。暂不支持用户定义可变 builder。

缓存、运行标识及所有中间值由精确 GC 与快照重定位追踪。运行标识按内部身份比较，
不能以结构相等把不同 run 混同；用户侧无法创建、读取或修改缓存槽。

验收覆盖惰性、共享前缀、独立 run、显式 cache、失败重试、Dict／Array／String
结果及输入隔离、强制 GC、私有入口不可导入、旧写入语法被拒绝。ontology 迁移到
节点组合后，以生成模型和 IC 的发布内容等价、transform 结果及实际时间／内存验收。

## 惰性节点原型验证

公开 Node／Output 与 curried 操作符、run／both／cache 已实现；特殊语法、作用域
prepass 和卫生私有导入例外已移除。ArrayMut／DictMut 留在标准库内部，StringMut
以字符串块 ArrayMut 加一次 join 实现。节点以完整数组物化，尚未融合。

Core 209 项、Wasm 166 项、语法语料 37 项通过。测试实测共享前缀仅求值一次，
独立 run 重算；显式 cache 不受来源其他 run 影响。语言失败清除 Pending，递归
节点报错，重复重试仍可工作；Unicode、空集合、去重顺序、Dict 覆盖、强制 GC 和
构建期预热缓存的快照恢复均通过。应用不可导入可变后端、运行标识或缓存槽入口。
业务测量在 lab-ontology #66 记录，不把语义测试当作性能收益证明。
