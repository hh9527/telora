# Telora 执行模式

Telora 从源码建立封闭 MIR，生成内存中的 Wasm，初始化后执行。无需中间制品落盘。

| 命令 | 入口 | 行为 |
| --- | --- | --- |
| `eval MODULE:NAME` | 一个 Value 导出 | 输出初始化后的值 |
| `run MODULE` | 类型导出 MainService | 读取一个 stdin JSON，返回一个 JSON |
| `serve MODULE --bind URI` | 同一个 MainService | 通过 JSONL 或 HTTP 连续处理独立请求 |
| `build MODULE -o app.wasm` | 同一个 MainService | 编译并保存普通 Wasm，不初始化服务 |
| `telora-run app.wasm` | 制品中的 MainService | 独立执行；指定 `--bind URI` 时持续服务 |

服务实现 `std/transform-service.TransformService`：

```telora
import "std/transform-service" as service;
import "std/value" {Value};

@service.source("knowledge")
type MyService = struct {knowledge: Value};

impl service.TransformService for MyService {
    init: fn(ctx) {
        {knowledge: ctx.sources["knowledge"]}.ty!(Self)
    },
    transform: fn(self, input) {
        Value.Object({knowledge: self.knowledge, request: input})
    },
};

export {MyService as MainService};
```

`Self` 是具体实现类型，不是运行时 trait object。MIR 在执行前确定 init 与 transform
的方法实例。模块可以正常重导出或给类型起别名；入口只要求导出名是 MainService。

```sh
printf '{"question":42}\n' | telora run @src/app --source knowledge=model.json
printf '1\n2\n' | telora serve @src/app --source knowledge=model.json --bind stdio+jsonl://
```

`init: Fn(Context) -> Self` 消费固定来源，返回初始化实例。
`transform: Fn(Self, Value) -> Value` 处理一次输入，不修改跨请求状态。
没有来源时省略 source 装饰器，Context.sources 为空。

## 来源与生命周期

`@service.source("name")` 声明逻辑来源。可以声明多个不同名称，重复声明报错。
CLI 提供的来源集合必须与声明相同，缺失、多余和重复绑定均失败。
`--source name=path.json` 按扩展名识别 JSON/YAML/TOML；`file+json://path`
等形式可显式指定格式。stdin 留给请求，两种服务命令都不接受 stdin 初始化来源。

Context 只有 `sources: Dict(Value)`。没有隐式环境变量、字符串参数或外部 I/O。
需要这些信息时，由业务宿主明确转成输入数据。初始化来源只加载一次，逐次请求的数据
直接进入 transform 的第二个参数。

来源诊断使用 `@service/name`，特殊名称按 UTF-8 字节百分号编码；请求使用 `@request`。
物理文件位置只属于 Host，不进入数据来源身份。

顺序为：模块与 property 初始化 → 准备来源 → init → 固定实例 → transform。
每次 transform 都从同一初始化状态开始；服务间隙 reset 执行环境，保证干净、确定的
起点，不重新读取外部来源。状态在 Wasm 内保留，不转成 Host JSON 再构造回来。

## 失败与配额

初始化失败不发布服务实例。普通语言失败由内置 entry 的 with_diagnostics 包装捕获；
transform 本身仍返回 Value，无需为了执行错误添加 Result。

JSONL 服务每条请求恰好输出一行；HTTP 使用相同响应封装：

```json
{"schema":"telora.service/v1","ok":42,"error":false,"diagnostics":[]}
```

失败时 `error` 为 true、`ok` 为 null。diagnostics 保留 severity、message、labels 和 notes；
其中来源坐标保留行与 UTF-8 字节偏移信息。业务返回 null 与执行失败由 error 区分。
请求成功、语言失败或配额耗尽后，下一条请求都获得独立的执行机会。

fuel/memoryLimit 只约束单次服务调用，不在整个 serve 生命周期累计；目的在于可停机，
不要求精准计费。参数读取 workspace 的 runtime 配置，CLI 可用 --with-fuel 和
--with-memory-limit 覆盖。fuel 单位为一百万，memoryLimit 单位为 MiB。
请求之间的 reset 实现与页级计量方式不构成语言语义。

run 成功只向 stdout 输出结果；失败非零退出，诊断走 stderr JSONL。serve 使用上面的
响应封装。dbg! 和 usage 观察仍输出到 stderr，不混入结果。

## 普通 Wasm 制品

```sh
cargo build --release -p telora -p telora-run
telora build @src/app -o app.wasm
telora-run app.wasm --source knowledge=model.json < request.json
telora-run app.wasm --source knowledge=model.json --bind stdio+jsonl:// < requests.jsonl
```

build 需要已更新的 workspace lock；从输入端将源码和静态数据模块中的 CRLF/CR
归一化为 LF，不修改原文件。编译成功后原子发布制品，失败不会覆盖旧文件。
制品包含程序、运行时和静态数据，不执行服务初始化，也不固化动态 --source。

telora-run 使用 wasmi，不需要源码、workspace 或编译器。启动时注入数据并初始化；
不传 --bind 则读完 stdin 的一个 JSON（直到 EOF），执行一次并退出。
制品保留构建时的默认执行预算；runner 的 --with-fuel 和 --with-memory-limit
覆盖每请求预算，不改变初始化预算。--report-usage 输出使用量诊断，
--report-timings 输出加载、初始化、reset 和请求等分阶段耗时。

当前不提供持久化 snapshot 或 Wasmtime。制品格式仍是实验版本，跨版本使用时
可能需要重新 build。独立 runner 暂不渲染 dbg! 事件，普通语言诊断正常保留。

## HTTP 与 Unix socket

两个服务入口共用 --bind 地址格式：

| 地址 | 传输 |
| --- | --- |
| `stdio+jsonl://` | stdin/stdout JSONL |
| `http://127.0.0.1:8080` | TCP HTTP/1 |
| `http+unix:///tmp/telora.sock` | Unix socket HTTP/1，仅 Unix 平台 |

```sh
telora serve @src/app --source knowledge=model.json --bind http://127.0.0.1:8080
# 或执行已构建的制品
telora-run app.wasm --source knowledge=model.json --bind http+unix:///tmp/telora.sock

curl -H 'Content-Type: application/json' -d '{"question":42}' http://127.0.0.1:8080/transform
curl --unix-socket /tmp/telora.sock -H 'Content-Type: application/json' \
  -d '{"question":42}' http://localhost/transform
```

POST /transform 接收 JSON，返回 telora.service/v1 响应。成功、语言失败和执行
陷阱都返回 HTTP 200，通过 error 区分；未知路径 404、错误方法 405、过大请求体
413。协议错误由 HTTP 层处理。服务初始化失败时不开始监听。

执行串行，请求间 reset；每连接处理一次请求后关闭。最多同时接收 64 条连接，
body 读取超时 30 秒，连接总时限 60 秒，执行本身仍依靠 Wasm 配额保证边界。
支持明文 HTTP，不提供 TLS。Unix socket 必须使用绝对路径，不覆盖已有文件，
停机后由部署方清理。旧 stdio 地址和独立 runner 的 --serve 参数已由 --bind 统一替代。

## 检查与迁移

`check --only-types` 不执行代码；普通 check 完成模块初始化，但不调用 init/transform，
也不获取服务来源。行为验证使用 run/serve 或显式调用方法的 `.telora` 测试。

旧 eval-with、entry.Eval/Run/Serve、actor reducer 和应用 EES 协议已移除。
原来承载单次查询的 source 应迁到 transform 参数；固定知识库来源才放进 init。
包管理的 IMOS Host 能力保持私有，不成为 Telora 程序的 effect system。
