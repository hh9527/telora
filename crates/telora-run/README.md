# 独立 Wasm Runner（实验）

`telora build` 生成普通 Wasm 制品，`telora-run` 使用 wasmi 执行。
运行库和 binary 不依赖 Telora 编译器、MIR、codegen 或包管理。
现有 `telora run/serve` 尚未接入本库。制品 envelope 为实验版本，不承诺跨版本兼容。

```sh
cargo build --release -p telora -p telora-run
telora -C project build @src/main -o app.wasm
telora-run app.wasm --source model=model.json < request.json
telora-run app.wasm --source model=model.json --serve < requests.jsonl
```

`build` 选择导出 `MainService` 的源码模块。在输入端把源码（包含 builtin）和
静态数据模块的 CRLF/CR 归一化为 LF，不改写原文件，反斜杠转义不受影响。
制品携带静态数据 bundle，不执行初始化。失败构建不会覆盖最终输出。

Runner 注入静态数据和 `--source` 后初始化服务。单次运行向 stdout 输出成功的
JSON 值，诊断写到 stderr，失败退出码为 1。`--serve` 每行输入输出一个
`telora.service/v1` envelope。完成请求后 truncate reset；陷阱后重建实例并
恢复内存中的初始化基线，继续处理下一条请求。基线副本占用额外 Host 内存。
暂不渲染旧 Host 的 `debug!` 事件，普通语言诊断由 Guest 生成。

制品保存初始化和请求默认预算。`--with-fuel N`（百万）和
`--with-memory-limit N`（MiB）覆盖单次请求预算，不改变初始化预算。
请求内存边界为初始化 memory 大小加请求 allowance。fuel 用于保证有边界、能停机。

`--report-usage` 输出 JSON 使用量诊断；`--report-timings` 分别记录文件读取、
metadata、Module 加载、实例创建、初始化、reset 和请求耗时，不含进程创建时间。

端到端验证：`node scripts/build-run-smoke.mjs`，覆盖三种 EOL 制品一致性、
静态和注入数据、Regex、诊断、连续请求、fuel/内存陷阱恢复和失败发布保护。

Wasmtime 与持久化 snapshot 已根据实验结果移除。历史数据保留在
[WASMTIME.md](WASMTIME.md) 和 [EXPERIMENT.md](EXPERIMENT.md)，其中命令描述的是历史实现。
