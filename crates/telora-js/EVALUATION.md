# IC / NS JavaScript 后端实验

日期：2026-10-27。Node.js 26.12.1；Wasm 基线为 telora-run 的 wasmi 解释器。
两边都通过独立进程 JSONL IPC 调用，顺序重复同一请求 100 次。JS 子进程由 systemd/cgroup 限制总内存 512 MiB，禁用 swap，初始化和请求分别设墙钟预算。

## 制品和构建

| 指标 | IC | NS |
| --- | ---: | ---: |
| 单一 JS 制品 | 6,572,680 B | 9,586,707 B |
| 已部署 staged Wasm 基线 | 13,363,548 B | 21,050,485 B |
| JS 构建 | 9.07 s | 15.11 s |

表中 JS 构建为普通模式，只执行前端和代码生成，A 阶段在子进程初始化时执行；与完成 A 阶段的 staged 构建不代表相同工作量。

## 同请求性能

| 指标 | IC JS | IC Wasm | NS JS | NS Wasm |
| --- | ---: | ---: | ---: | ---: |
| 热态 IPC 中位数 | 2.34 ms | 15.04 ms | 2.42 ms | 10.17 ms |
| 热态 IPC p95 | 3.07 ms | 127.84 ms | 3.37 ms | 10.83 ms |
| 冷启动至首个响应 | 1,408.63 ms | 390.08 ms | 3,992.48 ms | 778.65 ms |
| 测试结束时进程 RSS | 231.4 MiB | 42.8 MiB | 343.4 MiB | 66.5 MiB |

JS 冷启动至首个响应包含 ready 前的加载/初始化及 ready 后第一次调用。IC ready 前 1,382.02 ms，其中加载 312.23 ms、初始化 1,000.98 ms；NS ready 前 3,965.82 ms，其中加载 452.27 ms、初始化 3,443.10 ms。

JS 的制品更小、热态执行更快，但当前总进程内存明显更多，冷启动更慢。这里比较 V8 JIT 与 wasmi，不能推广为 JS 相对所有 Wasm 引擎的结论。RSS 包括引擎与共享页，与 cgroup 计费内存不同；内存硬上限不能用 V8 堆参数替代。

## 正确性

- IC：1,880 条请求，1,858 条接受、22 条拒绝；所有成功查询均在 SQLite 中执行成功。
- NS：156 条请求，151 条接受、5 条拒绝；所有成功查询均在 SQLite 中执行成功。
- 两边接受/拒绝、SQL、bindings、列描述、警告消息及 severity 对齐。共 27 条拒绝响应的位置列表存在差异，移除 locs 后没有其他差异。
- 覆盖 IC 50 个 Dataset、NS 151 个 Dataset 的投影及 IC 测试中的多种 intent；不代表完整语言兼容性证明。

验证通过：telora-js 的 8 个执行测试、CLI backend_surface 的 2 个测试、进程 host 的 4 个测试。硬限制测试包含真实 cgroup 堆外内存超限终止、超时、响应配额及重新创建进程。源码尺寸检查与 diff 检查通过。

## 复现

先按 README 构建模型。测试套件 JSON 包含 domain、wasm、db、cases，case 包含 id、input 和可选 expectedAccepted。路径可按环境设置。

```sh
node scripts/js-model-smoke.mjs --js model.mjs --cases cases.json --runner telora-run --report smoke.json
node scripts/js-model-benchmark.mjs --js model.mjs --cases cases.json --runner telora-run --report benchmark.json
```

性能脚本默认使用套件第一条请求。模型制品和完整请求报告不提交到源码仓库。

## 后续边界

通用子进程 host 尚未接入正式 telora-run/dsh-oks 路径。JS 没有确定性 fuel，使用 host 墙钟超时与 OS 内存限制。输入来源位置传播、全量标准库、尾递归及进一步的 B 阶段元数据裁剪仍待探索。本实验保持分支推进，不合入、不部署。

## JS A/B 构建

2026-10-28，`--backend js --staged`。构建进程硬内存上限 768 MiB，运行进程仍为 512 MiB。构建期间完成 A 及闭合的 handler 工厂，将运行期可达图固化；P2 JSON 按需解析，默认做函数依赖裁剪，保守保留 property/checker 根与类型元数据。

| 指标 | IC staged JS | NS staged JS |
| --- | ---: | ---: |
| 总构建（包含 A） | 10.83 s | 20.36 s |
| 制品大小 | 11.02 MiB | 23.43 MiB |
| 启动至 ready | 383.68 ms | 872.91 ms |
| 启动至首响应 | 413.80 ms | 903.07 ms |
| 其中模块加载与恢复 | 325.33 ms | 819.43 ms |
| 其中 createService | 0.38 ms | 0.48 ms |
| 热态 IPC 中位数 | 2.23 ms | 2.43 ms |
| 热态 IPC p95 | 2.97 ms | 3.04 ms |
| 100 次请求后 RSS | 190.5 MiB | 308.6 MiB |

同一请求重复 100 次，通过真实受限子进程 IPC 测量。启动至 ready 包含子进程启动、模块加载和图恢复；createService 不足 1 ms 不代表完整冷启动不足 1 ms。构建时间包含 A，但与前一日普通 JS 的构建时间并非同一次配对测量。

相对普通 JS，启动至首响应从约 1.41/3.99 秒降到约 0.41/0.90 秒，热态耗时基本持平，RSS 从约 231/343 MiB 降到约 191/309 MiB。静态 P1/P2、闭包工厂和保守元数据让制品由 6.27/9.14 MiB 增加到 11.02/23.43 MiB；当前优先验证两阶段语义与启动收益，尺寸尚待优化。总内存仍明显高于 wasmi。

完整回归重新通过：IC 1880 请求、1858 条 SQL；NS 156 请求、151 条 SQL，无功能差异，诊断来源位置的既有差异仍存在。新增独立进程 B 验证，确认 A 专属函数可被删除、构建域被关闭，覆盖闭包捕获、TypeId、共享/循环引用、精确整数、预热 Map/WeakMap、原生返回闭包和正则。10 个执行测试、3 个 CLI 测试及 4 个资源限制测试通过；CLI 受限构建测试需设置 `TELORA_JS_HARD_LIMIT_TESTS=1`。

基础底噪单独测量 10 次：空 Node RSS 中位数 41.75 MiB，空模型 IPC worker ready 时 48.75 MiB、100 次调用后 50.50 MiB。RSS 包含共享页，不能将其解释为独占物理内存。
