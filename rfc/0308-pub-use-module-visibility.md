# RFC 0308：`pub` / `use` 模块可见性

- 状态：已实现
- 跟踪：[#222](https://github.com/hh9527/telora/issues/222)
- 分支：`feat/rfc-0308-pub-use-modules`
- 日期：2026-09-22
- 前置：RFC 0306（静态模块挂载与 `::` 路径）
- 修订：以 `pub` / `use` 完整取代模块级 `import` / `export`

## 摘要

Telora 的代码模块只使用 `mod` 构造模块树、`use` 建立静态名字绑定、`pub` 表达跨模块
可见性。`pub use` 是唯一的 re-export 形式。旧模块级 `import` / `export` 语法直接删除，
不保留兼容双轨。

`data` 声明中的：

```telora
data config = import(json) "config.json";
```

保持不变。这里的 `import(format)` 是构建期数据装载语法，不建立名字绑定、不加载代码模块，
也不是普通 Call expression。

每个 crate 固定以 `src/lib.telora` 为根。内置 `std` 使用同一规则，以
`crates/telora-core/modules/std/lib.telora` 为根；该文件中的 `pub mod` 定义公开标准库
模块树。Rust 侧 embedded source 表只负责打包字节，不定义可达性或可见性。

## 动机

RFC 0306 已引入 `mod`、`use` 和 `::`，但迁移期仍保留旧 `import` / `export`。这使模块发现、
名字绑定与可见性呈现两套表面规则：源码通过 `mod` 挂载，却仍可能通过字符串 `import`
引用；静态名字通过 `use` 引入，却仍通过独立 `export { ... }` 列表公开。

统一语法后，每个结构只有一个职责：

```text
mod       挂载代码子模块
use       在当前静态作用域建立名字绑定
pub       允许声明越过模块边界
pub use   建立公开静态别名或 re-export
import    仅存在于 data 声明的构建期数据装载语法
```

## 规范语法

声明默认私有。以下模块级声明可以带 `pub`：

```telora
pub decl value: Int;
pub def answer: Int = 42;
pub native length: Fn(String) -> Int;
pub native type BlameError @1;
pub type User = struct { name: String };
pub trait Encode { encode: Fn(Self) -> Value };
pub mod query;
pub data config = import(json) "config.json";
```

Decorator 位于可见性之前：

```telora
@json::rename_all(json::RenameCase::CamelCase)
pub type User = struct { display_name: String };
```

`pub` 只允许在模块顶层。`let`、解构绑定、表达式和 `impl` 不接受 `pub`。impl 的参与范围由
trait、目标类型、模块可达性与 coherence 决定，不由值符号可见性决定。

## `use` 与 re-export

`use` 不读取文件，也不改变模块图：

```telora
use std::codec;
use std::value::{Value, ScalarValue};
use crate::model::User as DomainUser;
```

`pub use` 先在当前模块建立同一个静态别名，再把该别名公开：

```telora
pub use crate::model::User;
pub use crate::service::DefaultService as MainService;
pub use std::value::{Value, ScalarValue};
pub use Bool::{True, False};
```

选择器中的 `as` 只改变当前/公开名称，不改变目标 SymbolId。`pub use` 可以重新公开当前模块
已经可访问的身份，但不能绕过目标模块的可见性边界访问私有名字。

模块本身也服从可见性。`mod query;` 只允许父模块内部访问 `query`；依赖方只有在父模块写出
`pub mod query;` 或公开其内部身份时才能沿该模块路径访问。

## 旧语法映射

```telora
import "std/codec" as codec;
import "std/value" { Value };
export def answer = 42;
export { User };
export { Service as MainService };
export Bool.{True, False};
```

迁移为：

```telora
use std::codec;
use std::value::Value;
pub def answer: Int = 42;
pub type User = ...;
pub use self::Service as MainService;
pub use Bool::{True, False};
```

本 RFC 不提供弃用期。旧模块级 `import` / `export` 在 parser 层不再形成合法 binding；由语法
恢复产生的节点也不得被 lowering 当成可执行兼容路径。

## MIR 与执行边界

`pub` 只改变静态可见性，不触发求值。HIR lowering 可以继续用内部 public/export marker
构造模块公开记录，但 marker 必须来自：

- 声明上的 `pub`；或
- `pub use` 产生的公开别名。

Sealed MIR 仍只让实际 execution roots 及其依赖进入可执行闭包。`pub def` 在 `check --lib`
中是检查根，但不会因此让所有公开值在 `run` / `serve` 时被无条件求值。

## 数据装载例外

以下语法完整保留：

```telora
data config = import(json) "config.json";
```

其中 `import`：

- 只能出现在 `data` 声明的 `=` 右侧；
- format 仍是 `json` / `yaml` / `toml` 静态语法；
- 不产生 Import binding、ModuleId 或作用域别名；
- 不允许通过 `use` 引入、覆盖或调用。

本阶段 `data name: Type = ...` 可被 parser 识别，但 lowering 明确报告尚未支持；无类型
注解的声明固定绑定 `std::value::Value`。类型化静态数据留给后续 RFC。

公开数据写为：

```telora
pub data config = import(json) "config.json";
```

## 迁移计划

1. tree-sitter 增加 `pub` 修饰符和单路径 `as`，删除 export/import binding grammar；
2. CST/AST 增加可见性读取，删除旧 ExportBinding / ImportBinding；
3. HIR 从 `pub` 声明和 `pub use` 生成公开 marker；
4. module/symbol resolve 强制私有边界，并保持 `pub mod` 语义；
5. 迁移 std、fixtures、示例、正式文档和 lab-ontology；
6. 删除旧 token/rule/lowering、字符串路径 import 与相关恢复逻辑；
7. 验证 tree-sitter、core、Wasm、language acceptance 和真实 ontology 模型。

## 验收条件

- 合法 `.telora` 代码中不存在模块级 `import` / `export`；
- `data ... = import(format) "..."` 保持合法且语义不变；
- 私有声明不能从其他模块或依赖 crate 访问；
- `pub` 声明可通过确定的静态路径访问；
- `use path as Alias` 与 `use path::{Name as Alias}` 身份一致；
- `pub use` 可公开直接路径、选择器、alias 和 enum variant；
- `pub mod` 与私有 `mod` 的跨模块行为不同且有精确诊断；
- `pub` 在局部 binding、`let` 和 `impl` 上被拒绝；
- 未挂载文件仍不能由 `use` 隐式加载；
- std、全部语言 fixtures、示例和 lab-ontology 完成一次性迁移；
- tree-sitter corpus、`telora-core`、`telora-wasm`、language acceptance 与真实模型通过；
- 正式文档只描述 `pub` / `use`，RFC 0306 标注由本 RFC 修订。

## 放弃的方案

### 永久保留 `export { ... }`

这会让声明可见性继续依赖远处列表，也让 `pub use` 与 export alias 重复。

### 让 `import` 成为 `use` 的别名

双语法会继续混淆代码名字绑定与 data 构建期装载，且无法为宏提供单一静态路径模型。

### 将 data 的 `import(format)` 改名

该语法已经具有清晰、封闭的构建期含义，不参与模块系统。本 RFC 有意保留它，避免为了词面
统一破坏已经稳定的数据声明。
