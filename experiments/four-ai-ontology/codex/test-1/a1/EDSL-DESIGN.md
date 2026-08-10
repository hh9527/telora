# Ontology eDSL 表面设计

本文是 clean-room 实验中交给 AI-2 的稳定设计输入。它描述 eDSL 应向企业模型
提供什么能力、各层承担什么责任，以及哪些语义必须成立。它不提供 Telora 实现、
内部算法或现成函数签名。

AI-2 的任务不是重新发明方法论，而是：阅读 `tutorial.md` 学会 Telora，将本文设计
实现为可复用库，并写一份只面向企业建模者的 eDSL 教程。

## 1. 目标

这门 eDSL 用于把领域 ontology 写成可执行的意图编译器：

```text
领域意图
-> capability lowering
-> 关系与组合规则验证
-> 领域 execution plan
```

它既验证，也 lowering。验证失败应产生来源可追踪的诊断；验证成功应得到 typed
plan，而不是只返回 True。

eDSL 必须允许多个企业共享方法论，同时保留各自封闭的 Entity、Capability、Intent
和 Plan 类型。它不能预先规定 SQL、数据库、报表 renderer 或固定 Host ABI。

## 2. 三层结构

### Definition families

第一层用 TypeMetadata function 描述通用角色，但让企业选择所有具体类型。

至少需要表达三类定义：

```text
Capability definition
    id
    lower(id, input) -> Option(output)

Relationship definition
    from node
    to node
    cardinality/safety classification
    enterprise-owned mapping payload

Compilation evidence
    every independent result
    completed values
    evidence that all requested values completed
```

Analytics profile 还需要更具体的 role family：

```text
Measure definition
    measure id
    semantic value type
    natural grain/entity
    aggregation policy
    lowerer

Dimension definition
    dimension id
    lowerer
```

这些 family 产生企业自己的具体 record 类型。eDSL 不拥有企业 id enum、lowering
input、output 或 mapping 类型。

### Reusable rules

第二层是普通高阶 Telora 函数，实现一次维护的规则：

- requested id 到 capability 的查找；
- 每个请求独立执行 lowerer；
- 缺失 capability 的统一诊断；
- partial result 与 complete evidence 同时保留；
- required node 汇集；
- safe、fan-out-only、missing path 的分类；
- 对具体 authored subject 报告关系错误；
- 只有完整证据才允许发布最终 plan。

规则必须对企业类型参数化，不能要求 String id、`Any` 或 `Dyn`。

### Enterprise model

第三层由使用 eDSL 的企业定义：

- 封闭 Entity/Measure/Dimension 等 vocabulary；
- capability catalog；
- 关系事实和 mapping payload；
- 公式、自然 grain、aggregation 等领域知识；
- 真正属于企业的组合、授权或 alignment policy；
- final typed plan 与 builder。

企业不应重新实现 capability 搜索、完整性统计、通用路径分类或公共发布顺序。

## 3. Capability compilation

共享 capability compilation 接受以下抽象角色：

```text
requested ids
capability catalog
capability -> id selector
(capability, requested id, input) -> Option(output) lower adapter
shared lowering input
result builder, if the generic result family cannot be named directly
```

每个请求必须独立 lowering。某个请求缺失或返回 None 时，其他请求仍应尽可能执行，
以支持 Host best-effort diagnostics。

结果需要同时区分：

```text
results
    与 requested 一一对应的 Array(Option(Output))

values
    所有成功 Output

complete
    只有全部请求成功时才存在的完整 Array(Output)
```

`values` 可用于继续执行独立诊断，但不能单独证明可以发布 plan。

## 4. Relationship model

Relationship 是企业事实。eDSL 只解释端点和安全分类，不解释 mapping payload。

Analytics profile 至少区分：

```text
safe edge
    不扩张当前 measure grain，例如 many-to-one

fan-out edge
    扩张当前 grain，例如 one-to-many
```

给定 base node、required target nodes、safe edges 和 fan-out edges，路径规则需要同时
产生：

```text
selected safe edges
    构造合法计划所需的安全关系

fan-out-only targets
    可以到达，但所有已知路径都需要经过 fan-out

missing targets
    即使允许 fan-out 也不可达
```

完整可达性计算必须同时使用 safe 和 fan-out catalogs。一个 many-to-one edge 只属于
safe catalog，不应为了计算 fan-out 后续路径而重复放进 fan-out catalog。

路径实现可以是闭包、固定点或显式有界展开，但必须在文档中诚实说明其边界。不得
把一个适用于小 fixture 的有界深度描述成无界图算法。

## 5. 关系诊断

路径规则不能只返回 Bool。fan-out-only 和 missing target 应连接到具体 authored
requirement，例如用户请求的 Dimension。

共享验证函数因此至少需要：

```text
fan-out target set
missing target set
authored requirements
requirement -> required node
requirement -> diagnostic subject
```

fan-out 错误应说明该关系会扩张 measure grain，并要求显式 pre-aggregation 或
allocation policy。missing 错误应说明不存在经过验证的关系路径。

如果 measure 自身也产生 required node，设计应说明它的错误 subject 如何保留；
第一版可以把这一点明确记录为边界，但不能默默宣称所有 required node 都具有同样
精确的诊断。

## 6. Analytics compilation profile

在通用 definition/rule 层之上，实现一个 analytics profile，把常见编译顺序维护在
一个共享入口中：

```text
compile requested measures independently
-> compile requested dimensions independently
-> apply enterprise measure-combination policy
-> collect measure/dimension/extra required nodes
-> classify relationship paths
-> report fan-out and missing requirements
-> run enterprise final builder for independent downstream diagnostics
-> publish only if all required evidence is complete
```

这个入口必须对以下类型参数化：

```text
MeasureId, MeasureCapability, MeasureInput, MeasureOutput
DimensionId, DimensionCapability, DimensionInput, DimensionOutput
CombinedMeasure
Node, Edge, DiagnosticSubject
Plan
```

具体表面 API 由 AI-2 设计。它可以使用长参数函数、若干分层函数、typed builder 或
continuation，但必须保留类型关系，且 AI-3 能只依据教程正确实例化。

### Enterprise extension points

企业至少需要能提供：

- measure/dimension requested ids 和 capability catalogs；
- id selectors 与 lower adapters；
- measure/dimension lowering inputs；
- `Array(MeasureOutput) -> Option(CombinedMeasure)` 组合策略；
- combined measure 的 base node 和 required nodes；
- dimension 的 required node 和 diagnostic subject；
- filter 等阶段产生的 extra required nodes；
- safe/fan-out relation catalogs 与 endpoint selectors；
- `(CombinedMeasure, Array(DimensionOutput), selected edges) -> Option(Plan)`
  final builder。

AI-2 可以改善这些 extension points 的组织方式，但不能假设 Telora 存在 trait、
associated type 或结构约束。如果只能通过 typed selector 保持精度，应明确接受并在
教程中解释，而不是擦除类型。

## 7. Best-effort 与原子发布

这是设计的核心不变量。

### Best-effort diagnostics

相互独立的 measure、dimension、relationship 和 final-builder 检查应尽可能执行。
例如一个 dimension capability 缺失，不应阻止另一个已完成 dimension 的 fan-out
错误被发现。

在 measure combination 已经产生 candidate 的情况下，final builder 可以在完整性
门禁前执行，以便 restriction、render 或其他企业阶段产生独立诊断。

### Atomic publication

candidate 被求值不代表它可以发布。只有以下证据全部成立，shared compiler 才返回
`Some(Plan)`：

- measure compilation complete；
- dimension compilation complete；
- enterprise combination 成功；
- relationship requirements valid；
- final builder 返回 candidate。

其他情况必须返回 None。partial `values` 不得通过 fallback 形成可观察的成功 plan。

## 8. 类型要求

以下要求是硬约束：

- Entity、Measure、Dimension 等企业 vocabulary 保持封闭类型；
- capability output、mapping payload、combined value 和 final plan 保持企业具体类型；
- 公共协议不得使用 `Any`、`Dyn` 或 String id 制造复用；
- eDSL 不得要求所有企业使用同一 record plan；
- callback/selector 可以机械，但必须有 typed 契约；
- metadata family 无法精确命名时，可以使用 result builder/continuation；
- 不引入 compiler/VM/Host 的 ontology 特例。

## 9. 方法边界

共享 eDSL 不得包含：

- 具体企业 Entity/Measure/Dimension；
- table、column、join predicate 或 SQL AST；
- 固定 restriction vocabulary；
- 固定 renderer 或 output schema；
- 固定 Host execution ABI；
- 针对隐藏企业题面的特殊分支。

Analytics profile 是一套行业方法，不是 universal ontology object。通用层可以被其他
行业 profile 复用，但第一轮实验不要求 AI-2 同时证明非 analytics 行业。

## 10. AI-2 的表面设计责任

AI-2 必须自己决定并记录：

- package/module 布局；
- metadata family 的实际 Telora 函数名和参数；
- compilation evidence 的构造方式；
- path classification 的实现和深度边界；
- analytics compiler 是单入口还是分层入口；
- 如何组织较多 typed extension points；
- eDSL tutorial 的教学顺序；
- 哪些 adapter 是当前语言机制导致的样板；
- 哪些缺口留给后续版本。

AI-2 不得改变本文的 best-effort、atomic publication、closed types、diagnostic
provenance 和企业/方法论边界。

## 11. AI-2 交付物

```text
ontology-edsl/
  telora-deps.json
  src/...

EDSL_TUTORIAL.md
    AI-3 不读本文也能使用 eDSL 的教程

AI3_CONTRACT.md
    企业模型必须定义什么、共享层保证什么

STAGE2_DESIGN.md
    实际表面 API、内部策略、边界与取舍

STAGE2_NOTES.md
    预期风险和无法自行执行情况下的自检结果
```

## 12. 第一轮成功标准

第一轮目标是验证设计传递和实现能力，而不是方法论独立发明能力。

AI-2 成功意味着：

1. 仅依据 `tutorial.md` 和本文实现可检查的 Telora eDSL；
2. hidden neutral fixture 能实例化它而不复制 shared orchestration；
3. capability 缺失和 fan-out 能在一次 recovery 中独立报告；
4. 不完整 evidence 不发布 plan；
5. AI-3 只读 AI-2 教程即可完成隐藏企业模型；
6. 企业类型保持封闭，诊断保留 authored subject；
7. 实现没有企业题面知识或 analytics 物理细节；
8. AI-2 对有界算法、typed adapter 和语言缺口保持诚实。

未来提高难度时，可以逐步删减本文：先只保留不变量和验收，再要求 AI-2 设计
extension points；最终才测试 AI-2 能否只凭问题描述独立发明方法论。
