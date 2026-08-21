# Telora CLI 指南

每个 Telora crate 的可复用模块位于 `src/`，应用入口位于 `src/bin/`，测试入口位于
`tests/`。`telora-deps.json` 声明该 crate 的依赖边界。

Telora 从当前目录向上查找最近的 `telora-deps.json`，因此命令可以从目标 crate 根
目录或其任意子目录执行。`-C` 可以显式改变查找的起始目录，该目录不必是 crate 根。
命令参数使用稳定逻辑模块 ID，不使用物理文件名：

```text
telora run main -C examples/my-crate
telora run verify -C examples/my-crate
telora check @test/compiler.telora -C examples/my-crate
telora query modules -C examples/my-crate
telora query at @bin/main.telora -C examples/my-crate
telora query at @src/compiler.telora -C examples/my-crate -k type,let,def,import
telora query exports @src/compiler.telora -C examples/my-crate
telora query at @src/compiler.telora:12:3 -C examples/my-crate
telora query exports std/string -C examples/my-crate
telora query at std/array -C examples/my-crate -p flat_map
telora run -S path/to/file.telora
telora run-with @src/tool.entry.telora main -C examples/my-crate -- argument
telora run invalid -C examples/my-crate --best-effort
```

`check` 的输入仍是完整 Module，不是任意表达式 scratch。模块顶层使用 `def` 声明
计算根并至少显式 export 一项；顶层 `let`、裸调用和 final expression 均不合法。
需要局部步骤时把它们放进 `do`：

```telora
export def lowering_case = do {
    let plan = lower(request);
    validate_plan.must_ok!(plan)
};
```

多个独立检查应写成多个具名 export，使 best-effort `check` 可以继续不依赖失败项的根。

- `run name` 完全等价于 `run-with std/entry/default name`。默认 Entry 要求 Main 的
  export record 含 String `output`，并把它作为 `Output(String)` effect 发给 Host。
  `name` 是不含路径分隔符和 `.telora` 后缀的单个 stem。
- `run name -C context` 从 `context` 开始向上发现 manifest。
- `run ... --best-effort` 只在遇到问题时用于扩大诊断覆盖。它在启动 Entry 前对 Main 做
  best-effort 诊断求值；只要出现任何 error，stderr 输出 `telora.run/v1` JSONL 诊断与
  error summary，非零退出且不产生任何 Entry effect，即使一个不依赖失败的干净根值仍能
  算出。没有 error 时仍重新走严格 Entry/Host lifecycle；成功结果的最终验收使用普通
  `run`。本参数用于调查问题时扩大诊断覆盖。
- `run-with <entry-module> ...` 由 Host 显式选择纯 Edge Entry。自定义 Entry 的
  `MainType` 和输出编码由它自己规定，可以通过 stdio-child effects 编排进程。Entry
  使用稳定模块 selector，例如 `@src/tool.entry.telora`；文件名必须以
  `.entry.telora` 结尾。它不能作为普通模块根，也不能被普通模块 import。
- `run-with` 中 `--` 后的参数按顺序进入 `Env.args`。Main 顶层的 `option` action 按
  源码顺序进入 `SystemOptions`。Entry 的 `config(options, env)` 返回 capability 诉求
  和 initializer；Host 提供的私有 native 在 Entry WorkWorld 内按诉求构造
  `SystemResources`，并由 runtime 直接传给 initializer，再由 initializer 把环境数据显式
  传给 Main。该资源值不会返回 Host 侧解码或重建。CLI 不提供 `--input`；Entry 负责解析参数，并通过 `SystemCaps` 请求具名
  JSON/YAML/TOML 数据源、文本源、环境变量、stdin 模式和 child-process 权限。文件
  诉求可用 `default: 'Some(value)` 容忍数据文件不存在，该默认值是 `Value`，不会按
  文件格式再解析；文本文件默认值仍是 String。已有但无法解析的数据文件仍报错。
  JSON/YAML/TOML import 与 `data_srcs` 共用同一管线：先完成 CST 和全部格式级验证，
  再把通用 `Value` 直接物化进目标 Heap；前者进入构建中的 MainWorld，后者进入 Entry
  WorkWorld。该过程不经过 `DataWorld` 或 Host-owned Telora value。业务 schema 校验属于
  codec，不属于资源读取。物理文件使用有界读取；文件大小、节点总数、深度、单容器大小、
  单 Bytes/String 长度和解码 payload 总字节数由独立 data limits 控制，不消耗或借用 VM
  的 fuel、stack、allocation quota。
  不存在的环境变量从 `SystemResources.vars` 省略，非字符串环境值报错。
  `stdin: 'Text` 注入完整文本；`'Lined` 在 `Initialize` 后逐行产生 `StdinLine`，并以
  单个 `None` 表示 EOF。`spawn_child` 必须为真才能发出 `SpawnStdioChild` 或
  `PostStdin`。
- `run -S file` 进入 standalone 模式，不发现 manifest，只使用根文件内的
  `crate.dependency` / `crate.format` options；这些 options 相对文件父目录解析。
  `-S` 与 binary name、`-C` 互斥。
- `run`、`check` 和 `query` 的 `-C context` 都从 `context` 开始向上发现 manifest；
  `check` 和 `query` 接受完整稳定模块 ID，`check @test/...` 检查测试入口，`run` 只
  接受 binary name。
- `check` 用 best-effort 模式继续彼此独立的求值，以一次收集更多诊断；最终判定仍然
  严格。stdout 完全采用 `telora.check/v1` JSONL：先输出诊断 records，最后输出一条
  `summary` record。只有完整求值并形成内部 semantic Module graph 时 summary 才是
  `status: "ok"`；它不把递归 TypeMetadata 等内部图物化为 legacy Host value。任何
  语法、类型、解析或运行时失败都会得到 `status: "error"` 和非零退出。
  最终应用验收仍以 `run` 为准，因为 `run` 还经过 Entry 调度。
- `query`（可见别名 `q`）输出 `telora.query/v1` JSONL 语义记录。`query modules`
  列出当前 crate 可见的规范模块 ID；`query exports <module>` 查询公共接口；
  `query at <module>` 查询顶层 local definitions，追加 `:<line>` 或 `:<line>:<column>`
  查询与源码行或位置相交的事实。它查询 recoverable CST 和部分语义/求值证据图，因此
  在模块损坏时仍可返回不受影响的事实；命令成功只表示查询完成，不表示模块能够通过
  `check` 或 `run`。
- 本 crate 的 Entry 会以 `visibility: "entry"` 出现在 `query modules`；它仍只能由
  `run-with` 选择，不能交给 `check`、`query exports` 或普通 import。
- `query at std/...` 和 `query exports std/...` 直接查询 Host 注册的公开标准库逻辑模块，与源码 `import "std/..."`
  使用同一模块身份；不需要也不能把 `std` 声明成 workspace dependency。
- `-p` 按名称的大小写敏感字面子串过滤，不是 glob 或正则。
- `query at <module> -k` 接受逗号分隔的 `type,let,def,import`；公共接口使用独立的
  `query exports` 子命令查询。
- Namespace import 的记录用 `target` 给出目标模块 ID，不带普通值 `type`；用
  `query exports <target>` 查询其成员的精确 type/scheme。Selective import 的记录
  直接携带所选成员的精确 type/scheme。
- `query at <module>:<line>[:<column>]` 的行号从 1 开始，列号从 0 开始并按 UTF-8
  byte 计数；输出范围同样采用 1-based line、0-based UTF-8 column 的半开区间。
  带行号的位置查询不接受 `-p` 或 `-k`。

程序中的 `dbg!(expr)` 和 `expr.dbg!()` 把旁路观察写入 stderr，不改变 stdout 的
`output`。每个事件是一行紧凑 JSON：

```json
{"name":"var","repr":"3","module":"@bin/main.telora","line":12}
{"name":"plan","repr":"{...}","module":"@bin/main.telora","line":13,"message":"generated"}
```

固定字段为 `name`、`repr`、`module`、`line`；只有显式 message 时才有 `message`。
`repr` 是有界 debug 表示，不是可反序列化的 JSON 值。Host 是否输出或丢弃事件对
Telora 程序不可感知。Float 的 `repr` 使用 Debug 表示，例如 `3.0` 和 `-0.0`；它与
字符串插值和 `fmt.display` 使用的 Display 表示不同。

命令退出码为零表示请求成功；非零表示 CLI 或 Telora 拒绝。`query` 的空匹配成功且
没有输出。记录中的 `authority` 区分 `authoritative`、`recovery` 与 `debug` 事实。
表达式级记录属于 `debug`；错误恢复记录的 authority 服从其事实和模块状态。
