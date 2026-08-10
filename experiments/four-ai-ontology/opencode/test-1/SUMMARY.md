# SUMMARY — test-1 四 Agent ontology eDSL 实验

- 完成：2026-08-10（Stage 2-4 全部通过）
- 模型：所有 Agent 使用同一模型（deepseek-v4-flash）
- 协议：前缀读（A2:R(a1) / A3:R(a1+a2) / A4:R(a1+a2+a3)，各写自己目录）

## 结果概览

| Stage | 角色 | 产出 | 结果 |
|---|---|---|---|
| 2 | A2 实现 ontology eDSL | types/ontology/compiler + 教程/契约/笔记 | ✅（compiler 由主 Agent 用参考形状补） |
| 3 | A3 建模航班运营企业 | 企业模型 + valid/invalid + PUBLIC_INTENT | ✅（2 轮收敛） |
| 4 | A4 查询（测试者/攻击者） | 6 类 trial + 攻击探针 | ✅（合法 lowered / 非法 refused） |

## Stage 2 关键发现（Telora 泛型推断边界）

1. **选择性导入（`{ func }`）的泛型函数调用会推断退化为 Any，命名空间导入（`as module`）正常**——A2 反复失败的主因，12 轮 + 主 Agent 大规模复现才定位。
2. **可工作形状**：CPS compileRequested（带 Input、idOf 返回 Id、内部直接调 lowerRequested 无包装闭包）+ classifyPaths（无 eq 参数、用内置 `==`）+ 最内层计算。
3. 该形状可发现性极低：独立作者（A2）+ 模式提示仍未收敛，只有主 Agent 用参考减法版 + 精确函数形状才能通过。这是 Telora 的一个真实工程缺口。

## Stage 3 关键观察

- A3 使用 DSL（2 轮收敛）远易于 A2 实现 DSL（12 轮）——eDSL 在消费侧的可用性成立。
- 跨层诊断工作：非法场景带模型规则位置 + eDSL 规则位置双来源。

## Stage 4 关键结果（A4 攻击发现）

- 合法查询（直接/新奇组合/顺序无关）→ lowered ✅
- 非法查询（缺失能力/fan-out/未授权）→ refused ✅
- **A4 作为攻击者发现并证实了真实缺口**：
  1. `compile([])` 返回 `Some(空计划)`——发布门不要求"至少一个能力意图"
  2. 重复请求不去重（`['FlightCount,'FlightCount]` → 重复粒度）
  3. fan-out 拒绝依赖企业 buildPlan 兜底（库门不阻塞 fan-out-only 目标）——结构性依赖
  4. buildPlan 被导出，技术上可绕过 compile（受协议约束未利用）
- **协议强制验收通过**：A4 能看到 model 完整实现（前缀读），但所有查询都走 `model.compile`，未直接构造计划。

## 边界与诚实

- 主 Agent 在 Stage 2 补了 compiler.telora（用户选项 1）——A2 独立实现的边界止于
  types/ontology；compiler 管线需要主 Agent + 参考形状。
- 发现的 Telora 缺口（选择性导入退化、大泛型函数形状敏感）应作为语言改进输入。
- A4 攻击发现的模型级缺口（空计划/重复/导出面）应反馈给 A3 迭代。
- 单次实验、单模型、单企业领域——不证明普遍性；但四个角色的知识传递链（语言→eDSL→模型→查询）
  端到端走通。

## 下一步（可选）

- 把 A4 发现的缺口反馈给 A3 迭代模型（拒绝空请求、去重、收紧导出面）
- 将 Stage 2 的 Telora 推断发现反馈给语言层（选择性导入 + 大泛型形状）
- 换更强模型重跑 Stage 2，对照"A2 独立实现"的收敛
