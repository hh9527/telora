# Telora 实验指南

Telora 使用独立的 Labflow 运行可复现、Artifact 驱动的 Agent 实验。实验方案保存在
`experiment-plans/<plan-id>/`，与具体 Agent runtime 解耦；Labflow 当前提供 OpenCode adapter。

## 核心模型

```text
Artifact  表达流程事实和完成边界
Asset     承载 workspace 文件或目录
Session   承载 Agent 上下文
```

Telora 仓跟踪方案、输入和脚手架。运行时 workspace、OpenCode 配置、角色 frontmatter、状态
和会话由 Labflow 在临时实验室中生成。独立实验结果仓只记录特定运行的输入快照、输出、诊断
和指标，不作为方案起点。

Labflow 有两种计划：知识工厂使用 `dag-mode`，由 Artifact DAG 承载研发、检视和 Host 放行；
打榜流水线使用 `benchmark-mode`，在冻结知识输入上运行固定题集。两种计划可以通过 Host bundle
移交资产，但不共享 Agent session。

在知识工厂中，Artifact 名称以 `.<role>` 结尾时归该角色所有，否则归 Host 所有。每个 Artifact
独立产生工作压力；输入只控制就绪和失效，不定义起点或终点。Host 和 Agent 都通过 submit
刷新 Artifact。

Asset 路径以 `/` 结尾时表示目录，否则表示文件。`level` 控制保留策略：

```text
0  环境或脚手架，不备份
1  过程资产，仅显式请求时备份
2  结果资产，默认备份
```

Artifact 的 Asset 必须存在且类型正确才能 submit。Agent 对自己所有 Artifact 的 Asset 有读写
权限，对直接输入 Artifact 的 Asset有只读权限；方案不再单独维护角色 `read`/`write`。

## 启动前

仓库根目录的 `./labflow` 固定到已验证的 Labflow revision。外部操作员维护实验室生命周期，
Host 只使用 `./labflow host ...`。开始长程任务前一次性确认完整 Host 命令权限；实验过程中
不得临时申请授权而阻塞调度。若需要向 Issue 汇报，也要提前确认对应权限，且汇报失败不能
影响调度。

外部操作员先运行：

```bash
./labflow lab run lab-1 --port 4201
```

该命令创建临时 lab root，启动无头 OpenCode server，并把 `{port, root}` 写入
`target/labs/lab-1/config.json`。它不读取方案、不创建实验 session。关闭该前台进程会回收
实验室，因此要先导出需保留的资产。

Host 在长程任务开始时验证连接：

```bash
./labflow host test-connect lab-1
```

这只建立 `connect/<generation>` 探针 session，不投递实验材料。若连接或权限有问题，必须在
此时修复；完成其他代码工作后再正式 start。

外部操作员可观察实验室：

```bash
./labflow lab ls lab-1
./labflow lab attach lab-1 ontology-3/1
```

## 方案结构

`experiment.json` 使用 `labflow.experiment-plan/v1`。Artifact DAG 使用
`labflow.workflow/v1`：

```json
{
  "schema": "labflow.experiment-plan/v1",
  "workspace": ["README.md", "src"],
  "roles": {
    "a1": {
      "description": "实现输出",
      "instructions": "roles/a1.md",
      "commands": ["labflow agent pull a1", "labflow agent submit a1 *"],
      "preflight": ["labflow agent pull a1", "labflow agent submit a1 *"]
    }
  },
  "assets": [
    {"source": "target/release/tool", "path": "bin/tool", "mode": "0555"}
  ],
  "execution": {"kind": "dag-mode"},
  "workflow": {
    "schema": "labflow.workflow/v1",
    "roles": ["a1"],
    "artifacts": {
      "input": {
        "desc": "Host 输入",
        "assets": [{"path": "README.md", "level": 0}]
      },
      "output.a1": {
        "desc": "角色输出",
        "input": ["input", "notes?"],
        "assets": [{"path": "src/", "level": 2}],
        "instruction": "完成并验证输出"
      }
    }
  }
}
```

顶层 `assets` 是从 Host 仓库构造 workspace 的投递项；DAG Artifact 内的 `assets` 是
workspace 中的流程资产。两者用途不同。

## 启动和调度

Host 从仓库根目录选择方案：

```bash
./labflow host start lab-1 ontology-3/1 ontology-3
```

Labflow 校验方案、复制 workspace、构建并投递顶层 Asset、生成 runtime adapter、执行权限
预检，然后创建 `ontology-3/1` 等确定性 title 的 session。方案根 Artifact 不会隐式刷新；
Host 根据计划显式 submit 初始输入：

```bash
./labflow host submit lab-1 ontology-3/1 lang qb-req edsl-req
```

角色循环由 coordinator 一次性启动。每个角色始终执行：

```text
loop {
  match labflow agent pull <role> {
    null => continue,
    task => { 完成唯一 target；labflow agent submit <role> <target>; }
  }
}
```

pull 最多等待 60 秒。成功响应列出 target、全部直接 input 的 `fresh` 状态，以及输入 Asset 的
`updated` 状态。`fresh: null` 表示可选输入未出现；Agent 不保存旧时间戳。

Host 不使用不可感知的长时间 sleep，而使用有界 pull：

```bash
./labflow host pull lab-1 ontology-3/1
./labflow host pull lab-1 ontology-3/1 <previous-next_since>
```

响应中 `timeline.events` 是 `at >= since` 的增量事件；`result.requests` 是当前待 Host submit
的 Artifact，不受 since 过滤。用 `timeline.next_since` 继续观察，需要详情时使用 `event`。

权威状态和统计：

```bash
./labflow host status lab-1 ontology-3/1
./labflow host status lab-1 ontology-3/1 --verbose
./labflow host stat lab-1 ontology-3/1
```

## Host 干预

Host 审核角色候选并运行方案验收命令，确认后刷新无角色后缀的批准 Artifact：

```bash
./labflow host submit lab-1 ontology-3/1 qb
```

投递或删除 Asset：

```bash
./labflow host update lab-1 ontology-3/1 \
  query-builder/FEEDBACK.md=feedback/qb.md
./labflow host update lab-1 ontology-3/1 query-builder/FEEDBACK.md=!
```

普通 Host 操作不能覆盖角色所有的输出。确实需要预制、修复或替换时显式使用 `--force`；
它不绕过安全路径、未知 Artifact、DAG 输入和 Asset 检查：

```bash
./labflow host update lab-1 ontology-3/1 role/output=host/replacement --force
./labflow host submit lab-1 ontology-3/1 candidate.a3 --force
```

角色意外退出循环时恢复：

```bash
./labflow host resume lab-1 ontology-3/1 a5
./labflow host resume lab-1 ontology-3/1 a5 --force
```

普通 resume 对健康循环幂等；`--force` 中止偏离任务的当前 turn 后重新进入循环。结束执行但
保留实验室时，可中止该 execution 的 session tree：

```bash
./labflow host abort-sessions lab-1 ontology-3/1
```

## 继承与打榜

复用旧执行中仍有效的昂贵产出：

```bash
./labflow host start lab-1 ontology-3/2 ontology-3 --from ontology-3/1
```

这会创建新 workspace 和 session，只继承兼容、current 的 Artifact 及其等级 1/2 Asset；
等级 0 脚手架来自新方案。旧角色上下文不继承。

打榜计划不使用 Artifact DAG 或 coordinator。它声明 Questioner、Answerer、输入、输出和
不包含标准答案的题集：

```json
{
  "kind": "benchmark-mode",
  "questioner": "q",
  "answerer": "a",
  "batchSize": 5,
  "input": [{"path": "knowledge/"}],
  "output": [{"path": "ch/out/", "level": 2}],
  "problems": [
    {"q": "problems/0000.md", "maxTurns": 2},
    {"q": "problems/0001.md", "k": "problems/0001-info.md", "maxTurns": 3}
  ],
  "bundle": {"paths": ["knowledge/"]}
}
```

`q` 是原始题面，`k` 是只对 Questioner 可见的可选隐藏事实。Labflow 按 `batchSize` 分组；
每组创建一个全新 Questioner，Questioner 创建一个全新 Answerer 子会话，这一对连续完成组内
题目。不使用 preflight 或 session fork，学习成本由组内多题自然摊薄。

启动时，Host/Labflow 把整套题一次性填入 plan workspace 的 `problem/<id>/`，每个 batch 只
触发 Questioner 一次。Questioner 按顺序读题、向同一个 Answerer 提问并自主处理必要澄清；
Host 不参与逐题调度。

每题的 Q 必须原样进入对话管道，Questioner 不得转述或改写。每题必须由 Questioner 写出非空
`ch/out/report.md`。Answerer 可以写 `ok-*` 成功证据或 `err-*` 失败证据，也可以两者都不写；
两类证据不能并存。Labflow 不解释证据文件的格式和内容。
Questioner 对每题先执行 `labflow problem start <id>`；Labflow 将原始 Q/K 复制到 `ch/`，并
生成包含 `id`、`maxTurns` 的 `ch/metadata.json`。结束时执行
`labflow problem end ok|error|cancel`，Labflow 根据 metadata 归档到 `result/<id>/`：`ok`
只保留 `ok-*`，`error` 只保留 `err-*`，`cancel` 不保留两类证据；三种状态都要求报告存在。
随后清理通道并建立下一题的统计边界。整批结束后 `result/stats.jsonl` 每题一行。
Questioner 不判断正确性，最终判断由 Host 完成。

知识工厂输出先整理为 bundle，再启动打榜流水线：

```bash
./labflow host start lab-1 query-service-1/1 query-service-1 \
  --bundle target/exp-outputs/08-26
```

该命令同步运行完整题集并返回 timeline、逐题 transcript 和输出清单。计划不保存预期答案，
也不要求 Host 在运行中扮演用户。

## 验证清单

正式实验前至少完成一次小型 smoke plan，验证：

- schema、Asset 文件/目录检查及保留等级；
- 角色读写权限由 Artifact 关系正确推导；
- Agent pull 超时返回 `null` 并立即重试；
- Host submit、update、pull、resume 和 `--force`；
- 可选输入刷新使旧输出失效；
- `stat` 中的角色耗时、token、最长 thinking 和方案声明命令计数。
