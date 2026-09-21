# 独立 Wasm Runner（实验）

`telora build` 生成普通或带初始化快照的 Wasm 制品，`telora-run` 使用 wasmi 执行。
运行库和 binary 不依赖 Telora 编译器、MIR、codegen 或包管理。
现有源码 `telora serve` 共用本库的传输层，编译和执行仍走原入口。制品 envelope 为实验版本，不承诺跨版本兼容。

```sh
cargo build --release -p telora -p telora-run
telora -C project build @src/main -o app.wasm
telora-run app.wasm --source model=model.json < request.json
telora-run app.wasm --source model=model.json --bind stdio+jsonl:// < requests.jsonl
telora -C project build @src/main --snapshot --source model=model.json -o app.wasm
telora-run app.wasm < request.json
```

`build` 选择导出 `MainService` 的源码模块。在输入端把源码（包含 builtin）和
静态数据模块的 CRLF/CR 归一化为 LF，不改写原文件，反斜杠转义不受影响。
制品携带静态数据 bundle，不执行初始化。失败构建不会覆盖最终输出。

Runner 注入静态数据和 `--source` 后初始化服务。带 snapshot 的制品在没有
`--source` 时直接恢复 ready service；一旦显式提供 `--source`，则忽略快照并走完整
初始化，因此同一制品同时支持快速默认启动和来源覆盖。单次运行向 stdout 输出成功的
JSON 值，诊断写到 stderr，失败退出码为 1。`--bind stdio+jsonl://` 每行输入输出一个
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

Wasmtime 已根据实验结果移除。早期 Wizer snapshot 实验数据保留在
[WASMTIME.md](WASMTIME.md) 和 [EXPERIMENT.md](EXPERIMENT.md)；当前实现使用独立
`telora.snapshot` custom section，不改写原始 Wasm 初始化路径。

HTTP 服务：

```sh
telora-run app.wasm --source model=model.json --bind http://127.0.0.1:8080
telora serve @src/main --source model=model.json --bind http+unix:///tmp/telora.sock
curl -X POST http://127.0.0.1:8080/transform -d '"query"'
curl --unix-socket /tmp/telora.sock -X POST http://localhost/transform -d '"query"'
```

HTTP/1 的 `POST /transform` 返回与 JSONL 相同的 envelope；语言诊断和执行陷阱
以 HTTP 200 加 `error: true` 表示。错误路径返回 404，错误方法 405，过大的
请求体 413。HTTP 分帧错误由协议层处理。每连接处理一次请求后关闭。
最多同时接收 64 条连接，读取 body 超时 30 秒，连接总时限 60 秒；
这些 I/O 时限不代替 Wasm 的执行配额。请求执行仍串行。

Unix socket 仅支持 Unix 平台，路径必须是绝对路径。不会覆盖或自动删除既有
socket 文件；停机后由部署方清理该路径。当前只支持明文 HTTP。
