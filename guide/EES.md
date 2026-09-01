# Telora EES 指南

EES（Extra Effect Service）是 Telora Actor 访问外部能力的 Host 边界。Telora reducer
只接收数据事件并产生数据效果；Host 根据 Entry 声明构造 native model、执行 EES 请求，
再把结果作为新事件送回 reducer。

本文面向编写 `entry.Run(State)` 和 `entry.Serve(State)` 的程序作者。标准库索引见
[`LIBSTD.md`](LIBSTD.md)，命令行参数和响应格式见 [`TELORA-CLI.md`](TELORA-CLI.md)。

## 分层

EES 程序由三个公开模块组成：

| 模块 | 职责 |
| --- | --- |
| `std/ees` | 声明 native model，并构造发送给 model 的请求 |
| `std/actor` | 定义 Event、Effect、Service 与 reducer transition |
| `std/entry` | 声明 Context，构造 Host 可调用的 Run 或 Serve 值 |

数据流是：

```text
Host Request
  -> actor.Event
  -> reduce(State, Event)
  -> (State, Array(actor.Effect))
  -> Host 执行 EES effect
  -> actor.EesReply
  -> reduce(...)
  -> actor.Reply
```

reducer 不执行 I/O，也不接收 callback。等待外部结果期间所需的阶段、关联信息和业务
上下文都保存在显式 State 中。

## Entry 配置

`entry.ContextConfig` 声明初始化函数可读取的 Host 输入：

```telora
type ContextConfig = struct {
    sources: Array(String),
    envs: Array(String),
    args: Bool,
};
```

初始化函数收到：

```telora
type Context = struct {
    sources: Dict(Value),
    env: Dict(String),
    args: Array(String),
};
```

`entry.run` 和 `entry.serve` 接受相同形状的初始化函数：它返回初始 State 和 reducer。

```telora
entry.run(config, ees_config, fn(ctx) {
    let initial: State = ...;
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) =
        fn(state, event) { ... };
    (initial, reduce)
})
```

Run 由 Host 投递一个输入请求；Serve 可以持续接收请求。两者都使用相同的
reducer/effect 协议。

## EES 配置

`ees.Config` 在 Entry 值中声明变量约束和 native model：

```telora
type Config = struct {
    vars: Dict(String),
    models: Array(Model),
};
```

没有 native model 的程序使用：

```telora
ees.none
```

有 model 的程序显式给出逻辑名称。reducer 只使用这个名称，不接触 Host 的物理资源
句柄：

```telora
def effects: ees.Config = {
    vars: {"tenant": "[a-z][a-z0-9-]{0,31}"},
    models: [
        ees.sqlite_model(
            "catalog",
            "user-data:{tenant}/catalog.sqlite",
        ),
    ],
};
```

CLI 使用 `--ees-var tenant=production` 提供变量。每个变量必须在 `vars` 中声明，并满足
对应的完整正则约束。变量只参与 Host locator 解析，不会进入 `entry.Context`。

## Locator

内置 component 使用逻辑 locator 表达用户范围内的资源位置：

```text
user-data:path
user-cache:path
user-config:path
user-state:path
```

Host 按 XDG 用户目录解释这些前缀；相应 XDG 环境变量不存在时，使用基于 HOME 的标准
位置。插值只接受 `ees.Config.vars` 中声明并经正则校验的值，例如：

```text
user-data:{tenant}/catalog.sqlite
user-cache:materialized-store
```

解析后的物理路径只对 native component 可见，不进入 Telora Value、World 或诊断来源。

## Actor 协议

Host 请求以 `actor.Event` 进入 reducer：

```telora
type Request = struct {id: String, input: Value};

type Event = enum {
    'Request(Request),
    'EesReply(EesReply),
};
```

reducer 返回 `actor.Transition(State)`，即：

```telora
Tuple([State, Array(actor.Effect)])
```

两种效果分别用于调用 EES 和回复 Host 请求：

```telora
actor.ees_call(effect_id, request_id, ees_request)
actor.reply(request_id, value)
```

`effect_id` 标识一次在途 EES 调用；`request_id` 把最终结果关联到触发它的 Host 请求。
收到 EES 结果时，`reply.id` 是 effect ID，`reply.request_id` 是 Host request ID：

```telora
type EesReply = struct {
    id: String,
    request_id: String,
    result: Result(Value, String),
};
```

并发或多阶段流程应在 State 中保存所有在途关联。每个 transition 可以产生多个 effect；
同一 transition 内的 EES effect ID 必须能唯一标识对应调用。

## SQLite Query

SQLite model 由逻辑名称和数据库 locator 构造：

```telora
ees.sqlite_model("catalog", "user-data:{tenant}/catalog.sqlite")
```

查询请求为：

```telora
ees.sqlite_query(
    "catalog",
    "SELECT name, score FROM items WHERE score > ? ORDER BY score DESC",
    ['Int(1)],
)
```

bindings 的类型是 `Array(std/value.ScalarValue)`，支持 null、Bool、Int、Float 和 String。
component 返回 `std/value.Value`。查询成功值的结构由 SQLite Query component 契约决定；
当前结果包含 `columns` 和 `rows`。

完整的单次查询程序：

```telora
import "std/actor" as actor;
import "std/ees" as ees;
import "std/entry" as entry;

def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
def effects: ees.Config = {
    vars: {"tenant": "[a-z][a-z0-9-]{0,31}"},
    models: [ees.sqlite_model(
        "catalog",
        "user-data:{tenant}/catalog.sqlite",
    )],
};

type State = enum {'Ready, 'Waiting};

export def run = entry.run(config, effects, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match (state, event) {
            ('Ready, 'Request(request)) => (
                'Waiting,
                [actor.ees_call(
                    "query",
                    request.id,
                    ees.sqlite_query(
                        "catalog",
                        "SELECT name FROM items WHERE score > ? ORDER BY score DESC",
                        ['Int(1)],
                    ),
                )],
            ),
            ('Waiting, 'EesReply(reply)) => match reply.result {
                'Ok(value) => ('Ready, [actor.reply(reply.request_id, value)]),
                'Err(message) => fail!("SQLite query failed", message),
            },
            _ => fail!("unexpected actor event", state, event),
        }
    };
    ('Ready, reduce)
});
```

运行时绑定 locator 变量：

```bash
telora run @src/app:run --ees-var tenant=production
```

## IMOS InstallShared

IMOS model 需要 store 与 home 两个 locator：

```telora
ees.imos_model(
    "materializer",
    "user-cache:store",
    "user-data:home",
)
```

`store` 保存按内容或稳定 key 复用的物化结果，`home` 是计划结果的发布位置。
`install_shared(actor, plan)` 构造 `InstallShared` 请求，其中 plan 是普通 Value：

```telora
let call = ees.install_shared("materializer", plan);
let effect = actor.ees_call("install", request.id, call);
```

Telora 自身的 package preparation 使用独立的 Host 私有 service。应用 Entry 声明的
IMOS model 只处理应用发出的请求，不能发现或调用 package service。

## 多阶段状态机

EES reply 是后续事件，因此多次调用通过 State 显式排序：

```text
'Ready
  --Request / EesCall("load")--> 'Loading(request_id)
  --EesReply("load") / EesCall("save")--> 'Saving(request_id)
  --EesReply("save") / Reply--> 'Ready
```

State 应保存下一阶段需要的 request ID、业务数据和在途 effect ID。这样 reducer 的每次
调用都只依赖当前 State 与 Event，Host 可以记录完整的效果与回复时间线。

## Failure 与回复

EES component 的业务失败进入 `EesReply.result = 'Err(message)`。程序可以把它转换为
领域 Value、继续其他阶段，或者用 `fail!` 产生带诊断的请求失败。

`actor.reply(request_id, value)` 表达成功完成一个 Host 请求。`serve --bind stdio://`
会把结果包装成带 `ok`、`error` 和 `diagnostics` 的 JSON 响应；一次可恢复 failure 不会
结束后续服务。初始化错误、Entry 协议错误和资源类终止失败会结束命令。

## 接口发现

```bash
telora query exports std/ees
telora query exports std/actor
telora query exports std/entry
telora query at std/ees -p sqlite
```
