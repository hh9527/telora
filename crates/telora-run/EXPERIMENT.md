> 历史实验记录：Wasmtime 和持久化 snapshot 已移除；当前用法见 README.md。

# #212 阶段性验证

2026-09-17，Linux，Rust 1.98.1，release，wasmi 2，Wizer 49.0.0-rc.1。
基于 main f52e24fe 的独立分支实现。不是正式制品协议承诺，也不包含 Wasmtime 后端。

`scripts/build-run-smoke.mjs` 使用独立 Telora 文件，覆盖：

- LF/CRLF/CR 源码、静态 JSON 模块和注入数据的普通/snapshot 制品逐字节相同。
- 初始化编译的 Regex 在恢复后正确匹配/拒绝；普通与 snapshot 响应一致。
- 数据源位置和 fail! 位置保留；连续请求和语言错误后 reset。
- 独立验证 fuel 耗尽和 memory growth 被限制；陷阱后下一次请求成功且结果一致。
- 初始化失败不生成新文件、不覆盖已有制品；损坏的 Wasm 被拒绝。
- snapshot 拒绝重新注入 source。

原有 CLI 的 entry_services 5 项回归测试通过，run/serve 没有切换到新 Runner。

真实模型使用 lab-ontology 的 `world-model/@src/bin/make-query`，请求为国家名称查询、
`country_continent = Asia`。两种制品都输出：

```json
{"bindings":["Asia"],"sql":"SELECT c.Name FROM country AS c WHERE c.Continent = ?"}
```

以下为 9 次独立进程、交替顺序运行的中位数，文件缓存已热。进程总耗时通过
Node spawnSync + /usr/bin/time 包装测量，包含包装及进程启动，不是冷磁盘读取。
各阶段由 Runner 的 --report-timings 读取；RSS 是 /usr/bin/time 的峰值。

| 指标 | 普通制品 | snapshot |
|---|---:|---:|
| 进程总耗时 | 119.37 ms | 84.67 ms |
| 文件读取 | 3.53 ms | 3.78 ms |
| metadata | 1.18 ms | 1.19 ms |
| Module 加载 | 40.77 ms | 43.86 ms |
| 实例创建 | 1.61 ms | 2.12 ms |
| 初始化/建立恢复基线 | 44.70 ms | 1.08 ms |
| 请求前 reset | 0.0012 ms | 0.0012 ms |
| 首次请求 | 21.96 ms | 26.81 ms |
| 峰值 RSS | 21,308 KiB | 20,096 KiB |
| Guest memory | 1,835,008 B | 1,835,008 B |
| Wasm 文件 | 5,844,687 B | 6,435,375 B |

这些数据观察不包含源码编译。snapshot 保留 Guest 状态，不保留引擎惰性翻译缓存。
snapshot 的 initialize_ms 是 Runner 建立 Host 恢复基线，并非重新执行用户初始化。
各阶段中位数之和不必等于总耗时中位数。

当时未 strip 的 release binary：telora-run 5,233,016 字节；telora 24,799,728 字节。
后续小改动可能改变尺寸。cargo tree -p telora-run --edges normal 确认不依赖
telora-core、telora-wasm、telora-data、Wizer 或包管理；只共享 ABI 常量。

原始数据保存在当次机器 `/tmp/telora-212-measurements.json`，不作为长期依赖。
