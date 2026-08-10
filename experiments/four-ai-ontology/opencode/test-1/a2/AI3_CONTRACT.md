# AI3_CONTRACT.md — 企业模型必须定义什么 / eDSL 保证什么

本契约以 `compiler.telora` 的 `compile_with`（当前唯一入口）为基准。库文件：
`ontology-edsl/types.telora`、`ontology-edsl/ontology.telora`、`ontology-edsl/compiler.telora`。

## A. 企业模型必须定义什么

### A1. 封闭类型（全部由你声明，库不认识它们）
| 类型 | 约束 |
|---|---|
| `ReqId` | 请求/能力身份。**必须支持内建 `==`**（`idOf(cap) == request` 用它） |
| `Entity` | 实体（粒度节点）。**必须支持内建 `==`**（路径闭包/分类用它） |
| `Cap` | 能力记录（度量/维度能力）。字段语义由你定，需能提供 `idOf` 与 `lower` |
| `Input` | 降级的输入值 |
| `Output` | 降级的完成值（度量片段 / 维度载荷，可为你自己的和类型） |
| `Requirement` | 关系需求；需能提供 `targetOf` 取目标实体 |
| `Relation` | 关系记录；需能提供 `fromOf`/`toOf`；`mapping` 是你的物理载荷 |
| `Plan` | 你的惰性执行计划（`buildPlan` 的产物） |

### A2. 选择器与策略回调（typed，不传 Any/Dyn/String 身份）
| 回调 | 签名 | 含义 |
|---|---|---|
| `idOf` | `Fn(Cap) -> ReqId` | 能力的目录身份 |
| `lower` | `Fn(Cap, ReqId, Input) -> Option(Output)` | 降级；**第 2 参是请求的原始身份（意图）**，产物来源应指向它 |
| `requirementsOf` | `Fn(Array(Output)) -> Array(Requirement)` | 对整批完成值做"组合 + 派生关系需求" |
| `targetOf` | `Fn(Requirement) -> Entity` | 需求的粒度目标 |
| `subjectOf` | `Fn(Requirement) -> Entity` | 诊断主体（**当前实现未使用**，见 D） |
| `fromOf` / `toOf` | `Fn(Relation) -> Entity` | 关系端点 |
| `eq` | `Fn(Entity, Entity) -> Bool` | 实体相等（供诊断成员判断） |
| `buildPlan` | `Fn(Array(Output), Array(Requirement), Array(Relation)) -> Option(Plan)` | 最终构建器；`'None` 时须自报诊断 |

### A3. 数据
| 数据 | 类型 | 含义 |
|---|---|---|
| `requests` | `Array(ReqId)` | 请求的意图身份列表 |
| `capabilities` | `Array(Cap)` | 能力目录 |
| `input` | `Input` | 本次降级的统一输入 |
| `base` | `Entity` | 基准粒度实体（路径起点） |
| `allowed` | `Array(Entity)` | 授权实体列表 |
| `safeEdges` / `fanOutEdges` | `Array(Relation)` | 安全目录 / fan-out 目录；**同一条关系只能出现在一个目录** |

### A4. 导入方式（硬性）
必须用**命名空间导入**：`import ".../compiler.telora" as compiler;` 然后 `compiler.compile_with(...)`。
不要用 `import ".../compiler.telora" { compile_with }` 选择性导入——Telora 推断会退化。

## B. eDSL 保证什么（企业不实现）

| 能力 | 行为 |
|---|---|
| 独立降级 | 每个请求独立执行；一个失败不短路其他；缺失能力诊断以**请求身份**为主体 |
| 部分结果收集 | 保留每请求 `Option` 结果，派生完成值数组 |
| 编译完整性 | 只有所有请求都有值，`complete` 才是 `'Some(values)` |
| 需求聚合 | `requirementsOf` 一次作用于全部完成值（共享规则） |
| 图闭包与分类 | 从 `base` 做有界闭包（6 轮），一次派生三结果：joins（安全边）、fanOutOnly、unreachable |
| 维度路径诊断 | 目标在 fanOutOnly/unreachable 时，以**需求 + 目标实体**为主体报诊断 |
| 允许列表校验 | 目标未授权时诊断，并阻塞发布 |
| 原子发布门 | `compilationComplete && 无未授权 && 无 unreachable && 无 fanOutOnly && buildPlan 成功` 才发布；否则 `'None`（fan-out 拒绝路径带"粒度扩张需预聚合"诊断） |
| 诊断先于门 | 诊断与候选构建发生在门判定之前，不被过早的发布门掩盖 |

## C. 诊断契约（三层来源）
- **意图**：请求身份（`lower` 收到的第 2 参；缺失能力诊断的 subject）。
- **模型事实**：能力记录（降级返回 `'None` 时）、需求值、目标实体。
- **共享规则**：库内的检查位置（消息文本来自库）。

## D. 已知注意点（如实告知）
1. `subjectOf` 参数已声明但在 `compile_with` 函数体内**未被使用**（诊断主体目前固定为需求值本身）。
   如未来要自定义诊断主体，应让库在 `diagnoseTargets`/`validateAllowlist` 里使用它。
2. 发布门已包含 fan-out 与 unreachable 检查（见 E）；`diagnoseTargets` 仍会对这两类目标逐个报
   诊断，拒绝路径另有汇总诊断（subject 为 fan-out 目标数组）。
3. `ReqId`/`Entity` 依赖内建 `==`：你的枚举/原子类型天然满足；若用自定义结构作为身份，
   需确认它支持 `==`。

## E. 发布条件（逐条）
1. 每个请求能力都产生值（`compilationComplete(complete)` 为真）；
2. `array.length(unauthorized) == 0`（允许列表通过）；
3. `array.length(unreachable) == 0`（无不可达目标）；
4. `array.length(fanOutOnly) == 0`（无 fan-out-only 目标——粒度扩张需预聚合，拒绝发布）；
5. `buildPlan(...)` 返回 `'Some(plan)`。
全部满足才返回 `'Some(plan)`；否则返回 `'None`，期间产生的所有诊断由 Host 收集。
