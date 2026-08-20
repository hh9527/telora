# GCC wrapper：增强型 dotslash 的应用层思想实验

- Stage: Discussion
- Scope: application-level design probe
- Non-goal: implementing download, unpack, cache reuse, or process execution

## 问题

设想一个可以直接执行的 GCC wrapper：

```sh
./gcc-wrapper --target=aarch64-linux-gnu -c hello.c -o hello.o
```

它看起来像一个 dotslash 入口，但实际需求比“下载一个二进制并启动”更丰富：

1. 一个入口需要多个包。GCC 是 Host 平台相关的可执行工具，sysroot 是 TARGET 相关的目标平台运行时，两者应当独立下载、安装和缓存。
2. wrapper 需要改写命令行。它要选择并注入 sysroot，添加确定的 source/debug prefix map，并在必要时补充搜索路径。
3. 下载和改写逻辑需要复用。`gcc`、`g++`、`ar` 等入口共享同一套 GCC 安装，只在命令和参数政策上不同。
4. 错误必须可以诊断。未知 Host 平台、未知 TARGET、缺失环境输入、错误 digest 或不合法的最终执行计划，都应指出输入和拒绝它的规则。
5. 所有纯计算应在效果发生前完成。dry-run 应当展示 Host 将要执行的完整 `ExecEnv`，而不是一份仍需替换变量或解释模板的半成品。

抽象以后，这个 wrapper 是一个普通的数据程序：

```text
ExecSettings x ExecRequest x ToolName
    -> ExecEnv
```

Telora 负责右侧值的确定计算，Host 负责下载、校验、解包和启动进程。

## 目标目录

研发阶段可以先使用多个入口文件，共享一个普通模块：

```text
telora-deps.json
src/toolchain.telora
bin-src/gcc.telora
bin-src/g++.telora
bin-src/ar.telora
```

不需要额外设计“把整个依赖闭包物理合并为一个文件”的发布能力。顶层入口只要
能携带静态依赖选项，就可以保持很薄；依赖仍然是具有稳定身份的普通模块。
当前实现尚不支持下面的完整形态，但它可以作为这条路线的最终验收代码：

```telora
#!/usr/bin/env -S telora exec --dry-run --

option "crate.dependency" {
    name: "gcc-toolchain-define",
    source: 'Path({path: "../gcc-toolchain-define"}),
};
option "crate.dependency" {
    name: "gcc-wrapper",
    source: 'Path({path: "../gcc-wrapper"}),
};
option "exec.capture-envs" ["TARGET"];

import "std/rt-types/exec.telora" { ExecFn };
import "gcc-toolchain-define/source.json" as source;
import "gcc-wrapper/toolchain.telora" { wrap_gcc };

export def exec: ExecFn = wrap_gcc(source);
```

这里的 `option "crate.dependency"` 是 Host/resolver 消费的静态模块选项，不是运行
期求值产生的依赖，也不把下载能力交给 Telora 程序。它只包含立即数，锁定仓库
和 revision；resolver 获取并注册模块以后，后面的 import 仍然服从普通的确定
解析规则。`std/rt-types/exec.telora` 不属于这张远程依赖表：它是普通的内置
runtime protocol 类型模块。import 只取得协议；用户选择 `telora exec` 时，Host
才赋予 `ExecFn` 导出以执行意义。

这段入口也把职责切得很清楚：

- `gcc-toolchain-define` 提供纯数据 `source`，描述 GCC 与 sysroot 的来源；
- `gcc-wrapper/toolchain` 提供可复用的参数改写与计划生成函数；
- `std/rt-types/exec.telora.ExecFn` 定义 `telora exec` Host 接受的稳定入口协议；
- 顶层文件只装配依赖并导出 `exec`，没有隐式模板替换或构建 DSL。

当前对应能力分别是 `crate.dependency` option、`telora-deps.json` 中的 path dependency，以及
显式的 `Fn(ExecSettings, ExecRequest) -> ExecEnv`。尚待推进的是静态 `option`
表面语法、Path dependency、runtime protocol 类型模块和 `ExecFn`
类型名。
这个思想实验先用当前可检查的展开代码验证应用层计算，再用上述短入口约束
resolver 与发布体验的路线。

## 共享工具链模块

下面的代码使用当前的 import/export 语法、普通类型、函数、模式匹配和
`std/rt-types/exec.telora` 数据协议。URL 和 digest 是示例值，不代表真实发行地址。RFC 0158
已经用缩减的 `Fn(String) -> ExecFn` 形状验证跨模块闭包、模块 helper、混合
捕获和互递归；完整示例仍应在最终端到端 fixture 中单独验收。

```telora
# src/toolchain.telora

import "std/array" as arrays;
import "std/argv" as argv;
import "std/dict" as dicts;
import "std/rt-types/exec.telora" as exec_types;
import "std/hash" as hash;

type ExecSettings = exec_types.ExecSettings;
type ExecRequest = exec_types.ExecRequest;
type ExecEnv = exec_types.ExecEnv;

@struct
type Package = {
    name: String,
    src: String,
    digest: String,
};

def install_dest = fn(settings, package) {
    let identity = `\{package.name}\n\{package.src}\n\{package.digest}`;
    `\{settings.install_prefix}/\{hash.sha256(identity)}`
};

def unpack = fn(settings, package) {
    'Unpack({
        dest: install_dest(settings, package),
        ty: 'TarGzip,
        src: package.src,
        strip: 1,
        digest: 'Some(package.digest),
    })
};

def compiler_package = fn(platform) {
    let host = `\{platform.os}-\{platform.arch}`;
    match host {
        "linux-x86_64" => {
            name: "gcc-14.2.0-linux-x86_64",
            src: "https://toolchains.example/gcc-14.2.0-linux-x86_64.tar.gz",
            digest: "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        },
        "linux-aarch64" => {
            name: "gcc-14.2.0-linux-aarch64",
            src: "https://toolchains.example/gcc-14.2.0-linux-aarch64.tar.gz",
            digest: "sha256:2222222222222222222222222222222222222222222222222222222222222222",
        },
        unsupported => panic!(`unsupported GCC host platform: \{unsupported}`),
    }
};

def sysroot_package = fn(target) {
    match target {
        "x86_64-linux-gnu" => {
            name: "sysroot-x86_64-linux-gnu-v1",
            src: "https://toolchains.example/sysroot-x86_64-linux-gnu-v1.tar.gz",
            digest: "sha256:3333333333333333333333333333333333333333333333333333333333333333",
        },
        "aarch64-linux-gnu" => {
            name: "sysroot-aarch64-linux-gnu-v1",
            src: "https://toolchains.example/sysroot-aarch64-linux-gnu-v1.tar.gz",
            digest: "sha256:4444444444444444444444444444444444444444444444444444444444444444",
        },
        unsupported => panic!(`unsupported GCC target: \{unsupported}`),
    }
};

def compiler_args = fn(request, sysroot_dest) {
    let arguments = match argv.reject_option(request.args, "--sysroot") {
        'Ok(arguments) => arguments,
        'Err(error) => panic!(error.message),
    };
    let arguments = match argv.reject_option(arguments, "-ffile-prefix-map") {
        'Ok(arguments) => arguments,
        'Err(error) => panic!(error.message),
    };
    let arguments = match argv.reject_option(arguments, "-fdebug-prefix-map") {
        'Ok(arguments) => arguments,
        'Err(error) => panic!(error.message),
    };
    argv.prepend(
        [
            `--sysroot=\{sysroot_dest}`,
            `-ffile-prefix-map=\{request.cwd}=.`,
            `-fdebug-prefix-map=\{request.cwd}=.`,
        ],
        arguments,
    )
};

def command_args = fn(tool, request, sysroot_dest) {
    match tool {
        "gcc" => compiler_args(request, sysroot_dest),
        "g++" => compiler_args(request, sysroot_dest),
        "ar" => request.args,
        unsupported => panic!(`unsupported GCC tool: \{unsupported}`),
    }
};

def installs_for = fn(tool, compiler_install, sysroot_install) {
    match tool {
        "gcc" => [compiler_install, sysroot_install],
        "g++" => [compiler_install, sysroot_install],
        "ar" => [compiler_install],
        unsupported => panic!(`unsupported GCC tool: \{unsupported}`),
    }
};

export def command:
    Fn(String) -> Fn(ExecSettings, ExecRequest) -> ExecEnv =
    fn(tool) {
        fn(settings, request) {
            let target = match dicts.get(request.env, "TARGET") {
                'Some(target) => target,
                'None => panic!("TARGET is required by the GCC wrapper"),
            };
            let compiler = compiler_package(settings.platform);
            let sysroot = sysroot_package(target);
            let compiler_dest = install_dest(settings, compiler);
            let sysroot_dest = install_dest(settings, sysroot);
            let compiler_install = unpack(settings, compiler);
            let sysroot_install = unpack(settings, sysroot);

            {
                install: installs_for(tool, compiler_install, sysroot_install),
                cwd: 'Some(request.cwd),
                bin: `\{compiler_dest}/bin/\{tool}`,
                args: command_args(tool, request, sysroot_dest),
                env: dicts.merge(request.env, {
                    GCC_EXEC_PREFIX: `\{compiler_dest}/lib/gcc/`,
                }),
            }
        }
    };
```

这里有几个刻意的设计：

- `compiler_package` 只依赖 Host platform；`sysroot_package` 只依赖 TARGET。两种选择逻辑彼此独立。
- 安装位置由 package 的稳定身份计算，而不是临时目录或下载时序决定。
- source map 在 Telora 中根据显式 `cwd` 计算，Host 不再补做字符串替换。
- `ar` 复用 GCC 包，但不下载 sysroot，也不注入编译参数。
- `command(tool)` 返回符合 Host 协议的普通函数；共享逻辑不需要 VM 或 CLI 知道“工具链”这个领域概念。

## 薄入口

使用当前 workspace 布局时，三个命令入口只负责选择共享模块导出的工具函数：

```telora
# bin-src/gcc.telora
#!/usr/bin/env -S telora exec --dry-run

import "@src/toolchain.telora" as toolchain;

export def exec = toolchain.command("gcc");
```

```telora
# bin-src/g++.telora
#!/usr/bin/env -S telora exec --dry-run

import "@src/toolchain.telora" as toolchain;

export def exec = toolchain.command("g++");
```

```telora
# bin-src/ar.telora
#!/usr/bin/env -S telora exec --dry-run

import "@src/toolchain.telora" as toolchain;

export def exec = toolchain.command("ar");
```

`@src/toolchain.telora` 是当前工作 crate 内的绝对模块请求；它取代了早期设计稿
中的 `crate:...` 写法。`bin-src` 中的入口不是可检索的 `@src` 模块。

目标中的复用单位是普通模块和函数。顶层静态依赖选项足以描述发布单元，不
要求把依赖源码合并进入口文件。是否提供 multicall 文件或多个符号链接，是
工具与 Host 的决定，不应改变应用逻辑。RFC 0158 的生产回归已经验证这一
跨模块高阶调用形状；完整 wrapper 的 canonical dry-run 留给 RFC 0157 的最终
验收。

## 调用与计划

概念上的调用方式是：

```sh
TARGET=aarch64-linux-gnu \
  telora exec --dry-run bin-src/gcc.telora -- \
  -c /workspace/src/hello.c -o /workspace/out/hello.o
```

完整端到端 fixture 应产生类似下面的 canonical plan。路径 hash 在此缩写：

```json
{
  "install": [
    {
      "Unpack": {
        "dest": "/cache/telora/exec/installs/7c...",
        "ty": "TarGzip",
        "src": "https://toolchains.example/gcc-14.2.0-linux-x86_64.tar.gz",
        "strip": 1,
        "digest": {
          "Some": "sha256:1111..."
        }
      }
    },
    {
      "Unpack": {
        "dest": "/cache/telora/exec/installs/a2...",
        "ty": "TarGzip",
        "src": "https://toolchains.example/sysroot-aarch64-linux-gnu-v1.tar.gz",
        "strip": 1,
        "digest": {
          "Some": "sha256:4444..."
        }
      }
    }
  ],
  "cwd": { "Some": "/workspace" },
  "bin": "/cache/telora/exec/installs/7c.../bin/gcc",
  "args": [
    "--sysroot=/cache/telora/exec/installs/a2...",
    "-ffile-prefix-map=/workspace=.",
    "-fdebug-prefix-map=/workspace=.",
    "-c",
    "/workspace/src/hello.c",
    "-o",
    "/workspace/out/hello.o"
  ],
  "env": {
    "TARGET": "aarch64-linux-gnu",
    "GCC_EXEC_PREFIX": "/cache/telora/exec/installs/7c.../lib/gcc/"
  }
}
```

这个值已经没有待展开模板。Host 可以在下载之前显示、比较、签名或拒绝它，也可以按 `install` 顺序准备资源，最后使用 `bin`、`args`、`env` 和 `cwd` 启动进程。

## 诊断预期

### 未知 TARGET

如果输入为：

```sh
TARGET=arm64-linux telora exec --dry-run bin-src/gcc.telora -- -c hello.c
```

最小可接受结果是错误落在 `sysroot_package` 的拒绝分支。更理想的诊断应当同时保留 Host 输入与规则位置：

```text
host input TARGET: unsupported GCC target "arm64-linux"
  src/toolchain.telora: target set declared here
```

当前 Telora 已经能保留普通源码规则位置，也具备外部输入与 blame 的基础模型；Host 环境字段的精细来源呈现仍需要专门验收。

### 缺失 TARGET

第一版代码直接读取 `request.env.TARGET`。缺失字段会失败，但应用更希望显式表达：

```text
TARGET is required by the gcc wrapper
```

这暴露了一个标准库表达缺口：`Dict(String)` 需要类型保持的 `get`/`require` 能力，应用才能在不依赖动态字段错误的情况下返回自己的 `BlameError`。

### 错误的最终计划

即使应用代码错误地返回：

```telora
{
    install: [],
    cwd: 'Some(42),
    bin: "gcc",
    args: [],
    env: {},
}
```

Host adapter 仍应拒绝它，因为 `cwd` 不符合 `ExecEnv`。这形成两层边界：Telora 类型检查负责应用内部契约，Host 在赋予外部意义前再次验证协议值。

## 今天已经可以表达的部分

基于当前实现，不计算真实外部效果，下面这些能力已经具备：

| 能力 | 当前状态 | 说明 |
|---|---|---|
| 一个入口返回多个安装动作 | 已具备 | `ExecEnv.install: Array(Install)` |
| GCC 与 sysroot 分开选择 | 表达已具备 | 普通函数和模式匹配可通过检查 |
| 根据 TARGET 选择 sysroot | 表达已具备 | TARGET 可由显式 `ExecRequest.env` 提供；缺失字段的诊断仍较底层 |
| 确定的安装位置 | 表达已具备 | `hash.sha256` 与显式 `install_prefix` |
| command line rewrite | 表达已具备 | Array combinator、concat 与 String interpolation |
| 确定性 source/debug map | 表达已具备 | `cwd` 是显式输入，最终参数是具体 String |
| gcc/g++/ar 共享实现 | 基础已验证 | RFC 0158 覆盖 namespace import、高阶闭包、模块 helper 与 ExecFn 形状 |
| 最终计划类型约束 | 已具备 | `Fn(ExecSettings, ExecRequest) -> ExecEnv` |
| Host 边界再次校验 | 已具备 | `telora exec --dry-run` 校验并输出 canonical JSON |
| 有界纯求值 | 已具备 | fuel、栈、调用深度和分配配额 |
| 规则位置诊断 | 已具备 | runtime/type diagnostics 保留 Telora 来源 |

这说明核心应用逻辑距离当前 Telora 并不远。多资源计划、平台选择和参数改写
已经能够由普通应用代码表达；完整远程依赖入口的 dry-run 尚未成立，仍不能
用局部回归代替端到端能力证据。

## 仍然存在的缺口

### 1. 跨模块导出闭包的回归边界

当前代码可以通过：

```sh
telora check bin-src/gcc.telora
```

早期临时审计曾在调用 `toolchain.command("gcc")` 时观察到
`up-link read operand is not an up-link`，但临时 fixture 删除后无法复现。RFC
0158 增加的生产回归覆盖 namespace import、直接与高阶导出、模块 helper、
混合捕获、导入 TypeMetadata、`ExecFn` 形状和互递归，均能正确执行。

因此当前没有证据支持修改 VM 闭包表示。该问题转化为一条持续回归边界：完整
wrapper 若再次失败，必须保留并缩减具体 fixture，再按实际根因处理，不能默认
归因于跨模块 up-link。

### 2. `Dict` 的安全读取接口

当前示例使用：

```telora
let target = request.env.TARGET;
```

更合适的应用接口应当是类型保持的组合子：

```telora
dict.get(request.env, "TARGET") # Option(String)
```

再配合 `Result`/`Option` 的传播或一个 `require` helper，应用可以产生领域化错误，而不是依赖底层字段访问失败。

### 3. ExecRequest 的结构化调用上下文

入口通过 `option "exec.capture-envs" ["TARGET"];` 显式声明 Host 输入。Host
只把声明过且实际存在的环境变量写入 `ExecRequest.env`；未声明变量不能影响
求值、dry-run 输出或未来的缓存键。TARGET 因而仍是 GCC wrapper 的领域输入，
不需要进入所有命令共享的稳定协议字段；wrapper 继续从 `env` 中解析并验证它。

### 4. 参数解析与改写工具

数组拼接已经足以注入前置参数，但完整 GCC wrapper 还会需要：

- 识别并拒绝用户提供的冲突 `--sysroot`；
- 改写 `-I`、`-L` 与 response file；
- 区分输入路径和普通参数；
- 确保 source-prefix-map 不被后续参数覆盖；
- 对 `gcc`、`g++`、`ar` 和 linker 参数采用不同政策。

这些首先应当是 Telora 标准库或应用库中的 argv parser/rewriter，而不是语言语法。现有 `array.fold_control`、模式匹配和不可变数组已经提供了算法基础，缺少的是成熟的组合 API。

### 5. 包描述的名义与校验能力

`UnpackOpt.digest` 当前只是 `Option(String)`。真实工具希望在计划进入 Host 前检查：

- digest algorithm 与长度；
- URL scheme；
- `strip` 的合法范围；
- package identity 是否覆盖所有影响安装结果的 action；
- 相同 `dest` 是否描述相同安装动作。

Parse、Display、codec 和 validator 已经提供构建这些领域类型的路径，但 `std/rt-types/exec.telora` 还没有把它们产品化为更强的 package 类型。

### 6. 静态依赖入口

不需要额外的源码打包机制。需要补齐的是让顶层入口直接表达开发阶段由
`telora-deps.json` 承担的依赖约束：

- `option "crate.dependency"` 只能包含 Host 可静态读取的立即数；
- 当前阶段使用 `Path` 固定依赖图；远程发布以后再由 pinned provider 补上；
- resolver 取得依赖 crate root 后，再执行普通 import 解析；
- 依赖名与包内路径需要确定、无歧义的映射；
- `std/rt-types/exec.telora` 只描述协议，只有 `telora exec` Host 解释 `ExecFn`
  导出；
- 同一 wrapper 模块可以被 gcc/g++/ar 三个薄入口复用。

这些是 resolver 和 Host dependency boundary 的工作，不应改变 wrapper 的纯函数
主体。`telora exec URL`、源码物理合并和独立 packager 都不是这个思想实验的
前置条件。

### 7. 真正的效果执行

当前 `telora exec --dry-run` 只验证并打印计划。真实 Host 还需要实现：

- 按 URL 与 digest 获取内容；
- 原子解包和安装；
- 跨入口复用 cache 与 install result；
- 并发安装去重；
- 权限、超时、代理和离线政策；
- 最终进程替换或子进程管理。

这些明确不属于 Telora 语言能力。本思想实验评估的是：在不计算外部工具的情况下，Telora 能否产生足够完整、确定且可诊断的效果计划。

### 8. Host 输入的来源诊断

Telora 已经能追踪静态文件和规则来源，但 wrapper 还需要验证环境变量、用户 argv 与规则位置之间的双向诊断。特别是 command line rewrite 之后，错误应能区分：

- 用户原始参数；
- wrapper 注入的参数；
- 选择这些参数的规则；
- Host 最终拒绝计划的位置。

这与 Telora 的 provenance/blame 方向一致，但需要针对 ExecRequest 和计划适配器补充端到端验收。

## 距离判断

如果把“外部工具不算”作为边界，这个 GCC wrapper 的数据模型和单入口计算
主体与跨模块复用基础已经可达，但目标入口仍需后续 RFC 完成：

```text
多个包
  + Host/TARGET 选择
  + 确定安装路径
  + 命令行改写
  + 多入口复用
  + typed ExecEnv
  + dry-run
```

当前距离主要在三类应用基础设施：

1. `Dict`/argv/path 等标准库组合能力；
2. Host 输入与改写结果的精细 provenance；
3. 顶层静态依赖选项与真实 exec adapter；远程 dependency provider 留给发布阶段。

这对 Telora 是一个有价值的信号：不需要为了 GCC wrapper 引入 effect system、trait、可变状态或专用构建语法。更合理的推进方式是保持应用主体不变，逐步补齐标准库和 Host 协议。

## 对 Telora 介绍文档的价值

这个例子可以成为 Telora intro 的主线，因为它把抽象理念投射到了一个具体需求：

```text
静态包数据
    -> 类型与领域校验
    -> 普通函数选择 Host/TARGET 资源
    -> 纯 command line rewrite
    -> 可复用模块
    -> 完整 ExecEnv
    -> Host dry-run、授权和效果
```

随后可以用同一个模型解释其他场景：

- build rule 把 `ExecEnv` 换成产物 DAG 或 `OutputPlan`；
- Helm chart 把最终计划换成经过 schema 与来源校验的 Kubernetes 数据；
- Agentic Plan IR 把 wrapper 输入换成 Agent 生成的意图，Host 仍然只执行经过验证的完整计划。

它们共享的不是某项语法，而是 Telora 的核心边界：在封闭、纯粹、有界且来源可追踪的世界中完成所有确定计算，只把具体、可验证的计划交给外部世界。
