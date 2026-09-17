# 独立 Wasm Runner（实验）

在 #212、`feat/wasm-build-snapshot-runner` 分支中探索制品发布。支持互斥的
`wasmi`（默认）和 `wasmtime` features。制品 envelope 为实验版本，
不承诺跨版本兼容；Wizer 使用 `49.0.0-rc.1` 的拆分式快照接口。

运行库和 binary 均不依赖 Telora 编译器、MIR、codegen、包管理或 Wizer。
现有 `telora run/serve` 仍使用原来的路径，尚未接入本库。

```sh
cargo build --release -p telora -p telora-run

telora -C project build @src/main -o app.wasm
telora-run app.wasm --source model=model.json < request.json
telora-run app.wasm --source model=model.json --serve < requests.jsonl

telora -C project build @src/main --snapshot --source model=model.json -o ready.wasm
telora-run ready.wasm < request.json
telora-run ready.wasm --serve < requests.jsonl
```

Wasmtime 版本使用同一个制品和服务协议：

```sh
cargo build --release -p telora-run --no-default-features --features wasmtime
telora-run ready.wasm --mode pulley
telora-run ready.wasm --mode pulley-speed
telora-run ready.wasm --mode cranelift-none
telora-run ready.wasm --mode cranelift-speed
telora-run ready.wasm --mode cranelift-speed-and-size
```

| 模式 | 执行方式 |
|---|---|
| `wasmi` | wasmi 构建的默认模式，惰性翻译后解释执行 |
| `pulley` | Cranelift `OptLevel::None` 生成 Pulley 字节码，再解释执行 |
| `pulley-speed` | Cranelift `OptLevel::Speed` 生成优化后的 Pulley 字节码，再解释执行 |
| `cranelift-none` | Cranelift 不优化，生成本机代码 |
| `cranelift-speed` | Cranelift 速度优化，Wasmtime 构建的默认模式 |
| `cranelift-speed-and-size` | Cranelift 同时考虑速度与代码大小 |
| `winch` | Winch 基线编译；当前上游不支持尾调用，会明确拒绝含尾调用的 Telora 制品 |

Pulley 不是直接解释 Wasm，也有完整的预先编译成本。所有模式启用 fuel 和内存限制，
不使用持久化的引擎编译缓存；没有把尾调用退化成普通调用以迁就 Winch。
Wasmtime 开启 parallel-compilation，采用其默认并行编译策略。
构建出的 Wasmtime binary 包含上述各模式，wasmi/wasmtime 的选择仍在构建期完成。

`build` 选择导出 `MainService` 的源码模块。源码（包括 builtin）、静态数据模块，
以及 snapshot 的注入文本，都在进入编译器/数据解析器之前将 CRLF 和 CR 转为 LF；
不会改写原文件，反斜杠转义 `\r` 不受影响。普通制品带静态数据 bundle，不执行初始化。
snapshot 成功初始化后保存完整 Guest memory 和数值 globals，移除已经消费的数据 bundle；
运行时不再次初始化或注入数据，故拒绝 `--source`。失败构建不会创建或覆盖最终输出文件。

单次运行向 stdout 输出成功的 JSON 值，诊断写到 stderr，失败退出码为 1。
`--serve` 对每行输入输出一个 `telora.service/v1` envelope；诊断和陷阱不阻止处理下一条请求。
完成请求后走 truncate reset；陷阱后重建实例并恢复初始化基线。基线副本占用额外 Host 内存。
本阶段不提供旧 Host 的 `debug!` 事件渲染能力；普通语言诊断由 Guest 生成。

制品携带初始化预算和请求默认预算。`--with-fuel N`（单位百万）与
`--with-memory-limit N`（单位 MiB）覆盖单次请求预算，不改变普通制品的初始化预算。
请求内存边界为初始化 memory 大小加请求 allowance。fuel 用于保证执行有边界，
不保证不同引擎间精确计费或可比较。

`--report-usage` 输出合法的 JSON 使用量诊断；`--report-timings` 输出文件读取、metadata、
引擎 Module 加载、实例创建、初始化/基线建立、请求前 reset、Guest 请求分别耗时。
计时不包含进程创建；输入文件读取、stdout/stderr 处理等不包含在 Guest request 时间内。
snapshot 不保存引擎翻译缓存，所以仍需要加载/验证和可能的首次请求惰性翻译。

端到端验证使用独立 `.telora` 文件：

```sh
node scripts/build-run-smoke.mjs
# 对已构建的 Wasmtime binary 检查指定策略；EOL 构建逻辑不重复验证三遍。
TELORA_RUN_MODE=pulley TELORA_SMOKE_SINGLE_EOL=1 node scripts/build-run-smoke.mjs
```

覆盖三种 EOL 的逐字节制品一致性、静态和注入数据、初始化时编译的 Regex、诊断、
普通/snapshot 输出、fuel/内存陷阱后恢复、失败构建保留已有文件。

`scripts/runner-backends-bench.mjs` 可对同一普通/snapshot 制品比较首响应、Module
编译、初始化、首次请求和预热后的请求，以及峰值 RSS。参数包括 `--wasmi`、
`--wasmtime`（分别指向两份 binary）、`--ordinary`、`--snapshot`、`--input` 和
`--output`；默认每种组合 3 个独立进程，每进程 31 个相同 JSON 请求。
当前观察见 [WASMTIME.md](WASMTIME.md)，上一阶段 wasmi 单次运行记录见
[EXPERIMENT.md](EXPERIMENT.md)。
