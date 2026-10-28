# Map / MapMut 实现与模型验证

本文记录 ABI 41 阶段的历史实测。当前已删除 PersistentMap 与持久化 Dict 后端，使用 ABI 42；以下时间和大小没有因代码删除而重新测量。

## 当前实现

- 内置 `Map(K,V)` 使用排序的 key/value 两列；查询直接二分，普通不可变更新复制结果
- `Dict(V)` 透明展开为 `Map(String,V)`；String 特化共用已有字面量、字段访问、JSON object codec 和来源路径
- MapMut 仅作为编译器/RT 的内部构造指令存在，使用追加的 K/V 两列和排序的 u32 index，二分定位后插入 index，finish 一次整理列；pipeline::collect_map / collect_dict 共用此 builder
- 不实现 Eytzinger 或 aliasing 分析；批量构造不依赖 aliasing，普通更新保留复制语义

默认 key 全序支持 Int、String、Bytes、Bool、Type/TypeOf 及有限 tuple/struct/newtype。Float、函数、递归类型等没有隐式全序。非 String Map 使用显式 pairs 转换进行 JSON 编解码和 typed host input；host 输出支持有序 pairs。反射对 String 特化保留 Dict(value)，其他提供 Map(key,value)。

from_pairs 重复 key 保留首 key 代表、末 value；Map insert/merge 保留已有 key 代表。std::dict::from_pairs 保留重复 key 报错约定。共享 ABI 升为 41，编译器与 runner 需匹配。

## 验收

最终 ABI 41：Wasm 182 passed / 3 ignored / 1 filtered；core 210 passed；shared 35 passed。过滤项是 main 已复现的 guest_memory_growth_failure_is_distinct_from_managed_budget_exhaustion 配额分类失败。覆盖旧版本保持、类型别名身份、GC、newtype/产品 key、空及不可居住 key/value 类型、pipeline、512 次混合操作与独立 BTreeMap 对照、A/B 固化恢复。已有来源、codec、Service 和 transform 回归通过。

自定义 key 测试还发现并修复 newtype 构造器的 GC 根约定错误：普通构造器收到环境，canonical constructor literal 收到函数值，须按实际约定追踪。无 Map 的最小样例在隔离 main c03dc4fb 上复现原失败，修正后在强制 GC 及重复调用下通过。

fmt、RT rustfmt、diff-check、source-size、Python 脚本语法检查通过。source-size 只有原 generics.rs 1515 行 soft warning。未执行额外模型自检或知识图重建测试，未改生产配额。

## IC / NS 结果

本节对照表记录实现提交 54eb39df；下方另记按用户要求去掉标准库 MapMut 声明后的最终复验。

固定 Lab revision `f7dac73d4870e36a0e585d20c7c5fe0242a9540c`，git archive 提取独立工作区。对照是 commit 60efbe40 的编译器，关闭旧 PD 开关；candidate 是新的整体实现（包括 GC 修复），不是只更换一种数据结构的微基准。所有完整 P2 build information 相等。下面为最终 ABI 41 一轮：

| 模型 | 版本 | 前端 s | 代码生成 s | 构建期准备 s | 总构建 s | 制品 bytes |
|---|---|---:|---:|---:|---:|---:|
| IC | baseline | 9.13 | 2.72 | 11.28 | 24.04 | 13389300 |
| IC | columns | 8.97 | 2.76 | 11.31 | 23.92 | 13476099 |
| NS | baseline | 15.14 | 3.58 | 36.27 | 56.46 | 21076237 |
| NS | columns | 14.71 | 3.67 | 36.05 | 55.91 | 21162962 |

准备指 service_prepare；两侧 service_initialize 均约 0.8 ms。IC 准备基本持平，NS 单轮约快 0.6%；先行及反序验证也显示总体持平，不能认定有明显加速。当前收益是统一类型身份、开放自定义 key 和建立通用 builder，并未消除普通不可变更新的复制。

| 模型/版本 | fuel | 累计分配 bytes | 存活 bytes | committed bytes | 线性内存 bytes | GC |
|---|---:|---:|---:|---:|---:|---:|
| IC baseline | 25388748985 | 50926278 | 15521692 | 22759280 | 37224448 | 45 |
| IC columns | 25604366594 | 50938502 | 15627876 | 22865464 | 38141952 | 45 |
| NS baseline | 85427247710 | 143439313 | 33512881 | 49641480 | 88866816 | 56 |
| NS columns | 85869367013 | 143333033 | 33619065 | 49747664 | 88735744 | 56 |

累计分配基本持平，存活及 committed 各增加约 106 KB；fuel IC 约增加 0.85%、NS 约增加 0.52%。制品各增加约 87 KB。完整进程 RSS 含前端，最终轮 IC baseline/columns 为 718792/713704 KiB、NS 为 919056/919080 KiB，不替代托管堆指标。buildtime 保持 150B fuel、256 MiB memory。

## 复现

```sh
python3 scripts/measure-column-map-models.py \
  --workspace /path/to/lab-ontology \
  --baseline-telora /path/to/baseline-telora \
  --telora /path/to/candidate-telora
```

最终报告：`/opt/userhome/h00629578/.cache/tmp/column-map-models-on1omgtj/report.json`。candidate SHA256 `4b0dd27adf5f7feb7e02939801e6f185364c5dcaec7452726249f31a103bbccc`；baseline SHA256 `aca288f404935ef911e09fa6ce786c7afd6b44da8c893c192cbecac75e72d870`。先行及反序验证报告分别位于 `~/.cache/tmp/column-map-models-yvd994b_/report.json`、`~/.cache/tmp/column-map-models-t4xft9kr/report.json`；这两轮在 ABI 41 升版之前，三轮模型信息均相等，usage 一致。

所有构建使用 `~/.cache/tmp/telora-column-map-target`，没有替换 Lab 开发工具、runner、部署模型或服务配置。分支 impl/persistent-map，Issue #254，不合入、不部署。


## MapMut 指令边界的最终收敛

标准库中的私有 MapMut 名义类型已删除，`.telora` 中没有 MapMut 声明。MapMut 只表示编译器/RT 的内部构造机制；标准库仅以普通不可变 tuple `(Array(K),Array(V),Bytes,Bool)` 描述 GC 状态。受信任 native 指令验证封闭 key/value/布局合同后执行修改和 finish，没有可变语言类型或公开可变 API。

同一组回归继续通过：Wasm 182 / core 210 / shared 35；忽略和过滤项保持上述约定。最终模型复验：

| 模型 | 准备 s | 总构建 s | 制品 bytes | P2 信息与旧 Dict 相等 |
|---|---:|---:|---:|---|
| icloud_model | 11.50 | 24.13 | 13474227 | 是 |
| ns_model | 36.39 | 56.19 | 21161089 | 是 |

最终 compiler SHA256 `68d545d1c03b79f5876de0ac0f5f4eaa72800c30a2e7416b619939a785d7a526`；原始报告 `/opt/userhome/h00629578/.cache/tmp/column-map-models-on1omgtj/instruction-only-report.json`。准备性能仍基本持平，未启用 aliasing 优化。


## 删除持久化树后的复验（ABI 42）

仅保留列式 Map/Dict 与内部 MapMut，删除 PersistentMap 和持久化 Dict 实验后端，不保留 Rust feature。普通 Map 比较器由 native 模块 41 提供，RT 入口改为 telora_column_map；删除旧树 imports 和追踪布局。

同一隔离模型源码再次构建，完整 P2 信息与旧 Dict 基线完全相等：

| 模型 | 总构建计时 | 准备计算 | 制品大小 |
| --- | ---: | ---: | ---: |
| IC | 24.15s | 11.33s | 13,402,849 bytes |
| NS | 57.70s | 36.88s | 21,089,703 bytes |

相较上节 ABI 41 制品，IC 减少 71,378 bytes，NS 减少 71,386 bytes。单次测量不足以说明时间变化，未观察到明显构建加速。原始记录在 `~/.cache/tmp/remove-persistent-models-1zg8tuit/`。未替换生产二进制或部署模型。

CLI 全验收额外发现并修复此前桥接的类型显示遗漏：String key 显示为 Dict(V)，其他 key 显示为 Map(K,V)，不泄露内部参数顺序 [V,K]。
