# Telora Workspace 与 Crate 指南

Telora workspace 把一组 crate、依赖来源和精确 lock 组织成可重复准备的模块世界。每条
CLI 命令先发现 workspace，再准备完整 package graph，最后解析命令选中的模块。

本文面向 workspace 维护者和 crate 作者。语言与模块语法见 [`TELORA.md`](TELORA.md)，
执行入口见 [`EXEC-MODE.md`](EXEC-MODE.md)，完整命令参数见
[`TELORA-CLI.md`](TELORA-CLI.md)。

## 最小 workspace

一个最小 workspace 可以同时是一个 crate：

```text
hello/
  telora-config.json
  telora-crate.json
  telora-lock.json
  src/
    lib.telora
```

`telora-config.json` 定义 workspace package 来源：

```json
{
  "version": 1,
  "members": ["."]
}
```

`telora-crate.json` 定义当前 crate：

```json
{
  "name": "hello",
  "dependencies": []
}
```

创建或刷新 lock：

```bash
telora lock
telora check --lib
telora query modules
```

`telora-lock.json` 由 `telora lock` 生成。其他 crate-mode 命令只验证和消费 lock。

## 三份文件的职责

| 文件 | 所有权 | 内容 |
| --- | --- | --- |
| `telora-config.json` | workspace 作者 | member、远程 source 和开发 override |
| `telora-crate.json` | crate 作者 | crate 名和直接依赖名称 |
| `telora-lock.json` | `telora lock` | 完整、精确且排序稳定的 package graph |

workspace 中同一个 crate name 只有一个来源。crate 依赖只写名称；config 为名称选择
workspace member 或远程 tarball；lock 固定选择结果和依赖边。模块树不写入任何配置或
lock 文件，而是从每个 crate 的 `src/lib.telora` 开始按 `mod` 声明发现。

这三个 JSON 文件的正式属性名统一使用 camelCase。`sources`、`overrides` 和
`packages` 中的键是 crate 名称，遵循 crate 命名规则，不进行大小写或连字符转换。
命令行参数仍使用 kebab-case。

Telora 不进行语义版本求解，也不在同一 workspace 中安装同名 crate 的多个版本或多个
来源。crate 依赖图必须无环。

## Workspace Config

`telora-config.json` 的顶层结构如下；`compiler` 和 `runtime` 可省略，示例列出默认值：

```json
{
  "version": 1,
  "members": ["app", "query"],
  "compiler": {
    "maxTypeDepth": 256,
    "maxTupleItems": 1024,
    "maxTypeArguments": 4096
  },
  "runtime": {
    "fuel": 100,
    "memoryLimit": 1024
  },
  "sources": {
    "codec-lib": {
      "tarball": "https://packages.example/codec-lib-r17.tar.gz"
    }
  },
  "overrides": {
    "codec-lib": {
      "path": "vendor/codec-lib"
    }
  }
}
```

### `compiler` 与 `runtime`

两个对象及各自字段都可省略，省略的字段独立采用内置默认值。字段必须为正整数，
未知字段、错误类型及不可表示的数值会导致配置诊断。

| 字段 | 默认值 | 含义 |
| --- | --- | --- |
| `compiler.maxTypeDepth` | 256 | 归一化类型结构深度，叶节点深度为 1；不沿名义类型成员布局递归计数 |
| `compiler.maxTupleItems` | 1024 | 单个 Tuple 的最大单元数 |
| `compiler.maxTypeArguments` | 4096 | 单个类型节点的直接子类型数量，包括 Tuple、Record、类型列表和函数签名（含返回类型）；不是实例总数 |
| `runtime.fuel` | 100 | 会话 fuel，1 表示 1,000,000 fuel |
| `runtime.memoryLimit` | 1024 | Wasm 线性内存上限，单位 MiB：1 表示 `1 << 20` 字节（16 个 64 KiB 页）；不是进程 RSS |

编译限制只在此配置，没有对应 CLI 参数。整个静态求解 session 使用 workspace 根的
配置，依赖 crate 不能覆盖；CLI 和 LSP 共用该设置。直接处理标准库模块时也读取
当前 workspace 配置，没有 workspace 时才使用内置默认值。

运行时限制按字段分别采用 **CLI 显式参数 > workspace 配置 > 内置默认值**：
`--with-fuel N` 覆盖 `runtime.fuel`，`--with-memory-limit N` 覆盖 `runtime.memoryLimit`。
显式传入默认数值也属于覆盖。`check`/`test` 的初始化与用例在同一会话内共享预算，
`run`/`serve` 每个请求在 reset 后获得独立预算；普通 `check` 使用
运行时配置，`check --only-types` 不创建 VM。

这两类配置不写入 `telora-lock.json`，调整限制不需要重新生成 lock。超出编译限制
会诊断对应的配置路径与当前上限，不能 seal；这不代表已经证明程序无限展开。

### `members`

`members` 至少包含一个相对于 workspace root 的目录。每个目录中必须存在
`telora-crate.json`，并且所有 member 的 crate name 唯一。

workspace root 就是 `telora-config.json` 所在目录。CLI 从 `-C` 指定位置或当前目录向上
查找最近的 config，因此可以在任意 member 子目录中执行命令：

```bash
telora -C app check @src/main
```

这里 `@src/...` 相对于包含 `-C` 位置的 member crate，而不是相对于 workspace root。

### `sources`

`sources` 把非 member crate name 映射到远程 source。当前 source 是 HTTP(S) `.tar.gz`
URL：

```json
{
  "sources": {
    "query": {
      "tarball": "https://packages.example/query-2026-09-01.tar.gz"
    }
  }
}
```

URL 应指向内容不变的归档。归档解开后必须满足以下一种布局：

```text
telora-crate.json
src/...
```

或者只包含一个 crate 根目录：

```text
query/
  telora-crate.json
  src/...
```

物化后的 manifest `name` 必须与 `sources` 中的 key 相同。Telora 的 package Host 负责
下载、复用不可变 installation，并在 workspace 的 `.telora/crates-refs/` 下维护物化引用。
物理缓存位置不参与 crate 或 module 身份。

### `overrides`

`overrides` 为已经声明的远程 source 选择 workspace 内的研发目录：

```json
{
  "sources": {
    "query": {
      "tarball": "https://packages.example/query-r17.tar.gz"
    }
  },
  "overrides": {
    "query": {
      "path": "vendor/query"
    }
  }
}
```

override path 必须位于 workspace root 内，manifest name 必须匹配远程 crate name，直接
依赖集合必须与 lock 一致。`telora lock` 根据远程基线生成 lock；普通命令在远程基线和
lock 验证通过后使用 override 目录作为有效源码根。

override 不创建新的 package 身份。移除 override 后，同一个 crate/module identity 会
重新由 lock 指定的远程内容提供。

## Crate Manifest

每个 crate 根目录包含一个 `telora-crate.json`：

```json
{
  "name": "app",
  "dependencies": ["query"]
}
```

### `name`

`name` 是 crate 的规范身份，也是其他 crate 静态路径的首段。名称使用 ASCII 字母、
数字和 `-`，不包含 `/`、`.` 或 `\\`，也不以 `_` 或 `@` 开头。

每个 crate 必须提供 `src/lib.telora`。它是唯一固定入口，使用 `mod` 或 `pub mod` 挂载
其他源码模块；未从根可达的文件不属于模块图。manifest 中不存在 `modules` 字段。
服务 crate 通常从根模块公开实现 TransformService 的具体类型 MainService。

### `dependencies`

`dependencies` 只列直接依赖的 crate name：

```json
{
  "dependencies": ["query", "domain-model"]
}
```

被引用的名称必须出现在 workspace members 或 config sources 中。一个 crate 只能引用
自己的模块和直接依赖；传递依赖不会自动成为可见依赖。

## Module Identity 与发现

模块身份由 crate name 和逻辑路径组成，不包含 workspace、下载缓存或用户目录的物理
路径：

```text
当前 crate 的 src/lib.telora       -> app
当前 crate 的 src/model.telora     -> app/model
当前 crate 的 src/model/user.telora -> app/model/user
依赖 query 的 @src/types -> query/types
内置标准库              -> std/...
测试 @test/compiler      -> app/tests/compiler
```

父模块先声明子模块，再通过静态路径绑定名字：

```telora
pub mod model;
mod helpers;
use crate::model::{User};
use self::helpers::{normalize};
data request_schema = import(json) "../schema/request.json";
```

依赖模块使用 crate name 作为首段：

```telora
use query::types::{Query};
use std::array as array;
```

`use` 只绑定已经存在的模块或公开成员，不读取文件，也不增加模块图边。`mod child;`
从当前逻辑目录挂载 `child.telora`；位于根模块时对应 `src/child.telora`。嵌套模块采用
同样规则，例如 `src/model.telora` 中的 `mod user;` 对应
`src/model/user.telora`。模块发现不能越过 crate 的 `src/` 根。

module graph 在求值前封闭，并且不允许初始化 cycle。需要递归时，在单个模块或
已经建立的模块接口内使用语言的递归函数和递归 TypeMetadata。

## Public 与 Private Module

`mod child;` 挂载私有子模块，`pub mod child;` 同时把它加入父模块公开接口。模块中的
声明默认私有；`pub def`、`pub type`、`pub trait` 及 `pub use` 明确形成
公共接口。以下划线开头不再决定语义可见性。`telora query modules` 会列出当前 crate
已发现的 public/private 模块、直接依赖中已发现的 public 模块和公开 builtin 模块。

## Static Data Module

```telora
data defaults = import(json) "config/defaults.json";
```

`data` 声明创建静态数据模块边，并在当前模块绑定一个 `std::value::Value`。路径相对
声明它的源码模块；格式由 `import(json|yaml|toml)` 显式决定，扩展名不参与类型身份。
Host 在构造封闭模块图时加载数据并保留字段 provenance；这不是运行时文件 I/O。

## Test Root

本节描述测试模块身份与可见性；如何组织行为断言、预期失败和数据输入，见
[测试最佳实践](TESTING.md)。

测试文件位于 crate 的 `tests/`，使用 `telora test NAME` 选择一个入口，也可以继续用
`telora check @test/NAME`：

```text
@test/compiler -> tests/compiler.telora
@test/parser/expressions -> tests/parser/expressions.telora
```

选中测试时，Host 递归枚举当前 crate 的 `tests/`，为测试入口和辅助文件建立本次调用
的访问边界。它不写入 manifest 或 lock，也不加入普通 `query modules` 的结果。
符号链接（包括 `tests/` 根目录）不允许进入测试边界。其他后缀文件被忽略；未引用
模块的源码不会被解析或求值。
`tests/x.telora` 与已声明的 `src/tests/x.telora` 会产生相同的 canonical name，
测试准备阶段会拒绝这种命名冲突。

测试通过所属 crate 的公开模块树访问普通模块，通过 `@test/...`、
`<crate>/tests/...` 或相对路径访问测试模块。顶层测试和子目录辅助模块可以相互引用，
所有合法拼写保持同一模块身份。相对路径不能越出测试清单；源码模块和其他 crate
不能反向访问测试模块。循环依赖仍产生错误。

访问边界中的文件不会自动执行。Host 只从选中根发现完整可达图，再严格初始化。
被导入测试的错误会使本次测试失败；未引用测试的语法或求值错误不会影响本次
结果。入口必须是非 private 的 Telora 模块，名称不带 `.telora`，可以包含子目录。
当前必须显式指定一个名称，不提供无参数批量发现。初始化无错误后，`test` 按公开
公开名称执行入口直接公开的 `std::test::Test`；普通 `use` 不执行依赖模块的 Test。
`check` 和 query 不执行 Test。fixture 由 Host 作为数据源准备，不形成模块边；
其路径相对构造 `with_fixtures` 的模块，并限制在该模块所属 crate 内。
API、失败规则和输出协议见 [CLI 指南](TELORA-CLI.md)。

```bash
telora test compiler
telora test parser/expressions
telora check @test/compiler
```

## Workspace Lock

`telora-lock.json` 固定 workspace 中每个 package 的 source 和直接依赖：

```json
{
  "version": 1,
  "packages": {
    "app": {
      "source": {"workspace": "app"},
      "dependencies": ["query"]
    },
    "query": {
      "source": {
        "tarball": "https://packages.example/query-r17.tar.gz"
      },
      "dependencies": []
    }
  }
}
```

lock 是完整 package graph，而不是单个 crate 的依赖片段。依赖数组使用确定顺序；模块
不进入 lock，从相应发布内容的 `src/lib.telora` 发现。
workspace config、member manifest 或远程基线发生变化后运行：

```bash
telora lock
```

`eval`、`run`、`serve`、`test`、`check`、`query` 和 LSP 都要求 lock 存在且与当前
config、manifest 和远程物化结果一致。发现陈旧 lock 时，命令会要求刷新，不会隐式
改写它。

## Resolver 顺序

resolver 在加载模块前冻结 package graph：

1. builtin vendor 提供以 `std/lib.telora` 为根的 `std` crate；
2. 当前 crate 以自身 `src/lib.telora` 为根；
3. manifest 中的直接依赖各自以 `src/lib.telora` 为根。

选择以整个 crate 为颗粒，并采用 first-win。一个来源已经提供某个 crate name 后，后续
来源不能补充或覆盖其中的 module tree；配置中的 `std` 也不能改变内置 `std`。

## 常用工作流

创建或修改 workspace package graph：

```bash
telora lock
telora query modules
```

检查 crate 的可达模块树：

```bash
telora -C app check --lib
```

发现依赖接口和本地定义：

```bash
telora -C app query exports query/types
telora -C app query at @src/main
```

验收公开执行值：

```bash
telora -C app eval @src/model:schema
telora -C app run @src/compiler < request.json
telora -C app run @src/service:run
telora -C app serve @src/service --bind stdio+jsonl://
```
