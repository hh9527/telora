# Ontology four-Agent experiment for opencode

- Stage: Discussion
- Owner: 主 Agent（本仓库的人类 Host 与推进 Agent）
- Status: 设计已确认，未启动
- Related: RFC 0217（clean-room 四 AI 实验）、RFC 0202–0216（ontology eDSL 阶段）、
  `examples/saas-support-reporting/EXPERIMENT.md`（第三企业 Code Agent 试点）

## 目的

用 opencode 实现一个**自动闭环、但过程尽可能可观察**的实验：检验"知识通过文件在
三个隔离的 AI 角色之间分层传递 + 评价函数（Telora 验证闭环）驱动收敛"是否成立。

不追求强 clean-room（那属于 RFC 0217 的控制器方案）。本实验优先**可观察的协作式
知识传递**，用前缀读模型保住分层独立性。

## 角色

- **主 Agent**：整体推进者。创建 EWS、准备/更新 `a1`、按序调用 A2→A3→A4、
  承担验证（执行 `telora check/run` 并转发诊断）、A4 规范验收、维护 `RUNLOG.md`、
  最终写 `SUMMARY.md`。
- **A2**：读 `a1`，写 `a2`。实现 a1 中给出的 ontology eDSL 设计，并写 eDSL 教程。
- **A3**：读 `a1+a2`，写 `a3`。基于 a1 的 Telora 知识 + a2 的 eDSL，建模一个
  私有企业，并发布公共意图面。
- **A4**：读 `a1+a2+a3`，写 `a4`。扮演测试者与攻击者：在 A3 发布的查询能力
  范围内提出查询，合法请求降级成计划，越界请求被拒，且不能绕过边界。

所有 Agent 使用同一个模型。变量的干净优先于"独立评审来自模型差异"。

## 权限模型（前缀读）

```text
A2: R(a1),        RW(a2)
A3: R(a1+a2),     RW(a3)
A4: R(a1+a2+a3),  RW(a4)
主 Agent: 全读写 + 创建/维护 EWS 结构
```

- 每一层只看到"它之前的全部"，看不到"它之后的任何内容"。
- 每层只能修改自己的子目录；可读其他子目录（限前缀）。
- 这同时满足：可观察（主 Agent 随时可见全部）+ 分层独立（下游看不到未来）。

## 工作区结构

```text
target/exp-ws/test-<id>/
  a1/                  # 主 Agent 准备；分阶段放入各 stage 输入
  a2/                  # A2 输出
  a3/                  # A3 输出
  a4/                  # A4 输出
  RUNLOG.md            # 主 Agent 记录：每阶段开始/结束、验证、诊断、反馈轮次、时间戳
  SUMMARY.md           # 主 Agent 最终总结
```

每次运行独立 `test-<id>`；实验产物保留，可复现。

## 分阶段输入（a1）

输入按 stage 分阶段放入 `a1`，**不一次性全放**：

- stage2 开始时：Telora 教程、ontology eDSL 设计文档、实验协议、A2 brief。
- stage3 开始时：企业 domain 题面（**不得在 stage2 之前出现**）、A3 brief。
- stage4 开始时：intent 作者子集教程、A3 的公共意图面/接口说明、A4 brief。

## 阶段流程

### Stage 2：A2 实现 ontology eDSL

1. 主 Agent 创建 EWS，把 stage2 输入放入 `a1`。
2. 调用 A2（可恢复会话）：A2 读 `a1`，在 `a2/` 写 eDSL 实现 +
   `EDSL_TUTORIAL.md`（教 A3 的教程）+ `AI3_CONTRACT.md`（企业必须定义什么 /
   eDSL 保证什么）+ `STAGE2_NOTES.md`。
3. 主 Agent 验证：用一个 A2 未知的中性微模型（自建），执行 `telora check/run`，
   检查：类型保持、缺失能力/不安全关系诊断保留主体、独立错误恢复、证据不完整
   不发布、有效证据调用类型化 builder。
4. 诊断转发给 A2，A2 修复，循环，直至通过。

### Stage 3：A3 建模私有企业

1. 主 Agent 把 domain + A3 brief 放入 `a1`，确认 `a2` 就绪，调用 A3。
2. A3 读 `a1+a2`，在 `a3/` 写企业模型 + `valid.telora`/`invalid.telora` +
   `PUBLIC_INTENT.md`（唯一允许 A4 看到的企业文档；不得含表/列/连接谓词/物理
   计划）+ `STAGE3_NOTES.md`。
3. 主 Agent 验证：可见用例（domain 要求的）+ 隐藏用例（新奇组合、不同粒度度量、
   fan-out-only 维度、缺失能力、发布原子性）。可加语义分类审查（重复/误分类）。
4. 诊断转发，循环，直至通过。

### Stage 4：A4 提出查询意图

1. 主 Agent 把 intent 子集教程 + A3 公共接口说明放入输入，调用 A4。
2. A4 读 `a1+a2+a3`，在 `a4/` 写查询请求（合法 + 越界 trial）。
3. **协议强制**：A4 必须通过 A3 暴露的查询 Lowering 接口得到计划（SQL），
   不得直接写 SQL 绕过。A4 按规范性要求自验证。
4. 主 Agent 验收（判断力，不形式化）：检查 A4 输出是否走 A3 接口；
   并验证合法请求降级成计划、越界请求被拒（capability 缺失 / grain 扩张 /
   fan-out / 不可能请求）。

### 总结

主 Agent 写 `SUMMARY.md`：各 stage 收敛轮次、验证结果、暴露的缺口、
"A4 是否无法绕过边界"的判断、教训。

## 反馈通道

A2/A3/A4 在工作中若认为前置输出有问题，可在输出中声明（如 `NOTES.md` 或 task
返回）。主 Agent 收到后，用可恢复会话转发给对应前置 Agent，前置修复，主 Agent
重新验证，再让下游继续。修复轮次计入 RUNLOG。

## 记录与可观察性

- `RUNLOG.md`：每 stage 开始/结束、命令类、诊断（原始 + 转发）、反馈轮次、时间戳。
- 所有产物（输入/输出/诊断/反馈）都落在 EWS 内，主 Agent 与人类 Host 全程可见。
- 单次运行不证明普遍性；结果按"语言可学性 / eDSL 复用 / 企业建模 / 意图收敛 /
  边界强度"分别记录，不坍缩为一个成功率。

## 预算与终止

- 每 stage 至多 6 轮诊断反馈。
- 连续 2 轮相同根诊断视为卡住，主 Agent 记录并决定是否干预。
- 主 Agent 不替 Agent 改代码；只转发诊断与反馈。

## 局限与诚实声明

- 模型相同：独立评审只来自角色 prompt 与上下文隔离，不来自模型差异。
- 前缀读而非完全隔离：A3 可见 A2 完整输出（含笔记），协作优先。
- A4 边界是协议强制（看得到实现，但必须走接口），非物理隔离；验收靠主 Agent
  判断力，不形式化。
- 本实验验证"这一套教程 + 这一份 eDSL 设计能穿越三个隔离角色并收敛"，
  不证明普遍适用或任意领域的可迁移性。
