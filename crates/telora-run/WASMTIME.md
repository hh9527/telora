> 历史实验记录：Wasmtime 和持久化 snapshot 已移除；当前用法见 README.md。

# #212 Wasmtime 模式验证与性能观察

2026-09-17，Linux x86_64，4 个可见 CPU，Rust 1.98.1，release。
wasmi 2.0.0；Wasmtime 49.0.0-rc.1。沿用上一阶段的同一份 world-model 普通和
snapshot Wasm，没有为了适配引擎而重新生成或改写程序。

## 实现与验证

构建期选择互斥的 `wasmi` / `wasmtime` feature。Wasmtime binary 可通过 `--mode`
选择 `pulley`、`pulley-speed`、`winch`、`cranelift-none`、`cranelift-speed`、
`cranelift-speed-and-size`。默认 Wasmtime 模式为 cranelift-speed；默认 Cargo
feature 仍为 wasmi，现有源码 CLI run/serve 未改动。

Pulley 使用 Cranelift 生成解释器字节码，分别测试 None 和 Speed 优化级别。
三个 native 模式分别使用 Cranelift None / Speed / SpeedAndSize。所有模式均
启用 fuel、内存限制。Wasmtime 开启 parallel-compilation，不启用持久化编译缓存。

wasmi、两种 Pulley 和三种 Cranelift 模式均通过独立 Telora 端到端用例：普通和
snapshot 输出相同，Regex、静态和注入数据、初始化/运行诊断保留；连续服务、fuel
耗尽、独立 memory-growth 超限后能恢复并处理下一请求。wasmi 另复核三种 EOL
制品逐字节一致。各模式使用同一服务 ABI、数据传输与 reset 实现。

Winch 已实际尝试，上游返回 `tail calls support is not enabled`。
Wasmtime Config 源码也明确把 TAIL_CALL 列为 Winch 不支持的特性。
Telora 当前制品使用 return_call_indirect，因此 Runner 给出明确的不兼容诊断，
建议选择 cranelift-none；没有删除尾调用，也没有悄悄切换编译器。
Winch 不列入下方成功执行的性能比较。

## 测量方法

world-model `@src/bin/make-query`，相同查询 `country_continent = Asia`，输出 SQL
及 bindings 在所有成功模式、所有请求间完全一致。

每个模式/制品组合启动 3 个独立进程，每进程通过 --serve 执行 31 个相同请求。
文件缓存已热，没有使用引擎持久缓存。顺序交替运行，没有并发测量。

- 首响应：Host 发起 /usr/bin/time 包装的子进程，到 stdout 收到第一行。
- Module：引擎创建及验证/翻译/编译该 Wasm 的时间。
- 预热请求：每进程排除前 5 个请求后，Guest request 时间的中位数，再取进程间中位数。
- 请求计时包含 ABI 传输和 Guest 执行，不包含 stdout 序列化、进程启动及单独记录的 reset。
- RSS：/usr/bin/time 的峰值，包含编译阶段，不等于初始化后常驻内存。
- 数值均为中位数；3 次进程样本只用于观察方向，不用于判断几个百分点的差异。

## 普通 Wasm

| 模式 | 首响应 ms | Module ms | 初始化 ms | 预热请求 ms | 峰值 RSS KiB |
|---|---:|---:|---:|---:|---:|
| wasmi | 116.05 | 40.87 | 44.07 | 1.470 | 21,436 |
| Pulley None | 1,923.25 | 1,854.34 | 52.75 | 7.171 | 220,636 |
| Pulley Speed | 2,106.26 | 2,051.05 | 39.58 | 5.313 | 194,652 |
| Cranelift None | 1,585.90 | 1,573.74 | 3.09 | 0.299 | 188,036 |
| Cranelift Speed | 1,768.84 | 1,757.12 | 2.98 | 0.301 | 175,200 |
| Cranelift SpeedAndSize | 1,751.19 | 1,739.26 | 2.98 | 0.302 | 168,412 |

## Snapshot Wasm

| 模式 | 首响应 ms | Module ms | 建立基线 ms | 预热请求 ms | 峰值 RSS KiB |
|---|---:|---:|---:|---:|---:|
| wasmi | 81.37 | 43.54 | 1.06 | 1.458 | 20,224 |
| Pulley None | 1,871.88 | 1,854.37 | 0.44 | 7.180 | 235,536 |
| Pulley Speed | 2,046.11 | 2,030.84 | 0.43 | 5.312 | 204,472 |
| Cranelift None | 1,592.77 | 1,582.48 | 0.43 | 0.301 | 198,092 |
| Cranelift Speed | 1,754.18 | 1,743.93 | 0.43 | 0.288 | 170,092 |
| Cranelift SpeedAndSize | 1,765.85 | 1,755.94 | 0.42 | 0.287 | 169,252 |

snapshot 无需重新执行初始化，表中的“建立基线”是 Runner 准备请求恢复的 Host
快照。Cranelift 下用户初始化原本只有约 3 ms，故其 snapshot 首响应没有明显收益，
几毫秒的节省被 Module 编译主导的总时间和波动覆盖。

这条查询上 native 预热请求约比 wasmi 快 4.9 倍，但多付出约 1.5–1.7 秒启动成本。
Pulley 的两个配置在本用例中都没有体现相对 wasmi 的优势。Cranelift 更高优化级别
尚未体现明显请求收益；这些判断只适用于此模型/查询和当前参数，不泛化到所有程序。

未 strip 的 release binary：wasmi 5,273,504 B；包含全部 Wasmtime 模式的 binary
20,031,072 B。这里不是针对单一 Wasmtime 策略裁剪后的最小体积。
普通/snapshot Wasm 仍分别为 5,844,687 / 6,435,375 B。

## 重现

分别构建并保留两份 binary 后运行：

```sh
node scripts/runner-backends-bench.mjs \
  --wasmi /path/to/wasmi-runner --wasmtime /path/to/wasmtime-runner \
  --ordinary world.wasm --snapshot world-snapshot.wasm \
  --input world-query.json --output measurements.json
```

端到端验证可用 `TELORA_RUN_BIN=/path/to/wasmtime-runner TELORA_RUN_MODE=pulley
TELORA_SMOKE_SINGLE_EOL=1 node scripts/build-run-smoke.mjs`，依次替换为其余成功模式。

当次原始记录：`/tmp/telora-212-backend-measurements.json` 和
`/tmp/telora-212-pulley-speed-measurements.json`。Pulley Speed 为后续单独补测，
其余模式仍使用补测前的同一实现和配置，未重复测试无变化的模式。
