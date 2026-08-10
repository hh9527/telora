# STAGE3_NOTES — 建模决策、坑与 eDSL 可用性判断

## 交付物

- `enterprise-model/telora-deps.json` —— 声明 `edsl` 依赖指向 `a2/ontology-edsl`
- `enterprise-model/model.telora` —— 企业本体（类型、能力目录、关系、策略、构建器、入口）
- `valid.telora` —— 合法报表：航班量按 RouteOrigin 分组
- `invalid.telora` —— 三个非法场景：缺失度量（Revenue）、粒度扩张（Boardings）、未授权（AirlineName）
- `PUBLIC_INTENT.md` —— 给 A4 的公开意图面

## 建模决策

1. **实体与身份枚举**：`Entity` = {Flight, Aircraft, Route, Airport, Airline, Seat}；
   `Measure`（即契约里的 `ReqId`）= {FlightCount, RouteOrigin, AircraftType, AirlineName,
   Boardings, Revenue}。两者都是封闭枚举，天然支持内建 `==`（契约 D3 要求）。
2. **Output 用单变体 `'Value(measure)`**：完成值携带请求身份。这样避免"同一个 tag 名
   （如 `'FlightCount`）出现在两个枚举里"带来的推断歧义风险；`requirementsOf` 靠它还原每个
   请求并要求对应粒度。
3. **能力目录**：度量 + 维度统一为 `Array(Cap)`；每个 `Cap.lower` 对**请求身份**返回
   `'Some('Value(自己的身份))`。`Revenue` 只有 `Measure` 身份、没有对应 Cap —— 天然触发
   库的"缺失能力"诊断。
4. **需求派生 `requirementsOf`**：把每个请求身份映射为它要求的粒度访问需求 `At(目标实体)`：
   - FlightCount → At(Flight)
   - RouteOrigin → At(Route) + At(Airport)（维度由 Route 提供、值为 Airport）
   - AircraftType → At(Aircraft)
   - AirlineName → At(Airline)
   - Boardings → At(Seat)（自然粒度）
   - Revenue → []（占位，匹配穷尽）
5. **授权**：`allowed` = [Flight, Aircraft, Route, Airport, Seat]，排除 `Airline`（外部数据源）。
   `Airline` 在安全图中可达（Flight→Aircraft→Airline），所以它不会被误诊为 unreachable/fan-out，
   而会被 `validateAllowlist` 精确诊断为"未授权"。
6. **粒度策略（关键）**：fan-out 边只有 Flight→Seat。读 `compiler.telora` 后发现发布门实际是
   `complete && unauthorized==0 && unreachable==0` —— **fan-out-only 目标只被诊断、不阻塞门**。
   因此企业必须在 `buildPlan` 里执行"扇出粒度不可直接分组"的策略：用**企业自己的**
   `fanOutRelations` 目录判断需求目标是否落在扇出粒度，命中则 `emit_error!` 并返回 `'None`。
   这用的是企业私有关系事实，不算复制 eDSL 编排逻辑。

## 坑 / 注意

1. **命名空间导入**：库必须 `import "edsl/compiler.telora" as compiler;` 后 `compiler.compile_with(...)`；
   模型模块内部全部走命名空间。顶层文件对**自己的**模型模块用选择性导入 `{ compile }` 是安全的，
   因为 `compile` 是具体函数（无泛型 for 变量），不触发教程所述的选择性导入退化。
2. **推断锚定**：所有回调（idOf/lower/requirementsOf/targetOf/subjectOf/fromOf/toOf/entityEq/
   buildPlan）都写了完整签名；`array.flat_map`/`array.any`/`array.find`/`array.map` 的泛型参数
   都由这些具体签名或 `Array(Relation)` 等具体类型锚定。
3. **关系目录**：safe 与 fanout 各一张表，同一条关系只出现在一个目录（契约 A3 要求）。
   我给 Relation 加了 `cardinality` 字段（`'Safe`/`'FanOut`）作为语义标记，`compile_with`
   只读 from/to，多余字段不影响。
4. **角色构造器未用**：`types.telora` 的 MeasureDefinition/DimensionDefinition/RelationDefinition
   是可选的语义标记；`compile_with` 走 typed 选择器，为降低推断面我改用普通 `@struct` + 选择器
   （教程 §2 明确允许）。
5. **subjectOf 未使用**：库声明了该参数但函数体内未引用（契约 D1），我按教程将 `subjectOf = targetOf`。
6. **闭包 6 跳上限**：本图最远 2 跳（Flight→Aircraft→Airline），安全。

## A4 攻击测试的四个缺口处理（第 3 轮修复）

1. **空请求发布空计划**（`compile([])` → `'Some`）：**企业层解决**。在 `compile` 入口先去重，
   再判 `array.length(cleaned) == 0` 时 `emit_error!` 并返回 `'None`。理由：空报表在业务上无意义，
   入口归一化是查询 API 的自然防线；同时建议 A2 在发布门加"至少一个请求"约束（零意图的发布
   是退化状态，属于 eDSL 发布契约）。
2. **重复请求不去重**（`compile(['FlightCount,'FlightCount])` 产生重复粒度）：**企业层解决**。
   `compile` 先用 `dedupeMeasures`（基于内建 `==` 的 `array.any`/`array.fold`）去重再编译，
   重复意图视为同一意图。理由：同一请求身份两次请求是调用方噪声，入口归一化处理合理；
   需求级去重也可放 eDSL，但企业入口即可闭合。
3. **fan-out 拒绝依赖企业 `buildPlan` 兜底**：**应上升 A2（eDSL）**。`compiler.telora` 的发布门是
   `complete && unauthorized==0 && unreachable==0`，**没有检查 fanOutOnly**——fan-out-only 目标
   只被 `diagnoseTargets` 诊断、不阻塞发布。按"路径验证通过才发布"的原设计，门条件应对称地加入
   `array.length(fanOutOnly) == 0`。本轮在 A3 无法改 A2 的库，故保留企业兜底
   （`buildPlan` 用企业自己的 `fanOutRelations` 拒绝扇出粒度目标），并明确把门修复作为 A2 建议。
4. **导出面过大**（`buildPlan` 等可被直接调用绕过 `compile`）：**企业层解决**。导出收敛为
   `export { compile }`，`buildPlan`/`isGranularityViolation`/能力目录/关系目录全部变为模块内部。

## 对 eDSL 可用性的判断

- **优点**：单一入口 `compile_with`；选择器即契约，企业只需要填充封闭类型、能力目录、
  关系事实、需求派生、授权列表与构建器；诊断先于发布门，拒绝原因不会丢失。
- **锐边**：推断对导入方式非常敏感（选择性导入即退化）；每个回调都要完整签名锚定；
  fan-out-only 目标不阻塞发布门，策略必须下沉到 `buildPlan` —— 这要求模型作者先读
  `compiler.telora` 的门实现才能正确建模，教程 §11 的门描述与实现略有出入。
- **总体**：对"企业知识 vs 编排逻辑"的边界划分清晰，企业侧工作量小；但成功的建模强依赖
  对库推断边界和门实现的准确理解。
