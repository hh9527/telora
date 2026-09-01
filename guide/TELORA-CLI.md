# Telora CLI 指南

Telora CLI 及其运行时适配器共同充当运行时宿主（Host）：它们准备输入、执行 Entry
效果并呈现诊断。

每个 Telora crate 的模块位于 `src/`，测试位于 `tests/`。`telora-crate.json` 声明
canonical crate name、模块清单和直接依赖名称；workspace 根的 `telora-config.json`
选择这些名称的唯一来源，`telora-lock.json` 固定完整包图。

Telora 从当前目录向上查找最近的 `telora-config.json`，因此命令可以从 workspace 内
任意目录执行。`-C` 可以显式改变查找的起始目录。`telora lock` 是唯一写入 lock 的
命令；`eval`、`eval-with`、`run`、`serve`、`check`、`query` 和 LSP 要求 lock 已存在且与配置一致。
命令参数使用稳定逻辑模块 ID，不使用物理文件名：

```text
telora -C examples/my-crate eval @src/model:answer
telora -C examples/my-crate eval-with @src/model:evaluate --source request=request.json -- arg
telora -C examples/my-crate run @src/app:run
telora -C examples/my-crate run @src/app:run --source request=stdin+json://
telora -C examples/my-crate run @src/app:run --ees-var tenant=production
telora -C examples/my-crate serve @src/app:serve --bind stdio://
telora -C examples/my-crate check @test/compiler
telora -C examples/my-crate query modules
telora -C examples/my-crate query at @src/app
telora -C examples/my-crate query at @src/compiler -k type,let,def,import
telora -C examples/my-crate query exports @src/compiler
telora -C examples/my-crate query at @src/compiler:12:3
telora -C examples/my-crate query exports std/string
telora -C examples/my-crate query at std/array -p flat_map
telora -C examples/my-crate run @src/invalid:run --best-effort
telora -C examples/my-crate lock
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

- `eval module:name` 要求公开导出 `name: Value`，直接求值并编码为 JSON。
  `eval-with` 要求导出 `entry.Eval`。其 `entry.ContextConfig` 声明 source、环境变量和
  参数能力；两条命令都不进入 reducer/effect loop。
- `run module:name` 和 `serve module:name --bind stdio://` 分别要求导出
  `entry.Run(State)` 和 `entry.Serve(State)`。`run` 投递一次 Request 并在 Reply 后输出
  Value；`serve` 持续把每行 JSON 转成 Request，每个 Reply 产生一行
  JSON 响应。成功响应是 `{"ok": value, "error": false, "diagnostics": [...]}`；请求
  触发可恢复 failure 时是 `{"ok": null, "error": true, "diagnostics": [...]}`，服务
  继续处理下一行。当前诊断 JSON 只稳定公开 `message`。资源耗尽、取消等终止性失败，
  以及初始化和 Entry 协议错误仍带外报告并终止进程。
- `run` 和 `serve` 在 `ees.Config` 中声明命名 native model，并以 `ees.Config.vars` 与
  `--ees-var` 绑定 locator 变量。model 由 `std/ees.imos_model` 或
  `std/ees.sqlite_model` 构造。
  reducer 发出 `actor.EesCall`，Host 完成调用后投递 `actor.EesReply`；多步行为的阶段
  和关联信息保存在显式 State。应用 EES 与 package Host 的私有 IMOS Service 完全隔离。
- `entry.ContextConfig.sources` 声明初始化 source。声明的名称必须全部提供，CLI 也不能
  提供未声明或重复的名称。
  `--source name=path.json` 按 `.json/.yaml/.yml/.toml` 推断格式；
  `file+json://path`、`file+yaml://path`、`file+toml://path` 显式指定 transport 与格式；
  `stdin+json://`、`stdin+yaml://`、`stdin+toml://` 从标准输入读取一次。
  单次命令最多声明一个 stdin source。`serve --bind stdio://` 已把 stdin 用作 JSONL
  请求通道，因此不能再用 stdin 初始化 source。
  Main 收到的 Dict key 就是 `name`。值的诊断来源固定为 `@run-ctx/name`，不会暴露文件
  路径；该来源名不是模块，不能 import，也不会由 `query modules` 列出。
- `telora -C context run module:name` 从 `context` 开始向上发现 workspace config，并以包含
  `context` 的 member crate 解析 module selector。
- `run ... --best-effort` 只在遇到问题时用于扩大诊断覆盖。它在启动 Entry 前对 Main 做
  best-effort 诊断求值；只要出现任何 error，stderr 输出 `telora.run/v1` JSONL 诊断与
  error summary，非零退出且不产生任何 Entry effect，即使一个不依赖失败的干净根值仍能
  算出。没有 error 时仍重新走严格 Entry/运行时 lifecycle；成功结果的最终验收使用普通
  `run`。本参数用于调查问题时扩大诊断覆盖。
- `run`、`check` 和 `query` 的 `-C context` 都从 `context` 开始向上发现 workspace；
  `check` 和 `query` 接受完整稳定模块 ID，`check @test/...` 检查测试入口；`run` 和
  `serve` 接受 `MODULE:EXPORT`。
- `check` 用 best-effort 模式继续彼此独立的求值，以一次收集更多诊断；最终判定仍然
  严格。stdout 完全采用 `telora.check/v1` JSONL：先输出诊断 records，最后输出一条
  `summary` record。只有完整求值并形成内部 semantic Module graph 时 summary 才是
  `status: "ok"`；它不把递归 TypeMetadata 等内部图物化为外部 owned value。任何
  语法、类型、解析或运行时失败都会得到 `status: "error"` 和非零退出。
  纯导出以 `eval` / `eval-with` 验收；应用 service 仍以 `run` 为准，因为 `run` 还经过
  Entry 和 reducer/effect 调度。
- `query`（可见别名 `q`）输出 `telora.query/v1` JSONL 语义记录。`query modules`
  列出当前 crate 可见的规范模块 ID；`query exports <module>` 查询公共接口；
  `query at <module>` 查询顶层 local definitions，追加 `:<line>` 或 `:<line>:<column>`
  查询与源码行或位置相交的事实。它查询 recoverable CST 和部分语义/求值证据图，因此
  在模块损坏时仍可返回不受影响的事实；命令成功只表示查询完成，不表示模块能够通过
  `check` 或 `run`。
- `query modules` 列出本 crate 的 public/private source、dependency 的 public source
  和 public built-in；test 与 private built-in 不进入 catalog。
- `query at std/...` 和 `query exports std/...` 直接查询内置标准库模块，与源码
  `import "std/..."` 使用同一模块身份。resolver 在图发现前按 crate 粒度建立 first-win
  清单，builtin `std` 先于 workspace 配置；后序同名 dependency 不能补充或改写它。
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
{"name":"var","repr":"3","module":"@src/app","line":12}
{"name":"plan","repr":"{...}","module":"@src/app","line":13,"message":"generated"}
```

固定字段为 `name`、`repr`、`module`、`line`；只有显式 message 时才有 `message`。
`repr` 是有界 debug 表示，不是可反序列化的 JSON 值。运行时适配器是否输出或丢弃事件对
Telora 程序不可感知。Float 的 `repr` 使用 Debug 表示，例如 `3.0` 和 `-0.0`；它与
字符串插值和 `fmt.display` 使用的 Display 表示不同。

命令退出码为零表示请求成功；非零表示 CLI 或 Telora 拒绝。`query` 的空匹配成功且
没有输出。记录中的 `authority` 区分 `authoritative`、`recovery` 与 `debug` 事实。
表达式级记录属于 `debug`；错误恢复记录的 authority 服从其事实和模块状态。
