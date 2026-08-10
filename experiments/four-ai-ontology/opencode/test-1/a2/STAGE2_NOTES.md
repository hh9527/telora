# STAGE2_NOTES.md — 表面 API 选择、设计取舍、推断边界与风险

## 1. 交付物
- `ontology-edsl/types.telora`：角色构造器（`MeasureDefinition` / `DimensionDefinition` / `RelationDefinition`）。
- `ontology-edsl/ontology.telora`：能力降级原语、路径分类/诊断、允许列表、完整性。
- `ontology-edsl/compiler.telora`：唯一入口 `compile_with`。
- `ontology-edsl/telora-deps.json`：`{"dependencies":{}}`（std/array 直接导入）。
- 本文档 + `EDSL_TUTORIAL.md`（教 A3）+ `AI3_CONTRACT.md`（契约）。

## 2. 当前表面 API（compiler.telora）
```
compile_with(
    requests:     Array(ReqId),
    capabilities: Array(Cap),
    idOf:         Fn(Cap) -> ReqId,
    lower:        Fn(Cap, ReqId, Input) -> Option(Output),
    input:        Input,
    requirementsOf: Fn(Array(Output)) -> Array(Requirement),
    targetOf:     Fn(Requirement) -> Entity,
    base:         Entity,
    subjectOf:    Fn(Requirement) -> Entity,   # 声明但未使用
    allowed:      Array(Entity),
    safeEdges:    Array(Relation),
    fanOutEdges:  Array(Relation),
    fromOf:       Fn(Relation) -> Entity,
    toOf:         Fn(Relation) -> Entity,
    eq:           Fn(Entity, Entity) -> Bool,
    buildPlan:    Fn(Array(Output), Array(Requirement), Array(Relation)) -> Option(Plan)
) -> Option(Plan)
```

`compile_with` 内部（CPS 链，参考可工作形状）：
`compileRequested`（独立降级 + 完整性）→ `requirementsOf(values)` 派生需求 →
`classifyPathsNoEq`（有界闭包，3 结果：joins/fanOutOnly/unreachable）→
`diagnoseTargets` + `validateAllowlist`（诊断）→ 门：`compilationComplete(complete)
&& array.length(unauthorized) == 0 && array.length(unreachable) == 0
&& array.length(fanOutOnly) == 0` → `buildPlan(values, requirements, joins)` else `'None`
（fan-out 拒绝路径另发"粒度扩张需预聚合"诊断）。

## 3. Telora 推断边界（本实验最重要的发现）
1. **选择性导入退化**：`import "./x.telora" { a, b, c }` 使泛型函数调用退化为 `Any`；
   **命名空间导入** `import "./x.telora" as x;` + `x.a(...)` 正常。所有库调用必须走命名空间。
2. **CPS 函数体**：大管线函数必须是嵌套 continuation——每层锚定少量类型变量，
   实际计算（`match`/`let`/`if`/诊断调用）只能出现在**最内层**。
3. **continuation 回调参数**：不能含裸 for 变量（如 `Composed`），要用构造类型
   （`Array(Output)`、`Array(Requirement)`、`Array(Bool)` 等），否则该层推断退化。
4. **普通函数承载过多 for 变量退化**：一个"非 CPS、直接返回"的普通函数若带 6 个 for 变量
   且内部多次调用泛型函数，会全 Any 退化。计算应内联进最内层 continuation，或拆成
   ≤4 个 for 变量的小函数。
5. **第一个调用应返回具体构造类型**（如 `Array(Option(Output))`）来锚定大部分变量；
   不要用"返回裸 for 变量 R"的调用做锚定。
6. **内建 `==`**：参考形状用 `idOf(cap) == request`、`containsEq`、`array.any`、`array.length`、
   `if/else` 表达式——这些是可行的（本教程里 std/array 还包含 `any`/`length`，超出 a1 教程列表）。

## 4. API 变更历程（为什么变成现在这样）
- v1（21 参数大 lambda + 局部注解）→ 参数全 Any。根因：泛型函数体内局部注解不能引用
  外层 for 变量；巨型 lambda 推断退化。
- v2–v3（拆 stage + CPS 链）→ 逐步通过 `compileRequested`-类似形状；`Composed`/裸 for 变量
  在 continuation 里导致退化。
- 最终：主 Agent 定位"选择性导入"为退化主因，重写为命名空间导入 + 参考 CPS 形状；
  我保留的 `diagnoseTargets`/`validateAllowlist` 作为诊断原语接入。

## 5. 设计取舍
1. **单一能力目录**：度量/维度不再分两套编译，统一为 `Array(Cap)` + 模型自己的 `Output`
   和类型；`requirementsOf` 对整批完成值工作，兼容度量片段与维度载荷。
2. **组合并入 `requirementsOf`**：EDSL-DESIGN 里的"组合可用度量片段"步骤被吸收进
   `requirementsOf(Array(Output)) -> Array(Requirement)` 与 `buildPlan`，不再单列 `Composed` 类型
   （这是为了消除裸 for 变量的推断问题，也简化了表面）。
3. **`buildPlan` 只收 (values, requirements, joins)**：不含 fanOutOnly/unreachable；模型如需
   粒度扩张信息需自行从别处获得（或未来扩展）。
4. **有界闭包**：`closeSixNoEq` 固定 6 轮展开（诚实标注的界限）。超过 6 跳的路径会被低估为
   unreachable——微模型验证时请使用浅图。
5. **允许列表在门内**：未授权目标既诊断又阻塞发布（`array.length(unauthorized) == 0`）。
6. **角色构造器与可工作路径解耦**：`types.telora` 的构造器只是语义标记，`compile_with`
   通过选择器工作，因此模型也可用普通 `@struct` 而不强制用角色注解。

## 6. 已知问题与风险（如实申报）
1. **`subjectOf` 未使用**：已声明但函数体内不引用。保留它会让签名有冗余参数；
   若要自定义诊断主体，需让 `diagnoseTargets`/`validateAllowlist` 真正使用它。
2. **门条件**：完整性 ∧ 无未授权 ∧ 无 unreachable ∧ 无 fanOutOnly ∧ buildPlan 成功。
   fan-out 拒绝路径带"粒度扩张需预聚合"诊断（subject 为 fan-out 目标数组）；
   unreachable/fan-out 目标另有 `diagnoseTargets` 的逐需求诊断。
3. **内建 `==` 依赖**：ReqId/Entity 必须支持 `==`；若验收模型用不支持 `==` 的复杂类型会失败。
4. **闭包 6 轮上限**：深图会被误判 unreachable；文档已注明界限。
5. **微模型验证风险**：库对输入形状敏感（命名空间导入、`==`、能力目录非空等）；
   若验收微模型没有遵循命名空间导入，调用会退化。
6. **`array.any`/`array.length` 依赖**：假设 std/array 提供这两个函数（参考形状已使用并通过 check）。
