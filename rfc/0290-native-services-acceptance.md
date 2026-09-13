# Native run/serve 验收记录

日期：2026-09-13。实现：`56f0135`、`1e34c81`，分支 `feat/native-cranelift`。
跟踪：#185；契约：RFC 0290、RFC 0288。原 RFC 0282 的 eval-with 验收保持完成，
本记录只描述后续授权的服务扩展。

## 逐项验收

| 要求 | 实现与当前证据 |
| --- | --- |
| 静态 adapter/协议独立于旧 codegen | core `entry_plan.rs` 提供 RunMode、adapter_source、run_contract。静态源码在 resolve 前加入整图；CLI 的 native 分支只用 seal_export 和 compile_executable。旧 codegen 复用同一静态计划。静态 adapter 与错误签名单测通过。 |
| Configure → Initialize → Reduce | `telora-native/src/service.rs`；类型由 RunContract 给出，state/reducer 留在 native 表中。service-state.telora 验证多事件状态、共享 fuel、失败后禁止重试。 |
| 模块与数据初始化顺序 | 复用已经验收的 native Session.initialize，先注入 data module，再完成 property/顶层值并统一发布；服务配置在此之后执行。CLI 对照覆盖来源、环境变量和 args。 |
| Native host 协议，无旧值桥 | `runtime/service.rs` 根据既有布局读写 SystemCaps/Resources/Event/Effect。内部数据默认值保留 native 描述符，EES 外部 JSON 边界才序列化。`native_cli/run.rs` 不调用旧 Vm、Val、Heap、resources_provider 或 bytecode codegen。 |
| run/serve 隐藏入口 | ApplicationArgs 增加局部隐藏开关；run 与 stdio serve 分流到 native。默认分支保持。帮助中不列开关，docs/README 未增加该参数。 |
| 请求/回复与错误语义 | 原 std/_entry/run、serve 策略未修改。新服务语言资产对照 `null / fail / null / null`，结果依次 1/错误/2/3；普通语言失败由原策略恢复，资源 abort 仍不能被捕获。 |
| EES 与跨事件 state | 既有 CLI 资产现在对默认和 native 双运行，覆盖 SQLite 单次及连续两次调用、并发 serve 请求、失败 EES 回复、重复 call/reply、活动 call 期间提前 Reply、EES vars、IMOS actor 绑定隔离。 |
| 原子输出与终止 | 所有本轮 effects 先验证，再调度 host；Output 按现有协议缓冲至 Exit 和 host.finish 成功后输出。`null` 后非法 JSON 的对照测试确认整个 stdout 为空。无事件时沿用无进展错误，无任意事件轮数限制。 |
| Host 与机器码资源释放 | execute_inner 的成功/失败之后均调用 ProcessRunHost.finish，取消并 join 外部任务；Session/Compiled 的 RAII 所有权释放 native 表和 JIT 代码。编译/初始化失败时尚未启动 host。 |
| 安全边界 GC | ServiceSession.collect 保活 state/reducer 和显式额外根；CallContext 拒绝活动帧、待转交尾调用与已 abort 的上下文。每轮 effects 消费后、下一事件前回收。 |
| main、来源、共享与环 | collect 复用 publish 的 TypeId 驱动遍历；main HeapRef 原样保留，work 转发表先登记再遍历，成功后整体切换 generation。20 轮单测验证真实互递归、共享、main backing、旧句柄失效。 |
| 缓存与失败原子性 | work interpreter adapter 缓存只保留显式可达图中的 adapter，main cache 保持。测试验证重定位后的身份命中、不保活死 factory、空根清空、Regex 共享与释放。复制失败恢复分配检查点，原堆与句柄仍有效。 |
| 连续请求有界存活 | 200 请求 CLI 用例处理 202 个事件、201 次回收，存活 work 对象少于 40；真实 ontology 20 请求的回收后峰值为 8 个。计数来自真实 collector。 |

## 验证命令

- `cargo test --workspace --all-features`：全部通过，包括 354 项 core、86 项 CLI、
  146 项 native 单测（另一个手动语言审计默认忽略）、3 项独立 regex 实验；其他
  workspace 套件和 doc tests 也通过。默认语言资产验收由 CLI 套件执行。
- `cargo test -p telora-native`：29 项 runtime/ABI 单测、3 项实验通过。
- `cargo build --release -p telora`、`git diff --check`：通过。

没有为了加入 native 而删掉默认后端测试。EES 等既有语言资产复用双后端运行；
新增 service-state、service-entry 资产分别覆盖低层状态机与完整 CLI。

## Release 真实负载

实现 `1e34c81`；Linux x86_64，Intel Xeon Gold 6266C，rustc 1.98.1。
使用 lab-ontology `4d8915c` 的 ontology/world-model 源码快照（与当时工作树 src
逐文件对照一致），不修改外部项目。

临时 workspace 包含 app、ontology、world-model 三个 member，app 的模块为
`@src/app`，依赖 ontology/world-model。app.telora 使用仓库资产
`crates/telora-native/tests/fixtures/ontology-service.telora`；它调用真实
world-model/bin/make-query.main.evaluate，分别包装为 Run/Serve。
依赖源码与 manifest 复制进临时 workspace 后执行 `telora lock`。

输入与前期 query 观察相同：

```json
{"op":"list","measures":[],"dimensions":["country_name"],"filters":[{"dimension":"country_continent","op":"eq","kind":"text","value":"Asia"}],"ordering":[],"limit":null,"offset":null,"output_order":[]}
```

```sh
TELORA_NATIVE_TIMINGS=1 /usr/bin/time -v target/release/telora \
  -C /tmp/telora-native-ontology-service-0Sb2lj/app \
  run --native @src/app:run --source input=/tmp/native-world-input.json

# 请求文件是同一输入的 20 行 JSONL。
TELORA_NATIVE_TIMINGS=1 /usr/bin/time -v target/release/telora \
  -C /tmp/telora-native-ontology-service-0Sb2lj/app \
  serve --native @src/app:serve --bind stdio:// < /tmp/native-service-requests.jsonl
```

默认参照去掉 `--native`。每项各执行一次，只观察，不作统计或优化结论。
run stdout 与默认逐字节一致：
`{"bindings":["Asia"],"sql":"SELECT c.Name FROM country AS c WHERE c.Continent = ?"}`。
serve 的 20 条 JSONL 响应也与默认逐字节一致。

| 观察 | native run | native serve（20 请求） |
| --- | ---: | ---: |
| frontend | 482.56 ms | 469.71 ms |
| codegen | 2904.78 ms | 2902.37 ms |
| runtime_setup | 4.05 ms | 3.75 ms |
| 模块 initialize | 3.99 ms | 4.05 ms |
| service_setup | 0.18 ms | 0.07 ms |
| reducer 总时间 | 0.74 ms | 13.88 ms |
| 回收总时间 | 0 ms | 4.63 ms |
| 事件 / 回收次数 | 1 / 0 | 22 / 21 |
| 总墙钟 | 3.43 s | 3.43 s |
| 峰值 RSS | 62436 KiB | 65924 KiB |

默认 run 为 0.52 s、50696 KiB，serve 为 0.54 s、51964 KiB。
native serve 回收前最多 650 个 work 对象，回收后最多 8 个，本轮总逻辑复制
9576 bytes。终端事件不再回收，直接随 session 释放；main 不复制。

## 范围和限制

- 保持既有 stdio JSONL/终端缓冲输出语义，不引入新的网络 transport、AOT 或默认切换。
- 只在无活动帧的事件边界回收，长时间单次调用仍受原 fuel/显式栈/逻辑分配预算限制。
  回收不恢复 abort，也不重置 fuel 或 session 累计分配预算。
- GC 管理 work 对象图。main、源码/来源数据库、host 外部事件队列和已缓冲输出
  由各自 session owner 管理；不能把存活对象数或 copied_bytes 当作 RSS 上限。
  来源数据库为保证仍可引用的诊断位置保留至 session 结束。
- 保留 64-bit little-endian ABI 和当前 host 验证范围。默认后端切换与 main 合入另行决定。

本期 run/serve 与边界回收验收完成，未保留旧 VM fallback。
