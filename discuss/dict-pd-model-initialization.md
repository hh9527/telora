# Dict 持久化后端：IC/NS 构建期准备实验

历史实验记录：PersistentMap 及持久化 Dict 后端已从当前分支删除，本文命令、接口和数据仅用于保留实验结论。

## 结论

全量切换不可变 Dict 到 RT BTree 对当前 IC/NS 初始化是负收益。DictMut 原有收集过程不变；许多小型 JSON Dict 构造后只读，未体现持久化单项更新的优势。当前列接口/codec 的临时物化也有成本，不能由此推断所有持久化结构都慢。默认开关保持关闭，不合入、不部署。

## 方法与结果

固定 Lab commit `f7dac73d4870e36a0e585d20c7c5fe0242a9540c`。同一编译器、配额和源码，以 `TELORA_EXPERIMENTAL_DICT_PD=0/1` 对比 staged build，提取 phases、usage、artifact 和完整 build information。未额外执行模型自检或知识图重建。

反序确认轮（PD → flat，无其他构建竞争）：

| 模型 | 存储 | 前端 s | 代码生成 s | 构建期准备 s | 总构建 s | 制品 bytes |
|---|---|---:|---:|---:|---:|---:|
| IC | flat | 9.51 | 2.78 | 11.82 | 25.03 | 13389300 |
| IC | PD | 9.29 | 2.80 | 14.68 | 27.68 | 13367557 |
| NS | flat | 15.13 | 3.66 | 36.59 | 56.85 | 21076237 |
| NS | PD | 15.62 | 3.70 | 46.05 | 66.85 | 21054554 |

准备阶段分别慢 24.2% / 25.9%。这里准备指 build-time `service_prepare`；`service_initialize` 各约 0.8–1 ms。第一轮 flat → PD：IC 准备 11.43 → 14.56 s，NS 37.81 → 46.98 s，方向一致。两轮 IC/NS 的完整 P2 build information 均相等，制品大小与 usage 数据在两轮一致。

| 模型/后端 | fuel | 累计分配 bytes | 存活 bytes | 托管 committed bytes | 线性内存 bytes | GC |
|---|---:|---:|---:|---:|---:|---:|
| IC flat | 25388748985 | 50926278 | 15521692 | 22759280 | 37224448 |45|
| IC PD | 33214482096 | 66920854 | 17436532 | 24674120 | 42663936 |47|
| NS flat | 85427247710 | 143439313 | 33512881 | 49641480 | 88866816 |56|
| NS PD | 105212422984 | 195456017 | 39736769 | 63907840 | 137887744 |57|

完整进程峰值 RSS 包含前端，确认轮 IC flat/PD 708076/698428 KiB，NS 911864/905468 KiB；不能据此说 PD 托管内存减少。生产配额未改：buildtime 150B fuel、256 MiB memory。

## 复现与验收

运行 `python3 scripts/measure-dict-backends.py --workspace <lab-ontology> --telora <isolated-binary> --first-backend pd`。脚本只使用 git archive 临时工作区，不修改生产配置。

原始报告在 `~/.cache/tmp/dict-model-backends-{qq1y65eq,6q9tf8wu}/report.json`。确认轮实验 CLI SHA256：`aca288f404935ef911e09fa6ce786c7afd6b44da8c893c192cbecac75e72d870`；第一轮 `805565d1309b39c87de5710e5575837711a3016c38d060b6a2e9e41b023a6c3a`。隔离副本位于 `~/.cache/tmp/dict-pd-isolated/telora`。

两个开关分别通过 telora-wasm 175 tests，3 ignored，1 filtered；过滤的是 main 已复现的 `guest_memory_growth_failure_is_distinct_from_managed_budget_exhaustion` 配额分类失败。新增测试覆盖 PD marker、DictMut、嵌套引用、历史版本和强制 GC。原有 codec、来源、动态观察、A/B 和 transform service 测试通过。telora-core 210、telora-wasm-shared 35 tests 通过。原 Dict sort_input 的 200 entries 测试预算由 10M 提到 30M，仅为覆盖较贵的实验后端；未修改生产配额。fmt、source-size、diff-check 和脚本语法检查通过。

实验构建曾更新 Lab 开发工具符号链接指向的编译器路径；已保留独立实验副本，并恢复 main 开发编译器。runner、已部署模型、配置及服务未改动。


## 后续状态

后续实现采用 PersistentMap / 列式 Map / 私有 MapMut 分工，Dict(V) 透明展开为 Map(String,V)。本记录保留的是 commit 60efbe40 的编译开关实验；其复现脚本见该提交的 scripts/measure-dict-backends.py。当前分支的模型对照使用 scripts/measure-column-map-models.py。
