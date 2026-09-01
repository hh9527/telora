# Telora

Telora 是一门实验性的静态类型语言，用于在封闭、纯、确定且保留来源的世界中，
把高层意图验证并 lowering 为不可变数据或计划。

它位于静态配置与通用脚本语言之间：程序可以使用函数、闭包、模式匹配、递归、
模块和可编程类型元数据完成一般数据计算，但不能直接访问文件、网络、时钟、进程
或环境。外部能力始终由 Host 准备、约束和解释。

```text
静态模块 + 显式输入
  -> 类型检查与元数据计算
  -> 有界的纯数据计算
  -> 完整值或来源化诊断
  -> Host 决定是否发布或执行
```

Telora 当前仍处于快速演进阶段，不提供语法或 ABI 兼容性承诺。

## 快速开始

构建命令行工具：

```bash
cargo build --release -p telora
```

建立最小 crate：

```text
hello/
  telora-config.json
  telora-crate.json
  telora-lock.json
  src/app.telora
```

`hello/telora-config.json`：

```json
{"version":1,"members":["."]}
```

`hello/telora-crate.json`：

```json
{"name":"hello","modules":["@src/app"],"dependencies":[]}
```

`hello/src/app.telora`：

```telora
import "std/actor" as actor;
import "std/ees" as ees;
import "std/entry" as entry;

type State = struct {};
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
export def run = entry.run(config, ees.none, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (
                state,
                [actor.reply(request.id, 'String("hello, telora"))],
            ),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({}, reduce)
});
```

运行：

```bash
target/release/telora -C hello lock
target/release/telora -C hello check @src/app
target/release/telora -C hello run @src/app:run
target/release/telora -C hello query exports @src/app
```

`entry.run` 保留具体 State 类型；工具阶段验证这个名义 wrapper，向 reducer 投递一个
请求，并把 `Reply` 中的 `Value` 编码为 JSON。

## 语言模型

### 值与表达式

Telora 只有表达式，没有 statement。普通值不可变，基础表示包括：

```telora
42
3.5
"text"
b"bytes"
'Ready
'Some(1)
("port", 8080)
[1, 2, 3]
{name: "Ada", active: 'True}
```

Bool 是由 `'True` 和 `'False` 构成的封闭 Atom 类型，不进行 truthiness 转换。
Array 是有序同质序列；Tuple 是固定长度异质积；record 和 Dict 在运行时共享 Dict
表示，但具有不同静态语义。

模块顶层是声明空间，只接受 `import`、`type`、`trait`、`impl`、`decl`、`def`、`native`
和 `export`。局部顺序计算使用 `let`；复杂模块值通过 `do` 表达：

```telora
export def total: Int = do {
    let base = 40;
    base + 2
};
```

### 函数与类型

```telora
def identity: for(A) Fn(A) -> A = fn(value) { value };

type User = struct {
    id: Int,
    name: String,
};

type Option(A) = enum {
    'None,
    'Some(A),
};
```

Struct 和 enum 是封闭的具名类型。即使结构相同，不同声明也不是同一类型；alias、
import 和 reexport 保留声明身份。参数化声明定义 TypeMetadata constructor；同一
constructor 使用相同类型实参时得到相同的 canonical 类型。

类型元数据由普通 Telora 计算产生，并由同一个 VM 求值。它可以同时驱动静态检查、
运行时验证、codec、schema、文档和用户空间 interpreter，不需要另一门隐藏的类型级
语言。

### 模块与静态数据

模块依赖在执行前封闭，不支持动态 import 或 `eval`。稳定模块 ID 与 crate 布局对应：

```text
@src/model       -> <crate>/src/model.telora
@test/model      -> <crate>/tests/model.telora
dep/types               -> <dependency>/src/types.telora
```

resolver 在发现模块图前按 crate 粒度冻结 first-win 来源清单：builtin crates 在先，
当前 crate 和 manifest dependencies 随后；后序同名来源不能补充或改写既有 crate。

JSON、YAML 和 TOML 文件也是静态模块，并统一导出 `std/value.Value`：

```telora
import "./request.json" { data as request };
```

它们在模块图封闭时由 Host 加载，不是运行时文件 IO。

### Codec 与展示

```telora
import "std/codec" as codec;
import "std/json" as json;
import "std/result" as result;
import "std/value" { Value };

type Request = struct { subject: String, limit: Int };

def raw_text: String = "{\"subject\":\"orders\",\"limit\":20}";
def request: Request = json.decode(Request, raw_text) |> result.unwrap;
def encoded: Value = codec.encode(Value, request) |> result.unwrap;
```

`std/codec` 在 `Value` 与有类型值之间转换；`std/json` 负责 JSON 文本和 schema。
Decorator 是产生 attribute 的普通元数据函数，codec 与 schema 读取同一份元数据。

字符串插值 `` `value=\{value}` `` 只依据运行时 primitive meta 支持 String、Int、
Float 和 Atom，不隐式调用用户 Display。稳定的数据交换使用 codec；临时观察使用
`dbg!`；显式的面向人展示可以使用 `std/fmt`。

## 诊断与 best-effort

Telora 的值携带来源。失败可以同时指出不满足规则的数据位置和规则位置：

```telora
def require_positive: Fn(Int) -> Int = fn(value) {
    if value > 0 { value }
    else { fail!("expected a positive value", value) }
};
```

公共函数应直接承诺成功类型 `T`。无法产生合法 `T` 时使用 `fail!`，而不是为了
向 Host 报告诊断就把所有 API 改写为领域 `Rejection`。业务调用者确实需要恢复或
分支时，再显式使用 `Option`、`Result` 或领域 enum。

严格执行遇到失败立即中止。`--best-effort` 会在内部传播 Fail，并继续彼此独立的
计算，以便一次获得更多有意义的诊断；它不保证与严格模式经过完全相同的求值路径。
只要存在 error，最终返回值和效果都不会越过 Host 发布边界。

常用诊断组合包括：

```telora
checker.should_ok!(value)  # Result 的 Err 产生 Warning，返回 Option
checker.must_ok!(value)    # Result 的 Err 产生失败，返回 Ok payload
result.try_unwrap!()       # Warning + Option
result.unwrap!()           # failure + payload
value.dbg!("message")     # 返回原值，向 Host 发送 JSONL 观察
```

## Host 与 Entry

Telora 程序本身没有外部权限。普通模块通过 `std/entry` 构造工具可识别的名义值：
`Eval`、`Run(State)` 或 `Serve(State)`。resolver 只负责模块身份和可见性；CLI 在工具
阶段选择 `MODULE:EXPORT`，检查 wrapper 类型，并调用内置 Entry adapter。

`Run(State)` 和 `Serve(State)` 包含 Context 契约、EES 声明和初始化函数。初始化函数
返回具体 State 与 `reduce(State, Event) -> (State, Array(Effect))`。`run` 产生一次请求，
`serve` 持续接收 transport 请求。SQLite 与 IMOS model 由 `std/ees.sqlite_model` 和
`std/ees.imos_model` 构造；locator 变量在 `ees.Config.vars` 中约束，并由
`--ees-var NAME=VALUE` 绑定。reducer 用 `EesCall` 描述调用，并在后续 `EesReply` 中处理
结果。CLI Host 执行 EES effect；应用模块不能直接访问文件、环境、网络或进程。

`serve --bind stdio://` 对每行请求返回包含 `ok`、`error` 和 `diagnostics` 的 JSON。
可恢复的请求失败不会结束服务；初始化、协议和资源类终止失败仍带外报告并退出。

## 命令行

当前命令面包括：

```text
telora eval <module:name>  求值 module 的一个 Value 导出
telora eval-with <module:name> [--source ...] [-- args...]  调用一个 entry.Eval 值
telora run <module:name>   向一个 entry.Run(State) 投递请求
telora serve <module:name> 通过 stdio JSONL 驱动一个 entry.Serve(State)
telora lock                物化 package source 并原子刷新 workspace lock
telora check <module-id>   以 best-effort 策略检查并求值模块导出
telora query ...           以 JSONL 查询模块和语义事实；别名 q
telora lsp                 启动语言服务器
```

`telora-ees` 组合内置 Native Actor Components：IMOS 提供 `InstallShared`，
`sqlite-query` 提供参数绑定的只读 `Query`。
普通 package preparation 在进程内构造只含 `telora-packages` IMOS actor 的私有 Service；
应用 wrapper 构造另一个 Service，并只向应用暴露逻辑 actor 名称。资源使用
`user-data:`、`user-cache:`、`user-config:` 或 `user-state:` locator；component adapter
按 XDG 目录解释，并在 XDG 变量缺失时回退到 `$HOME` 下的标准目录。解析后的物理路径
不进入 Telora World。应用不能发现或调用包管理 Service。两条路径都不需要额外 executable。

`eval` 要求导出类型为 `Value`。`eval-with` 要求导出类型为 `entry.Eval`，其中
`entry.ContextConfig` 声明 source、环境变量和参数能力。两条命令只进行 module 求值和
至多一次普通函数调用，不运行 reducer loop 或应用 EES。

`query` 包含：

```text
telora query modules [-p pattern]
telora query exports <module-id> [-p pattern]
telora query at <module-id>[:line[:column]] [-p pattern] [-k kinds]
```

JSONL 位置默认使用 1-based line 和 0-based UTF-8 byte column；LSP 按协议协商位置
编码。`check` 不进行 Entry 调度，也不会调用已导出的函数；纯导出由 `eval` 或
`eval-with` 验收，应用 service 由严格 `run` 验收。遇到应用初始化问题时可使用
`run --best-effort` 扩大诊断覆盖。

## 资源限制

Telora 允许递归，但每次执行受 fuel、栈、调用深度、分配和取消边界约束。资源耗尽
产生结构化失败，不发布部分结果。程序仍应表达自身算法需要的语义边界，不能把 Host
fuel 当作正常终止条件。

## 文档

- [guide/TELORA.md](guide/TELORA.md)：语言使用教程与当前限制。
- [guide/WORKSPACE.md](guide/WORKSPACE.md)：workspace、crate、模块清单与依赖锁定。
- [guide/LIBSTD.md](guide/LIBSTD.md)：标准库模块定位与接口发现。
- [guide/EXEC-MODE.md](guide/EXEC-MODE.md)：eval、eval-with、run 与 serve 执行模式。
- [guide/EES.md](guide/EES.md)：Native Effect Service、Actor 协议与外部效果。
- [guide/TELORA-CLI.md](guide/TELORA-CLI.md)：CLI、工作区解析和 JSONL 契约。
- [docs/design/LANGUAGE.md](docs/design/LANGUAGE.md)：当前语言设计 SSOT。
- [docs/design/CONCEPT.md](docs/design/CONCEPT.md)：核心概念和所有权边界。
- [docs/MOTIVATION.md](docs/MOTIVATION.md)：问题域、动机与能力准入原则。
- [rfc/](rfc/)：设计决策的历史、方案与验收证据。
- [tree-sitter-telora/](tree-sitter-telora/)：Tree-sitter grammar。

## 验证

```bash
cargo test --workspace
cd tree-sitter-telora
npx tree-sitter test
```
