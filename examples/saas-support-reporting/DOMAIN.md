# SaaS 客服运营领域知识

## 企业背景

这是一家多租户 SaaS 企业。客户以 Workspace 为租户边界购买 Subscription，
Subscription 指向 Plan。Workspace 中的 User 可以创建 Ticket。Ticket 可被分配给
Agent，Agent 属于 Team。Ticket 选择一个 SLA Policy，并包含多条 Message。Ticket
还可以通过 TicketTag 关联多个 Tag。

目标不是读取真实业务数据，而是把接近业务意图的报表请求 lowering 为一个经过
验证的 typed execution plan。计划保存指标表达式、维度表达式和必要关系映射，
供后续 SQL lowering 使用。

## 物理数据模型

共有十一张表：

| Entity | 表 | 主键及关键外键 |
|---|---|---|
| Workspace | `workspaces` | `id` |
| Subscription | `subscriptions` | `id`, `workspace_id`, `plan_id` |
| Plan | `plans` | `id` |
| User | `users` | `id`, `workspace_id` |
| Ticket | `tickets` | `id`, `workspace_id`, `requester_id`, `assignee_id`, `sla_policy_id` |
| Message | `messages` | `id`, `ticket_id`, `author_agent_id` |
| Agent | `agents` | `id`, `team_id` |
| Team | `teams` | `id` |
| SlaPolicy | `sla_policies` | `id` |
| TicketTag | `ticket_tags` | `ticket_id`, `tag_id` |
| Tag | `tags` | `id` |

每条关系都应在模型中携带企业自己的物理 mapping，至少包含目标表名和 join 条件
字符串。具体字段名可以直接使用上表中的外键。

## 关系与 cardinality

以下从左到右都是 many-to-one 安全关系：

```text
Ticket -> Workspace
Workspace -> Subscription
Subscription -> Plan
Ticket -> User
Ticket -> Agent
Agent -> Team
Ticket -> SlaPolicy
Message -> Ticket
Message -> Agent
TicketTag -> Ticket
TicketTag -> Tag
```

以下方向会扩张当前 grain，属于 one-to-many fan-out：

```text
Ticket -> Message
Ticket -> TicketTag
TicketTag 之后才能到达 Tag
```

本挑战不提供预聚合或 allocation policy，因此 Ticket grain 的计划不得使用必须
经过上述 fan-out 才能到达的维度。

## 指标知识

### ResolvedTickets

- 业务含义：已解决工单数量；
- 自然 grain：Ticket；
- 语义值类型：Count；
- aggregation：Additive；
- 表达式：`COUNT(tickets.id) FILTER (WHERE tickets.status = 'resolved')`；
- 不需要 Ticket 之外的额外 entity。

### AgentReplies

- 业务含义：由客服发送的消息数量；
- 自然 grain：Message；
- 语义值类型：Count；
- aggregation：Additive；
- 表达式：`COUNT(messages.id) FILTER (WHERE messages.author_agent_id IS NOT NULL)`；
- 需要 Agent entity。

本轮只要求同 natural grain 的指标能够组合。不同 grain 同时出现时必须诊断并
返回 None，不得隐式选择预聚合策略。

## 维度知识

| Dimension | 所需 Entity | 表达式 | capability |
|---|---|---|---|
| OpenedMonth | Ticket | `substr(tickets.opened_at, 1, 7)` | 提供 |
| WorkspacePlan | Plan | `plans.name` | 提供 |
| AssignedTeam | Team | `teams.name` | 提供 |
| SlaPolicy | SlaPolicy | `sla_policies.name` | 提供 |
| TicketTag | Tag | `tags.name` | 提供，但 Ticket grain 路径会 fan-out |
| CustomerSegment | Workspace | `workspaces.segment` | 故意不提供 |

CustomerSegment 出现在封闭 Dimension enum 中，但本企业当前没有认可它的业务
定义，因此 capability catalog 必须缺少它。这个区别用于验证“可表达的概念”和
“已获准 lowering 的能力”不是同一回事。

## 企业发布策略

最终计划至少包含：

```text
revision          String
base_entity       Entity
measures          Array(MeasurePlan)
dimensions        Array(DimensionPlan)
joins             Array(Relation)
read_only         Bool
```

只有以下条件全部成立才可发布：

- 所有请求指标都有 capability；
- 所有请求维度都有 capability；
- 指标可以按企业的同 grain 策略组合；
- 所需 entity 均可沿安全关系到达；
- 不存在只可通过 fan-out 到达的维度。

共享 DSL 应负责通用 capability、路径和完整性流程；企业模型负责具体枚举、事实、
公式、关系 mapping、同 grain 策略和最终计划构造。
