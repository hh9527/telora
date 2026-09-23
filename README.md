# Telora

Telora 是一门实验性的静态类型语言，用于在封闭、纯、确定且保留来源的世界中，
把高层意图验证并 lowering 为不可变数据或计划。

它位于静态配置与通用脚本语言之间：程序可以使用函数、闭包、模式匹配、递归、
模块和可编程类型元数据完成一般数据计算，但不能直接访问文件、网络、时钟、进程
或环境。外部能力始终由 Host 准备、约束和解释。

```text
静态模块 + 显式输入
  -> 全图符号与类型求解（不执行 Telora 代码）
  -> 编译、数据注入与顶层值/property 初始化
  -> 有界的纯数据计算
  -> 完整值或来源化诊断
  -> Host 决定是否发布或执行
```

Telora 当前仍处于快速演进阶段，不提供语法或 ABI 兼容性承诺。

## 快速开始

构建命令行工具：

```bash
cargo build --release -p telora -p telora-run
```

建立最小 crate：

```text
hello/
  telora-config.json
  telora-crate.json
  telora-lock.json
  src/lib.telora
```

`hello/telora-config.json`：

```json
{"version":1,"members":["."]}
```

`hello/telora-crate.json`：

```json
{"name":"hello","dependencies":[]}
```

`hello/src/lib.telora`：

```telora
use std::transform_service as service;
use std::value::{Value};
type Greeting = struct {};
impl service::TransformService for Greeting {
    init: fn(ctx) { {}.ty!(Self) },
    transform: fn(self, input) { Value::String("hello, telora") },
};
@service::collection
pub type MainService = struct {
    @service::slot("greet") greet: Greeting,
};

```

运行：

```bash
target/release/telora -C hello lock
target/release/telora -C hello check @src/lib
target/release/telora -C hello check --only-types @src/lib
target/release/telora -C hello check --lib
target/release/telora -C hello check --tests --only-types
printf '{"method":"greet","input":null}\n' | target/release/telora -C hello run @src/lib
target/release/telora -C hello query exports @src/lib
```

模块公开带 `@service::collection` 的 MainService struct，其字段分别实现
`std::transform_service::TransformService`。MIR 封闭所有方法实例；Host 准备声明的来源并初始化各字段服务，run 处理一个带 method/input 的 stdin JSON，
serve 通过 JSONL 或 HTTP 处理请求。每次调用从同一初始化状态开始，服务间隙 reset；fuel/memory 配额只
约束单次调用。来源与诊断细节见 [执行模式](guide/EXEC-MODE.md)。

`check --only-types` 和 `query` 已接入新的三个静态 Pass，直接消费 MIR，
不执行 property、`@check` 或模块值。JSON/TOML/YAML 模块不读取或解析内容；
内容语法及数据限制由后续加载阶段检查。
所有编译入口共用 module-resolve、symbol-resolve、type-resolve 三个 Pass。
执行前由 SealedMir 确认类型、泛型实例和必要证据已经闭合，codegen 和 VM 直接消费
这些结果。查询可以保留错误图中的已确定信息，不回退到旧求解器。
执行命令统一从源码生成内存中的 Wasm，再由 Wasmi 执行；无需先生成 `.wasm`
文件，也无需在用户执行时安装或调用外部 linker。
普通 `check` 保持完整检查行为。两种模式的 JSON summary
均包含 `catalog_seconds` 和 `check_seconds`，分别记录清单准备及所选检查路径耗时。
两种模式共用 MIR 类型闭合阶段；`static_seconds` 包含类型闭合，
`execution_seconds` 包含 codegen、链接和 VM 初始化（`--only-types` 时为零）。

`check --lib` 一次检查从当前 crate 的 `src/lib.telora` 挂载的模块树（包含私有模块和
数据模块）；
`check --tests` 递归检查当前 crate 的 `tests/` 下全部模块，包含辅助模块，
但不执行 Test 用例。两个开关可以组合使用，与显式模块选择器互斥。
所选模块作为多个根进入同一张 MIR 图，共享依赖只求解和初始化一次。
带 `--only-types` 时停在类型闭合阶段，否则完成整图初始化，不调度 entry。
空集合检查成功；JSON summary 的 `roots` 列出按名称排序的根模块。

## 语言模型

### 值与表达式

Telora 只有表达式，没有 statement。普通值不可变，基础表示包括：

```telora
42
3.5
"text"
b"bytes"
True
Some(1)
("port", 8080)
[1, 2, 3]
{name: "Ada", active: True}
```

Bool 只接受 `True` 和 `False`，不进行 truthiness 转换。用户 enum 的构造使用
声明限定名，例如 `Status::Ready`；不使用带单引号的旧 tag 语法。
Array 是有序同质序列；Tuple 是固定长度异质积；具名 Struct 的字段由声明确定，Dict(T) 的键可变化而值类型固定。
记录字面量必须得到具名 Struct 或 Dict(T) 的完整类型，不能留下匿名 Record 值。

模块顶层是声明空间，只接受 `mod`、`use`、`data`、`type`、`trait`、`impl`、`decl`、
`def` 和 `native`；公开声明使用 `pub`。局部顺序计算使用 `let`；复杂模块值通过
`do` 表达：

```telora
pub def total: Int = do {
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

type Maybe(A) = enum {
    None,
    Some(A),
};
```

Struct 和 enum 是封闭的具名类型。即使结构相同，不同声明也不是同一类型；alias、
`use` 和 `pub use` 保留声明身份。参数化声明定义静态类型族；同一声明
使用相同类型实参时得到相同的 canonical 类型。

`T` 是静态类型，`T.type` 将其投影为精确的 `TypeOf(T)` 元数据，可作为 `Type`
传递。普通函数可以观察和组合元数据，但不能把函数返回的元数据反向用作类型声明。
类型骨架由声明和受信任的类型构造器建立；typed property 与用户空间 interpreter
在此基础上支持验证、codec 和文档生成。工具阶段和程序阶段共用求值器。

### 模块与静态数据

模块依赖在执行前封闭，不支持动态加载模块或语言内 `eval`。每个 crate 从
`src/lib.telora` 开始，由 `mod` 声明显式挂载代码子模块；`use` 只建立名字绑定。
稳定模块 ID 与 crate 布局对应：

```text
@src/model       -> <crate>/src/model.telora
@test/model      -> <crate>/tests/model.telora
dep/types               -> <dependency>/src/types.telora
```

resolver 在发现模块图前按 crate 粒度冻结 first-win 来源清单：builtin crates 在先，
当前 crate 和 manifest dependencies 随后；后序同名来源不能补充或改写既有 crate。

JSON、YAML 和 TOML 通过 `data` 声明挂载，并得到 `std::value::Value`：

```telora
data request = import(json) "./request.json";
```

静态阶段只登记其 `Value` 接口；类型闭合后才加载和解析内容。
程序本身不能读取任意文件。

### Codec 与展示

```telora
use std::codec as codec;
use std::json as json;
use std::value::{ Value };

type Request = struct { subject: String, limit: Int };

def raw_text: String = "{\"subject\":\"orders\",\"limit\":20}";
def request: Request = json::decode@[Request](raw_text).unwrap!();
pub def encoded: Value = codec::encode(request);
```

`std/codec` 在 `Value` 与有类型值之间转换；`std/json` 负责 JSON 文本。
Decorator provider 计算 typed property，codec 消费对应的类型元数据和 property。

字符串插值要求每个表达式具有静态选定的 `std::fmt::Display` 实现。
String、Int、Float 有标准实现；具名类型可以实现 Display 或使用 `fmt::display_by`。稳定的数据交换使用 codec；临时观察使用
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

静态阶段收集独立诊断，类型未闭合时不进入求值。check 初始化传播已有失败并继续独立
导出根，但任何错误都会阻止成功发布。多根初始化诊断使用 `check`；运行命令
遇到失败即停止，不提供 `--best-effort`，详见 [CLI 指南](guide/TELORA-CLI.md)。
只要存在 error，最终返回值和效果都不会越过 Host 发布边界。

常用诊断组合包括：

```telora
checker(value).ok_or_warn!()  # Ok -> Some；Err -> warn!，返回 None
checker(value).unwrap!()      # Ok -> payload；Err -> raise!
value.dbg!("message")     # 返回原值，向 Host 发送 JSONL 观察
```

`raise!` 和 `warn!` 接受 String 或 BlameError：String 只提供 message，BlameError
还提供显式数据引用；报告时添加实际宏调用处的 rule，不自动附加调用参数或 Result
容器的来源。`raise!` 返回 Never，`warn!` 返回上下文决定类型的 `Option(T)`，值为 None。

## Host 与 Entry

模块公开带 `@service::collection` 的 MainService struct，其字段分别实现
`std::transform_service::TransformService`。MIR 封闭所有方法实例；Host 准备声明的来源并初始化各字段服务，run 处理一个带 method/input 的 stdin JSON，
serve 处理 JSONL。每次调用从同一初始化状态开始，服务间隙 reset；fuel/memory 配额只
约束单次调用。来源与诊断细节见 [执行模式](guide/EXEC-MODE.md)。

## 命令行

当前命令面包括：

```text
telora eval <module:name>  求值 module 的一个 Value 导出
telora run <module>        读取一个 stdin JSON，输出 transform 的结果
telora run <module> --serve <URI>  通过 JSONL 或 HTTP 持续处理独立请求
telora build <module> -o app.wasm   编译为普通 Wasm 制品
telora build <module> --snapshot --source name=file.json -o app.wasm
                                  编译并嵌入可选的初始化快照
telora-run app.wasm [--serve <URI>]  独立执行制品，不需要源码和编译器
telora lock                物化 package source 并原子刷新 workspace lock
telora check <module-id>   类型闭合后完成可达模块图初始化
telora check --lib [--tests] [--only-types]  批量检查当前 crate
telora check --tests [--only-types]         批量检查当前 crate 的测试模块
telora test <name>         初始化 tests/<name>.telora，执行其直接导出的 Test
telora query ...           以 JSONL 查询模块和语义事实；别名 q
telora lsp                 启动语言服务器
```

包管理在私有 Host 内使用 IMOS 物化依赖，不向程序开放外部 I/O。
`eval` 读取普通 Value 导出；`run` 执行 MainService。

`--serve` 支持 `stdio+jsonl://`、`http://127.0.0.1:8080` 和
`http+unix:///tmp/telora.sock`。HTTP 路由由字段上的 `@http::get/post` 声明。
`telora-run` 不传 `--serve` 时从 stdin 读取一个完整 JSON，输出一个结果；
传入时持续服务。`--source name=file.json` 提供初始化数据，与请求输入分开。
普通 Wasm 制品每次启动 runner 都进行初始化。`build --snapshot` 在同一制品中保留
普通初始化代码和 ready service：runner 无 `--source` 时直接恢复快照，提供
`--source` 时忽略快照并以新来源重新初始化。当前不提供 Wasmtime 后端。
详细示例见 [执行模式](guide/EXEC-MODE.md)。

`query` 包含：

```text
telora query modules [-p pattern]
telora query exports <module-id> [-p pattern]
telora query at <module-id>[:line[:column]] [-p pattern] [-k kinds]
```

JSONL 位置默认使用 1-based line 和 0-based UTF-8 byte column；LSP 按协议协商位置
编码。`check` 不进行 Entry 调度，也不把导出函数当作入口调用；初始化计算可以调用函数。
纯导出由 `eval` 验收，应用 service 由严格 `run` 验收。调查静态错误时可使用
`check --only-types` 获取静态阶段的 JSONL 诊断。

`test parser/expressions` 支持嵌套测试入口。Host 为当前 crate 的整个 `tests/` 建立
临时来源访问边界，测试模块可以互相 `use`，但只求值从选中入口可达的模块。源码不能
反向引用测试。入口直接公开 `std::test::should_ok(fn() { ... })`、`should_fail`
或 `should_fail_with` 构造的 Test，支持 `with_fixtures` 批量生成用例。结果以
`telora.test/v2` JSONL 输出逐用例结果和汇总；`check` 不执行 Test。详见
[测试最佳实践](guide/TESTING.md)和 [CLI 指南](guide/TELORA-CLI.md)。

## 资源限制

Telora 允许递归，但每次执行受 fuel、栈、调用深度、分配和取消边界约束。资源耗尽
产生结构化失败，不发布部分结果。程序仍应表达自身算法需要的语义边界，不能把 Host
fuel 当作正常终止条件。

## 文档

- [guide/TELORA.md](guide/TELORA.md)：语言使用教程与当前限制。
- [guide/WORKSPACE.md](guide/WORKSPACE.md)：workspace、crate、模块树与依赖锁定。
- [guide/LIBSTD.md](guide/LIBSTD.md)：标准库模块定位与接口发现。
- [guide/TESTING.md](guide/TESTING.md)：契约断言、预期失败、fixtures 与测试分层。
- [guide/EXEC-MODE.md](guide/EXEC-MODE.md)：eval、run 与 serve 执行模式。
- [guide/TELORA-CLI.md](guide/TELORA-CLI.md)：CLI、工作区解析和 JSONL 契约。
- [docs/design/LANGUAGE.md](docs/design/LANGUAGE.md)：当前语言设计 SSOT。
- [docs/design/CONCEPT.md](docs/design/CONCEPT.md)：核心概念和所有权边界。
- [docs/design/IMPLEMENTATION.md](docs/design/IMPLEMENTATION.md)：当前实现架构与源码证据地图。
- [docs/MOTIVATION.md](docs/MOTIVATION.md)：问题域、动机与能力准入原则。
- [rfc/](rfc/)：设计决策的历史、方案与验收证据。
- [tree-sitter-telora/](tree-sitter-telora/)：Tree-sitter grammar。

## 验证

```bash
cargo test --workspace
cd tree-sitter-telora
npx tree-sitter test
```
