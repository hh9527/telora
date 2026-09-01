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
    app.telora
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
  "modules": ["@src/app"],
  "dependencies": []
}
```

创建或刷新 lock：

```bash
telora lock
telora check @src/app
telora query modules
```

`telora-lock.json` 由 `telora lock` 生成。其他 crate-mode 命令只验证和消费 lock。

## 三份文件的职责

| 文件 | 所有权 | 内容 |
| --- | --- | --- |
| `telora-config.json` | workspace 作者 | member、远程 source 和开发 override |
| `telora-crate.json` | crate 作者 | crate 名、普通模块清单和直接依赖名称 |
| `telora-lock.json` | `telora lock` | 完整、精确且排序稳定的 package graph |

workspace 中同一个 crate name 只有一个来源。crate 依赖只写名称；config 为名称选择
workspace member 或远程 tarball；lock 固定选择结果、模块清单和依赖边。

Telora 不进行语义版本求解，也不在同一 workspace 中安装同名 crate 的多个版本或多个
来源。crate 依赖图必须无环。

## Workspace Config

`telora-config.json` 的完整顶层结构为：

```json
{
  "version": 1,
  "members": ["app", "query"],
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
  "modules": [
    "@src/main",
    "@src/model",
    "@src/schema/request.json"
  ],
  "dependencies": ["query"]
}
```

### `name`

`name` 是 crate 的规范身份，也是其他 crate import 路径的首段。名称使用 ASCII 字母、
数字和 `-`，不包含 `/`、`.` 或 `\\`，也不以 `_` 或 `@` 开头。

### `modules`

`modules` 是 `src/` 下普通模块的权威清单。每一项必须以 `@src/` 开头：

```text
@src/main                -> src/main.telora
@src/model/user          -> src/model/user.telora
@src/schema/request.json -> src/schema/request.json
@src/schema/rules.yaml   -> src/schema/rules.yaml
@src/config.toml         -> src/config.toml
```

Telora selector 省略且不能写 `.telora`。JSON、YAML、YML 和 TOML 静态数据模块保留格式
后缀。模块文件名只能包含一个已知后缀；以 `.` 开头的文件不构成模块。

清单中的文件必须存在并保持在 crate root 内。存在于 `src/`、但不在清单中的 Telora 或
静态数据文件不会进入 resolver；`telora check` 会为它们产生 warning。

`src/` 内的目录只组织逻辑路径，不赋予文件特殊执行身份。程序通过导出的名义类型选择
执行模式，例如 `entry.Eval`、`entry.Run(State)` 或 `entry.Serve(State)`。

### `dependencies`

`dependencies` 只列直接依赖的 crate name：

```json
{
  "dependencies": ["query", "domain-model"]
}
```

被引用的名称必须出现在 workspace members 或 config sources 中。一个 crate 只能 import
自己的模块和直接依赖；传递依赖不会自动成为可见依赖。

## Module Identity 与 Import

模块身份由 crate name 和逻辑路径组成，不包含 workspace、下载缓存或用户目录的物理
路径：

```text
当前 crate 的 @src/model -> app/model
依赖 query 的 @src/types -> query/types
内置标准库              -> std/...
测试 @test/compiler      -> app/tests/compiler
```

当前 crate 内推荐使用 root selector 或相对 selector：

```telora
import "@src/model" {User};
import "./helpers" {normalize};
import "../schema/request.json" {data as request_schema};
```

依赖模块使用 crate name 作为首段：

```telora
import "query/types" {Query};
import "std/array" as array;
```

`@src/` 始终相对于 importing module 所属 crate。依赖自身源码中的 `@src/types` 仍指向
该依赖的 `src/types.telora`。`./` 和 `../` 按 importing module 的逻辑目录解析，不能
越过 crate source root。

module import graph 在求值前封闭，并且不允许初始化 cycle。需要递归时，在单个模块或
已经建立的模块接口内使用语言的递归函数和递归 TypeMetadata。

## Public 与 Private Module

文件 stem 以 `_` 开头的模块是 crate-private：

```text
src/_internal.telora
src/model/_lowering.telora
```

同 crate 模块可以 import private module；依赖方只能看到公开模块。`telora query modules`
会列出当前 crate 的 public/private 模块、直接依赖的 public 模块和公开 builtin 模块。

私有性属于整个模块，不改变模块内 export 的含义。公开模块仍需显式 export 向依赖方
承诺的类型和值。

## Static Data Module

声明在 `modules` 中的 JSON、YAML 和 TOML 文件是静态数据模块：

```json
{
  "modules": ["@src/config/defaults.json"]
}
```

```telora
import "@src/config/defaults.json" {data as defaults};
```

静态数据模块统一导出 `data: std/value.Value`。Host 在构造封闭模块图时加载数据并保留
字段 provenance；这不是运行时文件 I/O。格式或字段导致的后续诊断可以引用稳定模块
来源和 authored location。

## Test Root

测试文件位于 crate 的 `tests/`，由 Host 以 `@test/NAME` 选择：

```text
@test/compiler -> tests/compiler.telora
```

当前 test root 只支持 `tests/` 下的单个文件，不支持嵌套目录，也不进入
`telora-crate.json.modules`。test 使用 `@src/...` 导入所属 crate 的普通模块；test 中的
相对 import 不构成 crate module 依赖。

```bash
telora check @test/compiler
```

## Workspace Lock

`telora-lock.json` 固定 workspace 中每个 package 的 source、module 清单和直接依赖：

```json
{
  "version": 1,
  "packages": {
    "app": {
      "source": {"workspace": "app"},
      "modules": ["@src/main"],
      "dependencies": ["query"]
    },
    "query": {
      "source": {
        "tarball": "https://packages.example/query-r17.tar.gz"
      },
      "modules": ["@src/lib"],
      "dependencies": []
    }
  }
}
```

lock 是完整 package graph，而不是单个 crate 的依赖片段。模块和依赖数组使用确定顺序。
workspace config、member manifest 或远程基线发生变化后运行：

```bash
telora lock
```

`eval`、`eval-with`、`run`、`serve`、`check`、`query` 和 LSP 都要求 lock 存在且与当前
config、manifest 和远程物化结果一致。发现陈旧 lock 时，命令会要求刷新，不会隐式
改写它。

## Resolver 顺序

resolver 在加载模块前冻结 crate 清单：

1. builtin vendor 提供 `std` crate；
2. 当前 crate 提供自身模块；
3. manifest 中的直接依赖提供各自公开模块。

选择以整个 crate 为颗粒，并采用 first-win。一个来源已经提供某个 crate name 后，后续
来源不能补充或覆盖其中的 module；配置中的 `std` 也不能改变内置 `std`。

## 常用工作流

创建或修改 workspace package graph：

```bash
telora lock
telora query modules
```

检查 crate 模块和未声明文件：

```bash
telora -C app check @src/main
```

发现依赖接口和本地定义：

```bash
telora -C app query exports query/types
telora -C app query at @src/main
```

验收公开执行值：

```bash
telora -C app eval @src/model:schema
telora -C app eval-with @src/compiler:compile --source request=request.json
telora -C app run @src/service:run
telora -C app serve @src/service:serve --bind stdio://
```
