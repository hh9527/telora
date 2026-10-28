# RFC 0321：不可变列式 Map

- 状态：列式 Map / Dict 桥接及 PersistentMap 删除已实现，未合入
- 跟踪：[telora #254](https://github.com/hh9527/telora/issues/254)

## 范围

仅保留内置 `Map(K,V)`，采用排序的 K/V 两列；`Dict(V)` 是 `Map(String,V)` 的透明别名。所有用户可观察值保持不可变。删除此前探索的 PersistentMap 标准库、BTree RT、布局与追踪特判、专属测试及开关，不保留可选 feature。

此前持久化树及 Dict 后端对照仅作为历史实验记录保留，不能代表当前可用 API：[持久化 Map 实测](../discuss/persistent-map-evaluation.md)、[Dict 后端实验](../discuss/dict-pd-model-initialization.md)。

## 表达与语义

```telora
use std::map as map;
let a: Map(Int,String) = map::empty();
let b = map::insert(a,3,"three");
let c = map::insert(b,3,"THREE");
# a 为空，b 保留旧值，c 含有新值
map::pairs(c)
```

默认全序支持 Int、String、Bytes、Bool、Type/TypeOf，以及由这些类型组成的有限 tuple、struct 和 newtype。产品按字段声明顺序比较，String 使用 UTF-8 字节序。Type 编号只在同一次封闭编译中稳定。Float、函数、递归 key 不提供默认排序；本轮不提供自定义 comparator API。

插入等价 key 覆盖值、保留首 key 代表；from_pairs 重复 key 保留首 key 与末 value，merge 使用右侧 value。Dict 的 from_pairs 继续拒绝重复 key。删除缺失 key 不改变内容。旧版本始终保持原内容。

String 特化支持现有字面量、字段访问与 JSON object codec；非 String key 通过 pairs 显式编解码，host 输出为有序 pairs。反射对 String 特化提供 Dict(value)，其余提供 Map(key,value)。

## 实现

普通不可变查询在有序 key 列二分查找；更新复制私有状态后发布新值。没有 BTree 路径共享。别名分析和 Eytzinger 不在本轮范围。

MapMut 只存在于内部指令和 RT，不声明语言类型、不暴露 sink 或更新操作符。key/value 缓冲分列追加，有序 u32 index 通过二分插入维护；finish 按 index 整理为紧凑不可变两列。pipeline collect_map 与 collect_dict 共用泛型 builder，不依赖 aliasing 分析。

pipeline 的 GC 状态由普通 tuple `(Array(K),Array(V),Bytes,Bool)` 描述。受信任 native 指令验证封闭 key/value/结果类型合同；普通 tuple 的语言语义保持不可变。Bytes 保存 packed u32 index，按几何级数扩容，GC 追踪缓冲所有权。

native 模块 41 通过 `telora_column_map` 调用列式 RT。删除旧树入口与持久化 Dict imports，ABI 升为 42；编译器与 runner 必须匹配，不兼容旧 ABI 制品。

## 验收

- 别名身份、有限产品 key、确定性排序、重复 key、旧版本保持
- pipeline 批量构造、codec、来源、GC 和 A/B 固化恢复
- 确认标准库库存不再包含 PersistentMap，源码不再保留 feature 或 BTree 后端
- IC/NS 构建使用隔离目录，不部署、不调整生产配置

此前列式版本的性能数据保留在[模型验证记录](../discuss/column-map-model-initialization.md)，不得当作本轮删除后的重新测量。
