# Telora

> **TELORA Enables Lowering Objectives to Reliable Artifacts.**
>
> Telora 原名 Forma，最初名为 XL，完整设计演进记录在 [rfc/](rfc/) 中。

**Telora 是一门实验性语言：它在一个封闭、纯粹、确定且来源可追踪的世界中，提供可编程的数据转化与校验。**

它被设计为 Agent 与真实世界之间经过验证的意图语言：程序表达目标，库负责校验
与 lowering，Host 决定产生的可靠制品是否可以影响外部系统。

它试图回答：

> 一门语言最小需要什么，才能同时提供通用的数据计算、有限的执行边界，以及一等的诊断与反馈？

Telora 位于静态配置和通用脚本之间。静态格式容易检查，但表达力有限；脚本语言可以编程，却通常具有开放世界、效果、环境隐式输入，也难以解释转换后的数据究竟来自哪里。给脚本增加 fuel 和 Host API 白名单可以限制破坏，却不会自动得到权威的语义模型、跨数据来源追踪、容错分析或精确的编辑器反馈。

Telora 把这些诉求当作同一个设计问题。

## 核心模型

### 普通计算处理普通数据

配置、校验、规范化、迁移、编解码、schema 生成和计划构造都不是语言特性，而是普通纯函数对不可变数据的操作。

Telora 提供函数、闭包、递归、模式匹配、模块和较小的运行时数据模型。合并、默认值、优先级、编码等领域政策留在库中，可以被阅读、替换和组合。

### 封闭且有界的世界

模块路径静态可知，依赖预先固定，没有运行时 `eval`，真正的运行时输入只能通过 Host 显式提供的值进入。Telora、JSON、YAML 和 TOML 文件共同参与同一个不可变模块图。

Telora 允许递归，但每次执行都有独立的 fuel、栈、调用深度和分配配额。在配置的边界内，执行会确定地产生一个值或一个结构化资源错误。失败工作被原子丢弃，不会把半完成状态发布到持久世界。

### 诊断是一等公民

源码位置会跟随值穿过导入、转化、元数据和 codec 规范化。校验失败可以同时指出数据和拒绝它的规则：

```text
user.yaml:4:8: expected Int
  User.telora:3:10: requirement declared here
```

工作区中的 JSON、YAML 和 TOML 不是不透明的外部数据，而是一等源码模块。它们保留语法诊断和字段级来源，并参与依赖图与工作区分析。

即使 Telora 源码尚不完整，导航、类型和诊断仍应保留有用信息。语义事实明确区分已知值、显式 `Any`、未知、冲突、依赖阻塞和工具阶段不可计算。补全不会为了显得聪明而虚构结构。

反馈模型是语言实验的一部分，不是求值器完成后附加的编辑器功能。

### 类型是可编程元数据

类型声明求值为规范化的普通 Telora 数据：

```telora
def Maybe: for(A) Fn(TypeOf(A)) -> TypeOf(Option(A)) = fn(Item) {
    Option(Item)
};

type MaybeInt = Maybe(Int);
```

`Maybe` 是由程序代码所使用的同一个 VM 求值的普通纯函数。类型检查器解释它的结果，而不是在隐藏的类型层语言中重新实现这个函数。

同一份元数据可以驱动静态检查、LSP 信息、运行时校验、规范化、codec、文档、schema 生成和用户态解释器。`TypeOf(A)` 保留元数据见证与其所描述值之间的关系；受限的 `Dyn` 与 `interpreter!(...)` 边界允许异质解释，但不提供不安全 cast。

类型是 Telora 的核心机制，但它服务于更上位的目标：让数据规则可编程，并获得权威且来源可追踪的反馈。

## 效果属于 Host

Telora 本身没有影响外部世界的权限。Host 提供显式的普通输入，并决定某个普通输出值是否具有外部意义：

```text
外部世界
    -> Host 冻结输入快照
    -> 封闭的 Telora 计算
    -> 普通输出值
    -> Host 校验与授权
    -> 外部世界
```

Telora 不定义通用 action ABI。进程启动器、构建系统、Kubernetes controller 或 Agent runtime 各自定义类型，只解释自己认识的值。权限、IO、重试、事务、时钟和观察永远属于 Host。

`telora run`、`telora exec` 和 `telora build` 是具体的 Host adapter，不是语言级效果系统。当前 exec 和 build adapter 只校验并输出规范化计划，不执行计划所描述的效果。

## 这些原则带来了什么

### 没有语言魔法的 codec 与 schema

Decorator 是函数，attribute 是数据，codec 是元数据解释器：

```telora
import "std/json" as json;

@json.rename_all('CamelCase)
@struct
type User = {
    user_id: Int,
    @json.default('None)
    nickname: Option(String),
};
```

字段改名、默认值、扁平化和 skip policy 都是库定义的元数据。编码与解码共享一份计划，JSON Schema 也由同一份计划生成。

类型也可以声明文本解析规则。`Regex` 是标准库公开的 native type；正则在类型构造阶段编译，并校验命名 capture 与字段的完整对应关系：

```telora
import "std/regex" as re;
import "std/string" as string;

@re.parse_by(re.compile(r"(?P<name>\w+)=(?P<value>\d+)"))
@struct
type Rec = { name: String, value: Int };
```

`string.parse(Rec, "answer=42")` 的结果类型是 `Result(Rec, BlameError)`。类型是权威契约；正则只负责匹配并拆分经过验证的文本表示。捕获字段会递归使用同一个 `std/string.parse` 能力，因此嵌套的修饰 struct 类型可以自然组合，正则不需要拥有它们的转换逻辑。

反向转换使用独立的 `Display` capability，生成稳定、面向用户的文本：

```telora
import "std/fmt" as fmt;

@fmt.display_by("{host}:{port}")
@struct
type Endpoint = { host: String, port: Int };

fmt.display(Endpoint, { host: "localhost", port: 8080 })
```

模板在类型构造阶段完成校验和编译。字段替换会递归使用字段类型的 Display capability，因此嵌套的修饰 struct 可以自然组合，运行时不需要重新解析模板。面向诊断的 `Debug` 输出仍是另一项后续能力。

类型可以显式选择将这套文本表示作为结构化 codec 的容器表示：

```telora
@string.decode_by_parse
@string.encode_by_display
@fmt.display_by("{host}:{port}")
@re.parse_by(re.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
@struct
type Endpoint = { host: String, port: Int };
```

此后 `codec.decode` 接受 String，`codec.encode` 产生 String；即使 `Endpoint` 嵌套在其他结构中，JSON Schema 也会将它描述为 string。两个 bridge 声明必须成对，并且目前只作用于类型容器；field 级别的覆盖策略明确留待后续。

### 确定的执行与输出计划

模块没有默认返回值；它显式导出命名值，Host 再按运行模式选择入口。执行入口是一个普通函数：

```telora
export def exec: Fn(ExecSettings, ExecRequest) -> ExecEnv = fn(settings, request) {
    let plan = make_exec(settings, request);
    {
        ...plan,
        env: { clear: 'False, update: {} },
    }
};
```

Host 提供平台、下载与安装前缀、捕获的输入环境、参数和工作目录。捕获只决定 Telora 可以观察哪些变量，不会隐式转发给目标进程。Telora 显式返回 `{ clear, update }` 环境策略，并为每个安装动作计算下载文件与安装目录。Host 不计算缓存地址、不展开模板，也不重新解释政策。

`telora run` 读取命名导出 `output`；`telora exec` 调用 `exec`；`telora build` 调用 `build`。构建入口可写为 `export def build: Fn() -> build.OutputPlan = ...;`。adapter 校验规范化相对路径、拒绝重复目标并输出 canonical JSON。文本生成使用普通 String 与函数，而不是第二门模板语言。

### 静态数据也是源码

JSON、TOML 和 YAML 模块与 Telora 代码进入同一个不可变模块图。TOML 的时间类型保留为不同的 tagged representation。YAML 保守遵循 1.2 Core Schema：旧式隐式布尔值和时间戳仍是 String，mapping key 必须是 String，并拒绝 custom tag 与 merge key。存在歧义的格式行为会被拒绝，或交给显式库政策处理。

### 保守的局部多态

未标注、由闭包字面量初始化的 `let` 可以推导 rank-1 scheme：

```telora
let identity = fn(value) { value };
(identity(1), identity("text")) # (Int, String)
```

推断刻意保持有界。别名只实例化一次；递归组在没有显式契约时保持单态；数值约束不会被抹成无约束参数。Telora 宁可给出明确的未知或诊断，也不发布不稳定的精确信息。

## Agentic 系统

机器生成程序使 Telora 的约束更有价值。生成已经廉价，可信反馈和受控的外部意义仍然稀缺。

Telora 可以成为类型化、来源可追踪的 Plan IR。Agent 生成或修改一个纯程序；Telora 返回完整计划值；Host 可以在产生任何效果之前校验、比较、review、签名或拒绝它。计划中的 action 词汇仍是 Host 定义的普通数据。

Telora 也可以定义 Host 驱动 loop 的一个纯步骤：

```text
Context x State x Observation
    -> Result(LoopDecision(State, Plan, Output), BlameError)
```

Host 拥有观察、持久化、时间、效果、重试、审批和整个 loop 的预算。Telora 只计算一次确定且有限有界的状态转移。诊断可以同时指向 Agent 生成的 Telora、JSON/YAML/TOML 中的源值以及拒绝它的规则，由此形成精确的修复和审计闭环。

这些用途不需要 Agent 专用语法，也不会赋予 Telora 新的权限。

## 设计取舍

- **与 CUE 相比：**Telora 不以合一作为约束与组合的基础语义，政策是对数据的显式函数。
- **与 Dhall 相比：**两者都重视纯粹、可重放的计算；Dhall 保证规范化，Telora 允许递归，并提供确定的 fuel 与资源边界。
- **与 Starlark 相比：**两者都支持受控的宿主计算；Telora 还把可编程类型元数据、来源、部分语义事实和编辑器反馈纳入核心实验。
- **与 Nickel 相比：**Nickel 把契约、合并和优先级作为配置核心机制；Telora 把这些政策留在可替换的库中。
- **与沙箱脚本相比：**Telora 不仅有执行边界，还把静态数据、转化代码、规则、运行时校验和工具统一在一个来源可追踪的语义模型中。

Telora 不会消除复杂度。它试图把领域复杂度留在普通库和数据中，让可信的语言语义保持较小且一致。

## 当前边界

Telora 仍是实验品。它没有语言级效果、环境隐式 IO、动态导入、通用包获取、trait 或类型收窄。Host 可以提供狭窄 adapter，但效果并不是一项等待加入语言的功能。

项目已经证明了核心纵向路径，包括可计算和递归类型元数据、派生 codec 与 schema、容错工作区语义、语言服务器、有界 rank-1 推断、安全动态观察，以及用户态参考 Equality 和 Show 解释器。它尚未证明生产规模 Host、长期兼容性或广泛外部使用。

可能的应用包括可复用配置包、构建与工具链计划、持续调谐、政策驱动的数据管道、类型化 Agent Plan，以及 Host 驱动的 Agent Loop。

## 试一试

```sh
cargo run -p telora -- check examples/mvp/main.telora
cargo run -p telora -- run examples/mvp/external.telora --input examples/mvp/request.json
cargo run -p telora -- show examples/mvp/main.telora
cargo run -p telora -- lsp
```

## 文档

- [docs/README.md](docs/README.md)：当前设计 SSOT 的入口、文档地图与维护规则
- [docs/MOTIVATION.md](docs/MOTIVATION.md)：MRT 问题域，以及 Telora 为什么是
  lowering intent 和 harness agent 的语言
- [docs/design/LANGUAGE.md](docs/design/LANGUAGE.md)：当前完整语言设计的 SSOT
- [docs/design/CONCEPT.md](docs/design/CONCEPT.md)：权威的核心概念、所有权边界与
  依赖方向
- [INTRO.zh.md](INTRO.zh.md)：从问题域、既有方案和 GCC wrapper 案例介绍 Telora
- [VISION.md](VISION.md)：设计命题与功能准入原则
- [tutorial.md](tutorial.md)：当前公开语言表面的实用教程
- [rfc/](rfc/)：单项决策的历史与实现验收证据
- [README.md](README.md)：English

## 验证

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
