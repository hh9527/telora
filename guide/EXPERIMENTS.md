# Telora 实验指南

本文说明如何在 Telora 主仓库中准备、启动和主持基于 OpenCode 的多角色实验。它描述
通用基础设施和 Host 工作流；每个实验的研究问题、角色隔离、artifact DAG 和验收标准，
仍以对应计划目录中的 `README.md`、`EVAL-METHOD.md` 和 `experiment.json` 为准。

## 核心模型

实验基础设施包含四个彼此分离的对象：

- **plan**：`experiment-plans/<plan-id>/` 下由 Telora 仓直接跟踪的平台无关方案；
- **lab**：一个长期运行的 OpenCode 无头服务器及其临时根目录；
- **execution session**：由无空格 title 唯一标识的一次运行，例如 `ontology-3/1`；
- **workspace**：位于 lab root 下、从 plan 声明资产构造的实际工作目录。

实验方案是 Telora 研发资产，和语言代码一起评审、提交。独立实验仓只保存某次运行的
输入快照、Agent 产出、诊断、指标和最终状态，不作为 `oc-ctl start` 的输入：

```text
experiment-plans/ontology-3              平台无关方案 SSOT
<lab-root>/executions/ontology-3/1/      execution 状态和隔离运行环境
独立实验结果仓                          一次执行的结果快照
```

方案定义角色目标、可见性、允许命令和 artifact DAG，但不提交 `opencode.json`、
`.opencode/agents` 或 OpenCode prompts。控制面在准备 workspace 时确定性生成这些 adapter；
因此未来可以增加其他 Agent runtime，而不修改实验问题和 DAG。

基础设施的所有权边界是：

```text
coordinator  只一次性启动各个长期角色
role agent   循环 pull -> 完成一个任务 -> submit，只发布 .<role> artifact
Host         观察、审核、投递文件，只发布无角色后缀 artifact
oc-task      根据 artifact 时间戳计算角色的下一个任务
```

角色没有工作或 `pull` 超时后不能退出，而应继续 pull。Host 不直接向角色派发任务；
反馈正文通过文件投递，反馈生效通过 Host-owned artifact 发布。

## 启动前检查

本机需要可以直接运行 `git`、`cargo`、Python 3 和 `opencode`。可以分别确认外部 lab
和 Host 控制程序可用：

```bash
./oc-lab --help
./oc-ctl --help
python3 -m unittest tools.opencode_experiment.tests.test_control \
  tools.opencode_experiment.tests.test_task_cli
```

这里执行 `oc-lab --help` 只是安装可用性检查，不是 Host 权限检查。`oc-lab run` 始终由外部
操作员在外部终端运行，不属于 Host 的命令权限或实验调度能力。

开始前阅读实验计划并确认：

1. `experiment-plans/<plan-id>` 的所有输入已经提交，目录没有未提交修改；
2. `experiment.json` 使用平台无关 schema，角色、workspace 资产和 artifact 正确；
3. plan 的 `EVAL-METHOD.md` 明确研究问题、隔离边界、人工验收点和归因方法；
4. release Telora 可以构建，计划引用的教程和 CLI 文档是当前版本；
5. lab 名称尚未被另一个运行中的 `oc-lab` 使用。

如果 Host 运行在有命令授权机制的环境中，必须在 `start` 前一次性取得整个实验所需的
权限。至少包括 `./oc-ctl` 全部子命令、状态观察、实验计划中列出的本地验证命令，以及
必要的外部写入；不包括外部操作员使用的 `oc-lab`。启动后不能临时申请授权，让审批
等待阻塞调度。

长程任务开始时，应在人仍可处理授权时执行一次连接门禁：

```bash
./oc-ctl test-connect t1
```

该命令连接由 `oc-lab` 建立的无头 OpenCode daemon，实际调用 health 和 session HTTP API。
探针 session 使用 `connect/<generation>` title，成功凭据写入 lab root。它不选择 plan、
不准备正式 execution workspace，也不会影响 `oc-lab` 生命周期。授权时必须持久批准整个 `./oc-ctl` 命令前缀，
只批准 `test-connect` 的具体 argv 不能覆盖后续 `start/status/update/publish`。

同一个 lab 可以承载不同 workspace 的多个 execution session：

```bash
./oc-ctl test-connect t1
./oc-ctl start t1 query-service-1 \
  --bundle target/exp-outputs/08-26
```

这只共享无头 daemon 和端口，不共享 workspace、session、artifact 或 execution state。

需要向 GitHub Issue 汇报时，还要提前确认 `gh issue comment --body-file ...` 权限。
Issue 汇报只是观察的旁路输出，失败或延迟不能阻塞 `status`、`update`、`publish` 或
角色工作。

## 启动一次 execution

启动需要两个终端。外部操作员只负责维护无头实验室；Host 负责连接门禁、选择 plan 和开始
实验。外部 `oc-lab` 可以在长程 bug fix 开始时预先运行：

外部操作员在终端一、主仓库根目录运行：

```bash
./oc-lab run t1 --port 4199
```

`oc-lab run` 不需要 plan-id；端口可以显式指定，也可以由工具自动选择。它创建临时 lab root，
启动无头 OpenCode daemon，并将 `{port, root}` 写入 `target/labs/t1/config.json`。lab 固定使用
`OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX=128000`。它不读取 plan、不准备 workspace、
不创建正式 session，也不启动 TUI；不要关闭这个终端。

实验室可以直接列出和进入其中的 session：

```bash
./oc-lab ls t1
./oc-lab attach t1 ontology-3/1
```

`ls` 直接查询 OpenCode，并只显示 workspace 位于该 lab root 下的 session。`attach` 按 title
精确查找 session；title 不唯一时拒绝连接。停止 `oc-lab run` 会终止服务器、删除 lab root
及 `target/labs/t1/config.json`，因此需要保留的实验资产必须在停止前导出。

Host 在长程任务开头从主仓库运行连接测试：

```bash
./oc-ctl test-connect t1
```

测试成功后可以继续修复 bug、更新教程和构建代码；此时 plan revision、Telora binary、
教程和实验 workspace 都尚未冻结。

所有前序工作完成后，Host 才从主仓库根目录显式选择 plan：

```bash
./oc-ctl start t1 ontology-3
```

`start` 要求该 lab 已通过连接测试，然后从当前仓库的 `experiment-plans/<plan-id>` 加载并
校验计划。它根据服务器中的既有 title 分配下一个 generation，例如 `ontology-3/1`，随后复制 workspace 资产、生成
OpenCode adapter、构建或复制声明的 artifact、执行权限预检，并在同一个 daemon 上为正式
workspace 创建 session。`start` 还会发布 `start_artifacts`，并只提示 coordinator 一次。
准备或启动失败只返回给 Host，不会终止 lab daemon；修复输入后仍可复用同一实验室。

需要在新版 DAG 中复用同一 lab 内旧执行已经验收的长耗时阶段时，显式指定来源 session：

```bash
./oc-ctl start t1 ontology-3 --from ontology-3/1
```

这会创建全新的 workspace 和 OpenCode session。通常只有在旧执行中仍为 current、且在新版
DAG 中定义未变的同名 artifact 才会被继承；唯一例外是 Host promotion 仅增加了在旧审批前
已经 current 的必要前置项。`--from` 本身代表 Host 明确接受旧产物，即使新版 plan 的根输入
文件已经变化也不会强制重跑上游；若需要让新语言或需求使上游失效，应执行普通 start。
对应 checks 文件和 artifact freshness
会按 DAG 顺序重建。旧 session、`.oc-task` 工作窗口和角色上下文不会复制。由此可以保留
A1-A3 等昂贵的已验收输出，同时从新增的 A5 或其他后续节点开始工作。

所有 Host 控制命令都从主仓库根目录以 `./oc-ctl` 运行，避免因相对路径变化产生额外的
命令授权。`plan-id` 是 `experiment-plans/` 下的目录名，不是任意文件系统路径。

## 观察和调度

Host 用下面的命令获得权威状态：

```bash
./oc-ctl status t1 ontology-3/1
```

输出包括 workspace 路径、每个 artifact 的状态、各角色状态、最近任务的耗时和 token。
artifact 的关键字段为：

- `current`：输出文件检查和所有输入时间戳均有效；
- `runnable`：角色拥有该 artifact，输入已就绪且当前输出过期；
- `publishable`：Host 拥有该 artifact，检查通过且可以放行；
- `blocked_by`：尚未 current 的直接依赖；
- `stamp_mtime_ns` / `input_mtime_ns`：输出和最新输入的时间戳边界。

长程调度不要使用不可感知的 `sleep 60`。使用有界 Host pull：

```bash
./oc-ctl pull t1 ontology-3/1
./oc-ctl pull t1 ontology-3/1 <上次返回的-next_since>
```

已有 Host 门禁时它立即返回；否则最多等待 60 秒。退出前最后一次采集会返回所有
`at >= since` 的简短 events；timeout 只限制等待时间，不是事件上界。`next_since` 是返回事件
最大的 `at`，没有事件时保持原值。下一轮可依据稳定 id 去重包含的游标边界。
`requests` 只是前置 artifact 已就绪、
尚未 current 且只能由 Host 发布的 artifact 名称列表。requests 是当前快照，完全不受
`since` 过滤；首次出现及集合变化会立即唤醒 pull，未变化的 requests 仍会返回但不造成
忙循环。checks 仍在实际 publish 时验证。
Host 处理 requests 后，以上一次的 `next_since` 继续 pull；需要详情时运行
`./oc-ctl event <lab-name> <session-name> <event-id>`。这样等待期间一旦出现待审核/发布项，Host 可以立即响应。

实验期间由 Host 通过 `oc-ctl pull/status/event` 观察 ACP 流，并按约定频率汇报可验证进展。文件读取、写入、
命令调用、任务启动和获得结果属于进展；不可见的思考不算。角色 busy 时不要为了汇报而
干预它。单独的机械观察者不是调度前置条件。

一次成功的 `oc-task pull` 到对应 `submit` 是一个任务统计区间。不要把 Agent 的长会话
时间误当成任务时间，也不要因为角色暂时 idle 就认为它已经退出。

## 审核和放行 artifact

角色提交的是候选，例如 `qb.a1`；跨角色可见的正式交付由 Host 人工审核后发布，例如
`qb`。先从 `status` 取得 workspace，在其中检查候选文件和运行 plan 规定的验收命令。
检查通过后才放行：

```bash
./oc-ctl publish t1 ontology-3/1 qb
```

普通 `publish` 不能发布角色拥有的 `.<role>` artifact，也不能绕过缺失输入或文件 checks。
自动 checks 成功不等于人工验收成功。

Host 需要修复、预制或替换角色状态时，使用显式强制干预：

```bash
./oc-ctl update t1 ontology-3/1 role/output.telora=host/replacement.telora --force
./oc-ctl publish t1 ontology-3/1 candidate.a3 --force
./oc-ctl publish t1 ontology-3/1 candidate.a3=! --force
```

`--force` 只越过角色所有权，不绕过安全路径、未知 artifact、DAG 输入或 checks。受到影响的
进行中任务会归档为 stale。干预事件写入 execution 和 workspace 的
`control/host-interventions/`，并在 `status/stat` 中显示。

角色意外退出循环时执行：

```bash
./oc-ctl resume t1 ontology-3/1 a5
```

普通 `resume` 对仍在长期循环中的角色是幂等的。角色卡住、上下文已经偏离任务，或需要
重新加载修正后的 runtime adapter 时，Host 可以显式丢弃当前 turn 并 fork 干净会话：

```bash
./oc-ctl resume t1 ontology-3/1 a5 --force
```

角色当前领取的 artifact 任务记录仍然保留，新会话会重新 `pull` 并继续同一 DAG 工作。

已经在工作或等待 pull 时该命令幂等成功；否则先恢复原会话，必要时由控制面直接建立
coordinator 的替代 child session。只有观察到角色重新进入长期 pull loop 后命令才成功。

实验已经完成或不再需要旧会话继续轮询时，可以只中止该 execution 的 session tree：

```bash
./oc-ctl abort-sessions t1 ontology-3/1
```

该命令递归发现 coordinator 和 child sessions，只 abort 仍在运行的 turn；它不删除会话历史、
实验 workspace 或 artifact，也不停止 `oc-lab` 的无头 daemon。重复执行是幂等的，因此同一个
实验室仍可继续承载后续 execution 或供 Host 查看历史。

## 独立生产问题会话

需要验证持续服务角色时，使用 `execution.kind = thread-service` 的方案。该模式不建立
coordinator，也不运行 artifact pull loop；声明的角色就是 root Agent。研发交付可以作为
Host bundle 注入，从而复用已经验收的 A1-A4 资产而不重复研发过程：

```bash
./oc-ctl start t1 query-service-1 \
  --bundle target/exp-outputs/08-26
```

控制器只复制方案 `execution.bundle.paths` 明确声明的普通文件，拒绝符号链接和特殊文件，
并记录逐文件 hash 与整体 digest。A5 完成上岗题并结束答复后，由 Host 检查结果并显式冻结
baseline：

```bash
./oc-ctl approve-baseline t1 query-service-1/1 a5
```

审批会检查上岗产物、执行方案声明的验证命令，并固定 root session 的最后一条完整答复和
bundle digest。审批后不得继续向 root session 发消息。每道生产题从相同 baseline fork，题间
上下文隔离；同题澄清则进入原 session：

```bash
./oc-ctl open-thread t1 query-service-1/1 a5 0001 problems/0001.md
./oc-ctl comment-thread t1 query-service-1/1 a5 0001 comments/0001-clarify.md
./oc-ctl close-thread t1 query-service-1/1 a5
```

每个角色同时只能有一个 active thread。`open-thread` 和 `comment-thread` 只投递 UTF-8
文件；`close-thread` 要求最后一轮已经完整结束并由 Host 主动调用。`status/stat/event` 从独立
registry 观察 detached sessions，统计时剔除 fork 所复制的 baseline 历史，因而每题成本不会
重复包含上岗成本。`abort-sessions` 同样覆盖 registry 中的 detached sessions，但始终保留
实验室 daemon、历史和 workspace。

需要反馈时，先在 Host 当前目录准备正文，再投递到 workspace，最后发布反馈 artifact：

```bash
./oc-ctl update t1 ontology-3/1 \
  query-builder/FEEDBACK.md=feedback/qb.md
./oc-ctl publish t1 ontology-3/1 qb-feedback
```

顺序不能颠倒：先更新文件，后 touch artifact。角色自己的 review 使用
`<name>-feedback.<role>`；Host 整合后发布 `<name>-feedback`。新的输入时间戳会让旧候选
及其下游自然 stale，原作者随后由 `pull` 重新获得任务。

删除已投递文件或撤销 Host artifact 使用 `=!`：

```bash
./oc-ctl update t1 ontology-3/1 query-builder/FEEDBACK.md=!
./oc-ctl publish t1 ontology-3/1 qb-feedback=!
```

如果实验在 idle 边界需要更新 Telora binary 或教程，也使用同一投递机制，不直接修改
临时 workspace：

```bash
cargo build --release -p telora
./oc-ctl update t1 ontology-3/1 \
  bin/telora=target/release/telora \
  docs/TELORA.md=guide/TELORA.md \
  docs/TELORA-CLI.md=guide/TELORA-CLI.md
./oc-ctl publish t1 ontology-3/1 lang
```

这种更新建立了新的 runtime epoch。实验总结必须记录 revision、binary hash、输入 hash
和发布时间，不能把更新后的改善归因于角色自行迭代。

## 统计、结论与结束

随时可以读取稳定统计：

```bash
./oc-ctl stat t1 ontology-3/1
```

`stat` 按角色和任务报告耗时、token、最长 thinking 间隔、plan 在
`metrics.roles.<role>.commands` 中声明的命令类别次数/耗时，以及代码和文档产出。实验总结至少应区分：

- 语言学习、上游学习、实现、review 和反馈修订；
- 语言/类型系统/标准库/诊断问题与普通 API、算法或实验基础设施问题；
- 编译、`check`、严格 `run` 和 `--best-effort` 分别提供的反馈；
- 首轮结果与至多一次、无需语言变化的 Agent 自我改进轮。

语言机制问题先提取最小重现，再进入主仓库 Issue；不要要求被观察角色为了实验评分绕过
语言缺口。一次 intent 自然产生多少诊断属于 Host 的观察指标，不应暴露为角色考核目标。

当 plan 的 `finish_artifact` 已经 `current`，再次核对 `status`、`stat`、最终输出和评估
记录，再通知外部操作员终止无头实验室。当前公开控制面没有 `stop` 子命令；终止 daemon
不会替代 artifact 验收，也不会自动清理 execution。结果保存和历史分支命名
遵循具体 plan 的归档规则，artifact marker、临时 binary 和控制文件不应作为产品输出提交。

## 新增实验计划

新计划位于 `experiment-plans/<plan-id>/`，至少包含：

```text
experiment.json      workspace、通用角色能力、输入 artifact、metrics 和 workflow
README.md            角色、DAG、运行和交付说明
EVAL-METHOD.md        隔离、验收、观察、迭代和归因方法
<role-area>/GOAL.md  各角色只在任务就绪后读取的目标
roles/<role>.md       不含 runtime frontmatter 的角色工作协议
host/                 不复制到初始 workspace 的 Host-only 验收资产
```

顶层 schema 使用 `telora.experiment-plan/v1`，`experiment.json.workflow` 使用
`telora.artifact-workflow/v1`：

- `name.<role>` 由对应角色通过 `oc-task submit` 发布；
- 无角色后缀的 `name` 只能由 Host 通过 `oc-ctl publish` 发布；
- `input` 决定 DAG 和 freshness，末尾 `?` 只表示可选输入；
- `checks` 只验证交付文件存在且非空，不驱动 DAG；
- artifact 声明顺序决定一个角色同时有多个任务时的 pull 顺序；
- `start_artifacts` 是初始输入，`finish_artifact` 必须是 Host-owned；
- coordinator 只启动角色，不观察、不重试、不创建 artifact。

角色提示必须明确下面的长期循环：

```text
loop {
    match oc-task pull <role> {
        stop => break,
        waiting after at most 60s => continue immediately,
        task => { 完成唯一任务；oc-task submit <role> <artifact>; }
    }
}
```

修改基础设施后，先运行单元测试，再用一个不要求复杂 AI 产出的最小 DAG 实验验证
`start -> pull -> submit -> publish -> stale -> rerun`，最后才启动正式实验。

## 常见问题

- `oc-lab run` 持续占用前台：正常；该进程拥有实验室生命周期。
- `oc-lab run` 立即报告端口冲突：省略 `--port` 自动选择，或指定另一个空闲端口。
- `start` 报告缺少 connection test：回到主仓库运行对应 lab 的
  `./oc-ctl test-connect`；它不会启动实验，完成后再从 plan 目录执行 `start`。
- `start` 拒绝 plan：确认 `experiment-plans/<plan-id>` 已提交且没有未提交修改，并从
  Telora 仓根目录执行命令。
- `mark-done` 是 invalid choice：当前协议只有 `oc-task pull` 和 `oc-task submit`；旧命令
  不应再出现在角色提示中。
- artifact 文件已经存在但任务仍 runnable：文件不驱动 DAG；检查输入与输出 artifact
  的时间戳以及 `changed` 字段。
- feedback 没有触发重跑：确认先 `update` 正文，再 `publish` 对应 Host feedback artifact。
- 无头窗口不显示调度信息：coordinator 只负责启动。使用 `oc-ctl status` 查看 DAG 和角色状态，
  不要依赖 coordinator 持续输出。
- 无法联系 execution：先确认 `oc-lab run` 仍在运行，再检查
  `target/labs/<lab-name>/config.json` 和 `oc-lab ls <lab-name>`；不要用新的临时授权
  请求替代诊断。

基础设施协议和实现细节另见
[`tools/opencode_experiment/README.md`](../tools/opencode_experiment/README.md)。
