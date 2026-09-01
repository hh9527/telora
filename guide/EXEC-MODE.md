# Telora 执行模式指南

Telora 提供 `eval`、`eval-with`、`run` 和 `serve` 四种执行模式。它们都从 workspace 中
选择一个公开导出，但对导出值的类型、Host 输入和执行生命周期有不同要求。

标准库模块见 [`LIBSTD.md`](LIBSTD.md)，EES 与 Actor 协议见 [`EES.md`](EES.md)，完整
命令参数见 [`TELORA-CLI.md`](TELORA-CLI.md)。

## 选择执行模式

| 命令 | 导出类型 | 外部输入 | reducer/effect | 生命周期 |
| --- | --- | --- | --- | --- |
| `eval` | `Value` | 无 | 无 | 求值一次 |
| `eval-with` | `entry.Eval` | source、声明的 env、args | 无 | 调用一次纯函数 |
| `run` | `entry.Run(State)` | source、声明的 env、args、一个 Host request | 有 | 完成一个请求 |
| `serve` | `entry.Serve(State)` | source、声明的 env、args、持续 transport request | 有 | 持续服务 |

选择原则：

- 已经能在模块求值阶段得到 Value，使用 `eval`。
- 需要由 Host 准备输入，但计算本身没有外部效果，使用 `eval-with`。
- 需要 EES 或显式 reducer 状态，并且只处理一个请求，使用 `run`。
- 需要复用同一份初始化结果和 State 持续处理请求，使用 `serve`。

四种命令都使用 `MODULE:EXPORT` 选择器。模块可以被 resolve 只表示 Host 能找到它；被
某个执行模式选择还要求导出的名义 wrapper 类型与该模式匹配。

## `eval`

`eval` 直接求值一个公开的 `std/value.Value` 导出：

```telora
import "std/value" {Value};

export def answer: Value = 'Object({
    value: 'Int(42),
    label: 'String("answer"),
});
```

```bash
telora eval @src/app:answer
```

stdout 是该 Value 的 JSON 表示。`eval` 不构造 `entry.Context`，不运行 reducer，也不
调用 EES。它适合确定的计划、schema、常量数据和完全由模块内容决定的计算。

## `eval-with`

`eval-with` 选择 `entry.Eval`。程序用 `entry.main` 声明允许的 Host 输入，并提供一个
`Fn(entry.Context) -> Value`：

```telora
import "std/dict" as dict;
import "std/entry" as entry;

def config: entry.ContextConfig = {
    sources: ["request"],
    envs: ["TARGET"],
    args: 'True,
};

export def evaluate = entry.main(config, fn(ctx) {
    match dict.get(ctx.sources, "request") {
        'Some(value) => value,
        'None => fail!("missing request source"),
    }
});
```

```bash
telora eval-with @src/app:evaluate \
  --source request=request.json \
  -- argument-1
```

`entry.ContextConfig` 与 `entry.Context` 为：

```telora
type ContextConfig = struct {
    sources: Array(String),
    envs: Array(String),
    args: Bool,
};

type Context = struct {
    sources: Dict(Value),
    env: Dict(String),
    args: Array(String),
};
```

只有 config 声明的 source 和 env 才能进入 Context。`args: 'False` 表示入口不接受命令
参数。`eval-with` 调用一次 evaluate 函数并把返回 Value 编码到 stdout；它没有 Actor
事件、State 或 EES。

## `run`

`run` 选择 `entry.Run(State)`。初始化函数接收 Context，并返回初始 State 和 reducer：

```telora
import "std/actor" as actor;
import "std/ees" as ees;
import "std/entry" as entry;

type State = struct {handled: Int};

def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};

export def run = entry.run(config, ees.none, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (
                {handled: state.handled + 1},
                [actor.reply(request.id, 'String("done"))],
            ),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({handled: 0}, reduce)
});
```

```bash
telora run @src/app:run
```

Host 初始化 service，投递一个 `actor.Request`，执行 reducer 产生的 effect，直到该请求
得到 `actor.Reply` 或执行终止。最终 Reply 的 Value 作为命令结果写到 stdout。

run 本身仍采用标准 reducer/effect 模型。`ees.none` 只表示此 service 没有 native
model；需要外部能力时换成明确的 `ees.Config` 并处理 `actor.EesReply`。

## `serve`

`serve` 选择 `entry.Serve(State)`。Telora 代码的 wrapper 和 reducer 形状与 run 相同，
区别由 Host 生命周期和请求 transport 决定：

```telora
export def serve = entry.serve(config, ees.none, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (
                {handled: state.handled + 1},
                [actor.reply(request.id, request.input)],
            ),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({handled: 0}, reduce)
});
```

```bash
telora serve @src/app:serve --bind stdio://
```

当前公开 transport 是 `stdio://` JSONL。stdin 每行是一个请求 Value；stdout 每行是一个
响应：

```json
{"ok":{"message":"done"},"error":false,"diagnostics":[]}
{"ok":null,"error":true,"diagnostics":[{"message":"invalid request"}]}
```

同一个 service State 在请求间延续。一次可恢复的请求 failure 产生 error 响应，随后仍
可处理下一行；初始化、Entry 协议或资源类终止失败会结束进程。

## Source

`eval-with`、`run` 和 `serve` 使用相同的 `--source NAME=SOURCE` 形式：

```bash
--source request=request.json
--source request=file+json://request.data
--source request=file+yaml://request.yaml
--source request=file+toml://request.toml
--source request=stdin+json://
```

省略显式 scheme 时，Host 按 `.json`、`.yaml`、`.yml` 或 `.toml` 后缀选择 parser。
NAME 来自命令行左侧，并成为 `ctx.sources` 的 key；source 文件自身的名字不会成为 key。

每个声明的 source 必须提供一次，未声明和重复的 source 都会被拒绝。单次命令最多使用
一个 stdin source。`serve --bind stdio://` 已将 stdin 用作请求通道，因此不能再用 stdin
初始化 source。

source 中 Value 的诊断来源使用稳定名称 `@run-ctx/NAME`，不暴露物理文件路径。这个
名称是数据来源，不是模块 ID。

## Env 与 args

`ContextConfig.envs` 是允许进入 `ctx.env` 的环境变量名称全集。Telora 程序不能枚举或
读取未声明的 Host 环境变量。

命令参数放在 `--` 后：

```bash
telora eval-with @src/app:evaluate -- first second
telora run @src/app:run -- first second
```

只有 `ContextConfig.args == 'True` 的入口接受这些参数。

## EES

run 和 serve 的第二个参数是 `ees.Config`。配置声明逻辑 native model 与 locator 变量；
CLI 使用 `--ees-var NAME=VALUE` 绑定声明的变量：

```bash
telora run @src/app:run --ees-var tenant=production
telora serve @src/app:serve --bind stdio:// --ees-var tenant=production
```

reducer 通过 `actor.ees_call` 发出请求，在后续 `actor.EesReply` 中处理结果。完整模型、
locator 和状态机示例见 [`EES.md`](EES.md)。

## 失败与输出

成功的 eval、eval-with 和 run 各向 stdout 写一个 JSON 值。serve 按 JSONL 写响应。
程序 failure、类型不匹配、source 错误和 Host 协议错误使对应命令返回非零，诊断写到
stderr；serve 中可恢复的单请求 failure 使用带内 error 响应。

`run --best-effort` 和 `serve --best-effort` 在启动 Entry 前扩大模块诊断覆盖。只要发现
error，就不启动 service，也不产生任何 EES effect。最终运行验收应使用普通严格模式。

## 执行前检查

```bash
telora check @src/app
telora query exports @src/app
```

`check` 检查并求值模块导出，但不会调用导出的函数或启动 service。`query exports` 可以
确认导出名称和精确 wrapper 类型。纯 Value 用 `eval` 验收，Context 函数用 `eval-with`
验收，reducer service 用 `run` 或 `serve` 验收。
