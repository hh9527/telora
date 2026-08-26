# Ontology 3 evaluation method

本文只供 Host 使用，不注入角色提示。实验问题是：四个隔离角色能否仅通过稳定公共
契约，依次建立 QueryBuilder、EnterpriseKnowledge eDSL、具体企业知识和公共查询面，
并完成：

```text
EnterpriseKnowledge + Request -> Plan -> SQLite Query
private text intent -> JSON -> fixed query command -> SQLite Query/diagnostics
```

## 受控输入

对每个输入发布轮次分别记录 Telora revision、方案 hash、binary hash、由控制面生成的
Agent runtime adapter，以及 `experiment.json`、语言/CLI 教程、各角色 GOAL、两个
DESIGN、DOMAIN、INTENT 和该轮 FEEDBACK 的 hash。只有输入一致的结果才直接比较；
runtime 更新前后的结果属于同一 execution 中两个不同 epoch。

## Host 启动门禁

执行 `labflow host start` 前，Host 必须一次性确认整个实验所需的本地命令、观察和外部写入
权限。实验启动后直至外部 TUI 结束，Host 不得申请任何临时授权，避免授权等待挂起调度、
观察、文件投递或 artifact 发布。若遗漏权限，继续使用已有权限维持实验，并在实验结束后
修正基础设施；不得让授权申请进入实验关键路径。

若要求把进展报告到实验 Issue，启动前还必须确认 `gh issue comment` 权限。Issue 评论是
观察结果的旁路副作用，不是 DAG 节点或调度前置条件；评论失败、延迟或限流时继续执行
`status`、`update`、`submit` 和既定观察，待不影响调度时再补报。不得为了按时评论而暂停、
唤醒或干预正在工作的 Agent。

## 隔离要求

- A1 只读自身设计与源码，不读 ontology 和企业领域。
- A2 在 QueryBuilder 检视任务中可见 A1 送审资产；进入自身 build 时只以 Host 批准的
  `qb` 公共教程/契约作为上游知识，不使用 A1 私有实现。
- A3 在上游检视任务中可见对应送审资产；进入自身 build 时只以 Host 批准的 A1/A2
  公共教程/契约和固定 JSON 查询契约作为上游知识，也永远不读 A4 私有 `INTENT.md`。
- A4 只读 A3 公共查询教程/契约、JSON 查询接口说明和自己的 GOAL/INTENT/FEEDBACK，不读
  语言/CLI、DOMAIN、A3 私有源码或 A1/A2 文件。依赖清单中的传递 package 不构成可见输入。
- coordinator 只启动 A1-A4 各一个原生 child session，不补写任务定义或观察交付状态。
- `ent-1/FEEDBACK.md` 初始必须为零字节；角色只在 `labflow agent pull` 返回对应任务后读取动态输入。
- 核对 `control/artifacts/**` 与实际交付顺序一致；coordinator 不直接创建 artifact。
- Agent 只通过 `labflow agent submit <role> <artifact...>` 控制自己的 `.<role>` Artifact；
  Host 只通过 `labflow host submit` 控制无角色后缀 Artifact。
- feedback 与 build 同时 runnable 时按 artifact 声明顺序逐个 pull、逐个 submit，任务不合并。
- 核对每个 Host promotion 都晚于对应角色候选和必要评审，且下游首次输出发生在
  promotion 之后。
- Host 发布的新反馈必须使旧候选失效；上游重新执行后旧 promotion 必须失效并重新
  人工审核。不得以文件存在或自动验证成功替代人工放行。
- `labflow host pull/status` 一旦列出待决策请求，Host 必须立即检视相应候选并选择
  `submit` 或投递 feedback；“不干预 working Agent”不能被解释成继续等待 Host 门禁。
- `ent-1-model` 必须早于公共 facade 首次写入；`ent-1-query-surface` 必须
  早于 A4 首次 Request 写入。A3 公共面不得包含物理名词或隐藏 intent 答案。
- A4 首轮若仍有无需语言变化的明确公共接口改进空间，Host 可筛选并发布至多一次
  `edsl-feedback` 或 `ent-1-query-surface-feedback`；确认它只使对应旧交付
  及其下游失效，并由原角色修订重验。

核对 ACP 事件与归档，确认语言学习并行、A2/A3 的 QueryBuilder 学习并行，且角色
没有提前读取动态输入或通过 glob/grep/命令输出越过边界。

## A1 评估

检查标准 Plan 算子和语义是否封闭稳定；PlanProfile 是否显式收窄能力而不改变语义；
动态值是否只进入 bindings；SQLite transform 是否确定；非法或 profile 越界 Plan
是否产生诊断且无 Query。使用隐藏 Plan 覆盖投影、filter、join、aggregate、group、
sort、limit、绑定顺序和重复转换。

QueryBuilder 发布候选后，由 A2 根据 DESIGN、A3 根据 DOMAIN 审查。该审查可以独立
执行，也可以由同角色已放行的 build 吸收。Host 决定哪些
意见属于 A1 可处理范围，并通过反馈 artifact 发布；语言机制问题不得强塞给 A1。只有
Host 发布 `qb` 后，公共契约才可供下游实现使用。

## A2 评估

检查公共边界能否用 nominal types、member/type properties 精确表达
EnterpriseKnowledge、PlanProfile 和 prepared `Request -> Plan`。验证能力授权、grain、最短安全路径、
目录序 tie-break、共享边去重、有向环、八边边界、fan-out-only、missing、请求覆盖、
profile 覆盖和失败时不发布部分 Plan。A2 不得定义替代 Plan 或自行生成 SQL。一次
lowering 自然产生的诊断数量只由 Host 隐藏观察，不向角色暴露为验收目标。另行检查
metadata/property 只在准备阶段读取，重复 lowering 不重新构图、枚举 property 或执行 BFS。
另外必须检查 Request 能表达独立的有类型维度筛选、只引用已请求投影的排序和正整数
Top N；验证等值/范围筛选、未投影筛选维度、稳定 tie-break、非空 bindings，以及缺失
筛选能力、非法输入、未请求排序目标和非正 limit 的原子失败。固定空 filter/order/limit
或仅保留未使用 input 字段不算完成。

## A3 评估

检查具体企业知识是否仅使用公共 API，是否以 nominal entity/member metadata 完整表达
DOMAIN 的封闭 vocabulary、能力、关系、物理 mapping 与 PlanProfile，且没有平行的
entity/attribute/relation 事实目录。合法 Request 必须产生覆盖完整的 Plan，并由固定 QueryBuilder
得到稳定 SQLite Query；非法 Request 必须产生来源明确的诊断且无可信 Plan/Query。
合法物流场景必须至少覆盖月份范围、客户等级、地区筛选、指标降序、维度升序 tie-break
与 Top N，并证明筛选值和 limit 进入 bindings。

另行检查 A3 公共查询面是否从同一份私有 ontology root/property 导出业务 vocabulary、Request 和
`lower`，是否足以独立使用且不泄漏表、列、alias、join mapping、SQL 片段/模板。分别
统计 A3 `modeling` 与 `query_surface_design` 的时间和 token。

## A4 评估

Host 同时读取私有 DOMAIN 与 INTENT，检查 A4 的 JSON 是否忠实表达合法和非法文字意图、
是否只使用公共词汇且只通过 `just a4` 调用固定查询接口。合法 Request 的 Query 必须与私有
物理模型语义一致并逐字节确定；非法 Request 可以在编译、check 或 lowering 阶段拒绝，
但不得发布部分 Query。A4 不得重写 Plan/Query/SQL 或诊断容器。单独统计公共查询面学习、
JSON 接口验收的时间、token 与 JSON/文档产出；A4 不再包含 Telora 语言学习。
合法意图的验收必须看到非空 bindings，并逐项对应月份范围、等级、地区和 limit；仅生成
全量分组 SQL 不通过。

## 过程与反馈

分别记录 A2-A4 首次学习、首次写入、反馈修正、wall time、token、命令失败、
类型擦除、公共 family 数量、最大类型参数数量、A3 模型 LOC、身份重复声明、手工目录
装配量和文档/实现差异。记录 cold preparation 与同一会话 repeated lowering，确认
direct import、reexport 与公共 facade 观察同一 prepared behavior。每轮 A3 交付后 Host 判断结束外部 TUI 或发布
下一轮输入。只有当前语言能力内仍有明显的 QueryBuilder/eDSL 改进空间时，
才发布批准反馈；语言机制问题仍转为独立 issue。若 Host 在 idle 边界发布新版
binary/教程，必须记录原子发布证据，并把角色变化归属于 runtime epoch，而非自身修正。

反馈按目标候选分区。记录每次反馈、修订候选和重新发布 Host promotion 的时间戳。

## 归因

根因使用最窄分类：输入契约、QueryBuilder 设计、eDSL 设计、企业建模、教程可发现性、
语言表面、类型系统、标准库、诊断或基础设施。先从实验提取最小重现，再把语言机制
问题独立跟踪；不能因更丰富语言可能避免错误，就把普通算法/API 缺陷归因给语言。
