# RUNLOG — test-1

- Started: 2026-08-10 17:12 (CST)
- Model: 所有 Agent 使用同一模型（opencode 当前模型）
- Workdir: target/exp-ws/test-1
- 实验协议: a1/PROTOCOL.md

## Stage 2 — A2 实现 ontology eDSL

- 输入: a1/{TELORA-TUTORIAL.md, EDSL-DESIGN.md, PROTOCOL.md, brief-stage2.md}
- A2 权限: R(a1), RW(a2)
- 状态: 待启动

（以下由主 Agent 逐阶段追加。）

### Stage 2 验证轮 1 (2026-08-10 17:2x)

- 库文件：types.telora ✅ / ontology.telora ✅ / compiler.telora ❌
- 诊断（compiler.telora:59:5）：
  `definition runPipeline has type Fn(Any, Any, ..., Any) -> T2, which is not assignable to Fn(Array<T0>, Array<T1>, Fn(T1,T0)->Bool, ...) -> Option(T10)`
  泛型签名声明的精确类型未被满足——21 个参数的 runPipeline 被推断为全 Any。
- 已转发给 A2 修复（不替它改）。

### Stage 2 验证轮 2 (2026-08-10 17:3x)

- A2 修复轮 1：给 21 个 lambda 参数加显式注解（引用 for 变量）。
- 结果：错误完全不变（仍 Fn(Any×21) -> T2）。
- 主 Agent 判断：A2 撞上 Telora 已知语言边界（RFC 0210 记录）——
  "泛型函数体内局部注解不能命名外层 scheme 参数"。
  参数注解 `measureRequests: Array(MeasureId)` 不会绑定 for 变量。
- 已把"这是语言边界 + 不依赖函数体内命名 for 变量"反馈给 A2，让它重新设计。

### Stage 2 验证轮 3 (2026-08-10 17:4x)

- A2 拆分为 prepareEvidence/classifyAndDiagnose/publishGate/runPipeline。
- 结果：prepareEvidence ✅、runPipeline ✅，仅 classifyAndDiagnose ❌（70:5，全 Any）。
- 对比观察：prepareEvidence 第一行把参数直接传给 compileCapabilities（一次约束 ReqId/Cap/Output）；
  classifyAndDiagnose 先 `let targetEntities = array.map(...)` 再调用 classifyPaths。
- 已反馈 A2：建议二分定位（把 classifyAndDiagnose 减到最小 → 逐项加回）。

### Stage 2 验证轮 4 (2026-08-10 17:5x)

- A2 二分"先 let"方向无效（内联后仍失败）。
- 主 Agent 用最小复现（/tmp）定位真实根因：
  - v2（continuation 全 for 变量）✅
  - v5（continuation 参数含 PathReport 结构类型）❌ → PathReport 被误推断为函数类型
  - v6（continuation 参数用 Int）✅
- 结论：Telora 的类型推断边界 —— **具体结构类型（@struct 记录）作为泛型 continuation 回调参数时，推断失败**。
- 已反馈 A2：重新设计 classifyPaths 的 continuation，避免具体结构类型参数。

### Stage 2 验证轮 5 (2026-08-10 18:0x)

- 主 Agent 复现链精确定位根因（非 PathReport，是更深的结构）：
  - v8（4 for 变量 + 内层 continuation 返回 R + Relation 仅透传）❌
  - v9（去掉 Relation，3 for 变量）✅
  - v12（4 for 变量但内层 continuation 返回具体 Int 而非 R）✅
- 结论（Telora 推断边界）：当一个泛型函数有 4+ 个 for 变量、其中一个（Relation）
  仅通过"返回 for 变量 R 的 continuation 函数"透传时，推断器无法锚定它 → 全 Any。
- 已反馈 A2：改变 classifyPaths 的 continuation 返回设计（避免返回裸 R 的透传），
  或调整 classifyAndDiagnose 的变量结构。

### Stage 2 验证轮 6 (2026-08-10 18:1x)

- classifyAndDiagnose 已修（Relation 移出）✅；但 runPipeline 改为三层嵌套后开始失败（141 行，全 Any）。
- 之前 runPipeline（浅回调链、8 for 变量）是通过的。改动后三层嵌套（prepareEvidence → classifyPaths → classifyAndDiagnose，每层返回 R）触发同一推断退化。
- 证据：prepareEvidence（单层）✅、参考两层嵌套模式 ✅、浅回调链 ✅、三层嵌套 ❌。
- 这是预算内最后一轮。已指示 A2 做彻底重新设计：压回两层嵌套，或显著减少 for 变量。

### Stage 2 验证轮 7（主 Agent 判断追加 1 轮）

- 精确定位：第 3 轮 runPipeline 纯透传 ✅；现在 runPipeline 函数体含
  `array.map` + 3 个内联计算 → ❌。
- 结论：runPipeline 函数体必须纯透传；所有计算（map/诊断/allowlist）在独立函数里。
- 已指示 A2：把 array.map 移回独立函数，runPipeline 只做
  prepareEvidence → classifyAndDiagnose → publishGate 的透传链。

### Stage 2 汇总（7 轮后）

- types.telora ✅ / ontology.telora ✅ / compiler.telora ❌（runPipeline 与其依赖 finalize 均全 Any 退化）
- 已超预算（6 轮）。每次 A2 修复一个函数，问题转移至下一个（runPipeline → classifyAndDiagnose → finalize），
  根诊断相同：泛型函数参数退化为 Any。
- 参考实现（同规模 13 for 变量）在仓库中可通过——说明"可绕过的写法"存在，
  但 A2（无参考实现）未在预算内找到该模式。
- 实验发现（Telora 推断边界）：多层/复杂泛型 continuation + 多 for 变量 + 函数体内计算，
  会触发"泛型参数全 Any"退化；对独立作者的可发现性差。

### Stage 2 轮 8（主 Agent 审查参考模式后给提示）

- 主 Agent 审查仓库中同规模可工作的管线函数（13 for 变量、21 参数、嵌套 continuation、通过 check），
  提炼出可工作模式的本质，作为提示给 A2（不给代码）。

### Stage 2 终态（8 轮，主 Agent 停止追加提示）

- types.telora ✅ / ontology.telora ✅ / compiler.telora ❌
- A2 在模式提示后已采用正确结构（锚定调用前置、纯透传、finalize 承载计算），
  但 finalize（6 for 变量、普通函数、实现与通过的 publishGate 同形）仍全 Any 退化。
- 结论（实验发现）：
  1. Telora 泛型推断对"大泛型函数（≥6 for 变量 + 内部多调用）"敏感，触发参数全 Any 退化；
  2. 该边界的**可发现性差**：clean-room 独立作者 8 轮 + 模式提示后仍未收敛；
  3. 参考实现（13 for 变量）证明可行——门槛在"发现可行写法"，不在 Telora 能力本身。
- 研究问题 Q1 得到负面部分答案：AI-2 独立实现大泛型管线函数遇难以绕过的推断边界。

### Stage 2 轮 9（继续提示）

- 状态：runPipeline（两层 CPS 链）已通过 ✅；finalize（6 for 变量、普通函数）仍 ❌。
- 关键观察：参考 compile_with 是 13 for 变量但 CPS 函数体 → 通过；
  普通函数承载 ≥6 for 变量 + 多调用 → 退化。
- 方向：finalize 的计算要么内联进 runPipeline 最内层 continuation（参考最内层即 match/let/if），
  要么拆成 ≤4 变量的更小函数。

### Stage 2 轮 10（继续提示，含最小复现工具）

- 内联计算进 runPipeline 后，runPipeline 又失败。
- 主 Agent 观察：参考 compile_with 最内层是"内联 if/else"（不调用额外普通函数）；
  A2 最内层调用了 publishGate（需要 ReqId 的普通函数）——ReqId 可能未在 CPS 链中充分锚定。
- 已授权 A2 在 a2/ 下写最小复现文件，主 Agent 代跑定位。

### Stage 2 主 Agent 深度定位（12+ 轮后）

- 主 Agent 用 /tmp 复现链彻底定位：参考 compile_with 的"魔法"无法通过占位/适配复现。
- 确认通过的形状（减法版）：ontology-method 的 compile_requested（内部直接调用
  lower_requested，带 Input、matchCap 返回 Id、无包装闭包）+ classify_paths（无 eq 参数）。
- 失败形状（A2 的）：compileCapabilities（返回具体类型 Array(Option(Output))）、
  matchCap: Fn(Cap,ReqId)->Bool、包装闭包、classifyPaths 带 eq —— 任何 A2 变体都全 Any 退化。
- 结论（重要实验发现）：Telora 泛型推断对"具体函数调用形状"极度敏感——不是结构/层数问题，
  而是 CPS 函数内部必须直接调用"带 Input、matchCap 返回 Id"的 lower 函数、无包装闭包、
  分类函数无 eq 参数。独立重构（即使结构正确）大概率失败，只有精确的可行写法通过。
  这大幅提高了"实现 Telora 大泛型函数"的门槛。

### Stage 2 突破（轮 12，主 Agent 定位"魔法"）

- 主 Agent 定位到 Telora 泛型推断的关键行为：
  1. **选择性导入（{ func }）的泛型函数调用会退化，命名空间导入（as module）正常**。
  2. 参考形状 compileRequested（带 Input、idOf 返回 Id、直接调 lowerRequested 无包装闭包）
     与 classifyPaths（无 eq 参数、用内置 ==）是可行形状；A2 的 compileCapabilities
     （返回具体类型、matchCap 返回 Bool）与 classifyPaths（带 eq）与可工作模式不兼容。
- 主 Agent 在 A2 ontology.telora 追加参考形状函数（lowerRequested/compileRequested/
  classifyPathsNoEq/compilationComplete），重写 compiler.telora（命名空间导入 + 参考形状 +
  A2 的 diagnoseTargets/validateAllowlist 诊断逻辑）。
- 结果：types ✅ / ontology ✅ / compiler ✅（全部 check 通过）。
- 新 API 形状：compile_with(requests, capabilities, idOf: Fn(Cap)->Id, lower: Fn(Cap,Id,Input)->Option(Output),
  input, requirementsOf, targetOf, base, subjectOf, allowed, safeEdges, fanOutEdges, fromOf, toOf, eq, buildPlan)。
- 待办：A2 更新 EDSL_TUTORIAL.md / AI3_CONTRACT.md / STAGE2_NOTES.md 匹配新 API；
  主 Agent 做中性微模型验收。

### Stage 2 验收完成（主 Agent 中性微模型）

- 微模型（仓库库存域，A2 未见过）check ✅。
- 行为验证：
  1. valid（TotalUnits，Item 粒度）→ Some(ExecutionPlan) + 完整安全 join 链 ✅
  2. 缺失能力（MissingMeasure）→ error + 规则来源 + 不发布 ✅
  3. fan-out（TotalParts，Item 需经 OneToMany 到达 Part）→ error（granularity expansion）+ 不发布 ✅
  4. 发布门已加入 unreachable 检查（主 Agent 修正，采纳 A2 异议）。
- Stage 2 完成。A2 贡献：types/ontology 通过、三份文档、独立二分发现、诚实申报异议；
  compiler.telora 由主 Agent 用参考形状 + A2 诊断逻辑实现（用户选项 1）。

## Stage 3 — A3 建模航班运营企业

- 输入已放 a1：domain.md（航班运营）、brief-stage3.md（含 A2 教训：命名空间导入等）。
- A3 权限：R(a1+a2), RW(a3)。
- 待启动。

### Stage 3 完成（A3 建模航班运营）

- A3 独立建模（读 a1+a2 教程，写 a3），1 轮语法修复（enum 带 payload 写法）。
- 验证：
  - 模型 check ✅（依赖路径修正后）
  - valid（FlightCount+RouteOrigin）→ Some(plan) ✅
  - 缺失能力（Revenue）→ error + 规则来源 ✅
  - fan-out（Boardings/Seat）→ error（granularity expansion）+ 不发布 ✅
  - 未授权（AirlineName）→ error + 不发布 ✅
  - 隐藏：FlightCount 单独 ✅、+AircraftType ✅、FlightCount+Boardings 整体拒绝 ✅
  - 诊断均带跨层来源（model 规则 + eDSL 规则）
- 对比：A2 实现 DSL 卡 12 轮推断边界；A3 使用 DSL 2 轮（1 语法 + 1 路径）收敛——
  印证"使用 DSL"远易"实现 DSL"，eDSL 可用性在消费侧成立。

## Stage 4 — A4 查询意图（测试者/攻击者）

- 输入已放 a1：intent-tutorial.md（最小查询教程）、brief-stage4.md（角色 + 6 类 trial + 硬约束）。
- A4 权限：R(a1+a2+a3), RW(a4)。
- 硬约束：查询必须走 model.compile；不得直接写 SQL/物理计划；主 Agent 验收。
- 待启动。

### Stage 4 完成（A4 查询 + 攻击）+ test-1 收尾

- A4 交付 6 类 trial + 攻击探针 + TRIALS.md；全部走 model.compile（协议强制验收 ✅）。
- 验证：合法（直接/新奇/顺序无关）lowered ✅；非法（缺失/fan-out/未授权）refused ✅。
- A4 攻击发现（被证实）：compile([])→Some(空计划)、重复请求不去重、fan-out 靠 buildPlan 兜底、
  buildPlan 导出可理论绕过。
- SUMMARY.md 已写。test-1 四 Agent 实验端到端完成。

### 缺口逐层修复（A4 攻击 → A3 → A2）

- **A3 企业层解决**：缺口 4（导出面收敛，只导 compile）、缺口 1（空请求拒绝，带 subject）、
  缺口 2（入口去重）——全部验证通过 + 回归通过。
- **发现 Telora 运行时 bug**：`emit_error!` 零 subject 触发 panic（RFC 0187 声称合法）。
  A3 用带 subject 绕开；记录为语言层发现。
- **缺口 3（fan-out 未进发布门）上升 A2**：eDSL 发布门应加 `fanOutOnly == 0` 检查。

### 缺口修复全部完成

- **A2（eDSL）**：发布门加入 `fanOutOnly == 0` 检查（缺口 3），compiler check ✅。
- **回归**：valid ✅、fan-out 拒绝 ✅、缺失能力拒绝 ✅。
- 逐层反馈闭环：A4 攻击发现 4 缺口 → A3 解决 3（导出面/空请求/去重）→ 缺口 3 上升 A2（发布门）。
- 附带发现：Telora `emit_error!` 零 subject 运行时 panic（记录为语言层 bug）。
