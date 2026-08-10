# 实验协议（test-1）

这是一个四角色分层的 ontology eDSL 实验，全部在 opencode 内运行。

## 角色与权限（前缀读）

```text
A2: R(a1),        RW(a2)   # 实现 eDSL
A3: R(a1+a2),     RW(a3)   # 建模私有企业
A4: R(a1+a2+a3),  RW(a4)   # 查询意图（测试者/攻击者）
主 Agent: 全读写 + 创建/维护 EWS
```

- 每层只读"它之前的全部"，只写自己的子目录。
- 主 Agent 在验收时检查越界（读了不该读的、写了不该写的）。
- 所有执行（telora check/run）只由主 Agent 进行；Agent 不执行，只写代码。

## 阶段

1. Stage 2：A2 读 a1，在 a2 实现 a1/EDSL-DESIGN.md 规定的 ontology eDSL，
   并写 EDSL_TUTORIAL.md（教 A3）、AI3_CONTRACT.md（企业必须定义什么 / eDSL 保证
   什么）、STAGE2_NOTES.md。
2. Stage 3：主 Agent 放 domain + A3 brief 到 a1。A3 读 a1+a2，在 a3 建模企业，
   写 valid.telora / invalid.telora / PUBLIC_INTENT.md（唯一给 A4 看的企业文档，
   不得含表/列/连接/物理计划）/ STAGE3_NOTES.md。
3. Stage 4：主 Agent 放 intent 子集教程 + A3 公共接口说明 + A4 brief。A4 读
   a1+a2+a3，在 a4 写查询意图。**A4 必须通过 A3 暴露的查询 Lowering 接口得到
   计划（SQL），不得直接写 SQL 绕过。** 主 Agent 判断力验收。

## 反馈通道

下游 Agent 若认为前置输出有问题，可在输出中声明。主 Agent 用可恢复会话转发给
前置，前置修复，主 Agent 重新验证，再让下游继续。

## 预算

- 每 stage 至多 6 轮诊断反馈；连续 2 轮相同根诊断视为卡住。
- 主 Agent 不替 Agent 改代码；只转发诊断与反馈。

## 验收

- 主 Agent 执行 telora check/run 验证各 stage 产物。
- A4 的"协议强制"验收：主 Agent 判断 A4 是否走 A3 接口（非直接 SQL）。
- 各 stage 产物写入对应子目录；RUNLOG.md 记录每轮。
