# 智能报表实验

这个目录是“意图编译器”讨论的第一个可执行切片。它描述一个包含十张表的
B2B 商业领域，接受接近业务语义的报表意图，校验组合是否合法，并用普通
Telora 代码将合法意图逐步 lowering 为 SQLite SQL。

建议先阅读 [DOMAIN.md](DOMAIN.md)，其中定义了物理模型、语义身份、关系、
度量 grain 和第一批合法组合。

## 当前状态

- RFC 0180：伞 RFC 已接受，确定五阶段实验路线；
- RFC 0181：已完成，领域 capability 产生结构化 SQL AST，由统一 renderer
  负责 SQL 字符串和 quoting；
- RFC 0182：已完成，关系 catalog 与双向闭包 planner 根据 base grain 和目标
  entity 自动选择、合并并去重 join；
- RFC 0183：已完成，many-to-one 安全路径与 one-to-many fan-out 路径分离，
  Product 维度对 Order grain 的拒绝由关系证明产生；
- RFC 0184：已完成，意图支持 filter、显式排序、limit 和 render mode，并形成
  `SemanticPlan → RelationalPlan → SqlPlan` 三个 typed lowering 阶段；
- RFC 0185：已完成，成功结果发布 typed `ExecutionPlan`，边界模块将其编码为
  带版本、方言、只读声明和输出模式的稳定 JSON；
- RFC 0180：全部五个子阶段已经完成。
- RFC 0186：新的诊断伞 RFC 已接受；RFC 0187 已完成 message-first variadic
  `blame!` 与 `raise!`；RFC 0188 已完成普通 `report` BIF、Info/Warn/Error
  事件及 Error 成功边界；RFC 0189 已完成 `emit_info!`、`emit_warn!`、
  `emit_error!` 与 `fail!` 便利形式；RFC 0190 已完成领域库迁移，删除显式诊断
  数组，并证明普通 `Option`、数组组合子与 Host 事件足以覆盖当前本体实验；
- RFC 0186：全部四个子阶段已经完成。
- RFC 0192：已完成递归类型元数据的跨模块发布；SQL 表达式已改为真正递归的
  `Expr`，同时保留有效 lowering 与四条独立诊断。
- RFC 0193：新的多指标报表伞 RFC 已接受；RFC 0194 已完成 measure semantic
  model，显式记录业务值类型、自然 grain 与 aggregation behavior。
- RFC 0195：已完成显式 grain alignment；`NetRevenue + UnitsSold` 只有在意图
  请求 `PreAggregate(Order)` 时才组合，`Natural` 不会隐式猜测策略。
- RFC 0196：已完成原子执行计划；SQL placeholder、typed parameters、result
  schema 与经过核对的 render fields 由同一 lowering 链生成；RFC 0193 已完成。
- RFC 0197：可复用领域编译方法伞 RFC 已完成。RFC 0198 抽取最小跨行业组合子，
  RFC 0199 抽取 analytics 行业方法，RFC 0200 用 GCC wrapper 验证 toolchain 行业
  方法，RFC 0201 证明普通外部 restriction 数据可以参与 lowering，并保留跨来源
  诊断。共享层保持很小，没有伪装成通用 ontology framework。
- RFC 0202：ontology 定义方法复用实验已启动；RFC 0203 证明普通函数可以生成
  跨模块严格检查的 capability 类型，RFC 0204 抽取 typed capability、关系和
  restriction 方法，RFC 0205 已将本目录明确拆为共享 ontology 方法与具体 B2B
  模型，既有 SQL、诊断和 wire plan 保持不变；RFC 0206 已由独立十二表 B2C
  模型验证同一方法层，RFC 0202 全部完成。
- RFC 0207：高阶 ontology 规则抽象已完成。RFC 0208 让 B2B/B2C 共同使用
  Measure、Dimension、Relation 与 Compilation 类型族；RFC 0209 共享完整路径
  分类和诊断规则；RFC 0210 共享 capability 编译协议；RFC 0211 对实际复用范围
  做了逐项审计，没有把单方或 fixture-only API 算作已证明能力。

## 文件

- `schema.sql`：十张 SQLite 表及确定性测试数据；
- `DOMAIN.md`：文字形式的本体和业务规则；
- `sql.telora`：最小 SQL AST 与 SQLite renderer；
- `relations.telora`：关系 catalog、可达性分析和 join 路径规划；
- `execution.telora`：Host-facing typed plan 与显式 wire encoding；
- `b2b-model.telora`：B2B 领域类型、指标、维度、物理映射、校验和 lowering；
- `../ontology-method/src/types.telora`：可复用的 TypeMetadata 类型构造器；
- `../ontology-method/src/ontology.telora`：typed capability、关系与授权组合方法；
- `restriction.json`：允许全部报表 entity 的普通外部 restriction；
- `restricted.json`：只允许 Order entity，用于跨来源诊断回归；
- `valid.telora`：按月份、客户区域统计净收入；
- `valid-units.telora`：按月份、品类、SKU 统计销量；
- `valid-multi.telora`：显式将销量预聚合到 Order grain 后，与净收入共同输出；
- `valid-multi-sql.telora`：导出多指标 SQL，供 SQLite 结果回归；
- `invalid-alignment.telora`：证明不同自然 grain 不会被隐式组合；
- `invalid.telora`：一次暴露四个独立领域错误；
- `invalid-measures.telora`：拒绝多 measure 意图，不用任意 fallback 猜测依赖它的
  dimension 诊断；
- `invalid-restriction.telora`：同时拒绝未授权的 dimension 与 filter，并将诊断
  连接到意图、JSON restriction 和规则代码；
- `valid-sql.telora`：导出生成的 SQL，供 SQLite 执行；
- `host-plan.telora`：模拟 Host shape 核对并输出 JSON plan；
- `net-revenue.sql`：手写参考查询。

## 运行

```sh
cargo run -p telora -- check examples/intelligent-reporting/valid.telora
cargo run -p telora -- run examples/intelligent-reporting/valid.telora
cargo run -p telora -- run examples/intelligent-reporting/valid-units.telora
cargo run -p telora -- run examples/intelligent-reporting/invalid.telora
```

生成的 SQL 已直接送入 SQLite。净收入结果为：

```text
2026-01|East|10000
2026-02|West|12000
```

销量结果为：

```text
2026-01|Keyboards|KB-1|2
2026-01|Mice|MS-1|1
2026-02|Mice|MS-1|4
```

## 当前证明了什么

- Telora 库可以承载一个小型、可执行的领域本体和业务规则；
- 面向 Code Agent 的意图只包含 measure 和 dimension，不暴露表、join、CTE、
  支付/退款语义或 SQL 语法；
- measure 和 dimension 是静态检查的 enum，而不是开放字符串；
- capability record 将领域概念与 lowering 函数绑定；
- 高阶 factory 可以表达通用、特定 measure 和暂不支持的 dimension 家族；
- 校验与 lowering 是同一过程：合法 dimension 产生 grouping requirement，非法
  组合产生诊断，不需要平行的 Boolean 兼容矩阵；
- 各 dimension lowerer 独立运行，一次编译可以报告四个错误；诊断是 Host 事件，
  不再是领域函数返回值；
- capability 不再拼接 SQL，标识符和字面量只由 renderer 转义；
- measure 只声明 base entity 和自身语义需要的 entity，dimension 只声明目标
  entity；关系 planner 从 catalog 计算二者之间的最小相关 edge 集合；
- 多个 dimension 共享的路径只产生一次 join；
- relation 带有 cardinality；planner 区分安全可达、需要 fan-out policy 和完全
  不可达，诊断仍指向原始 dimension；
- filter 本身也声明所需 entity，因此会参与同一关系规划；排序只能引用已经
  选择的 dimension，limit 与 render mode 保留在 typed plan 中；
- filter requirement 同时产生 SQL placeholder 与 typed parameter，结果字段与
  projection 同序产生；render field 必须存在于派生的 result schema；
- semantic、relational、SQL 三个中间计划都是普通 Telora 值和显式函数边界；
- SQL AST 使用递归 `Expr` 表达调用、二元运算和聚合；跨模块运行时保留真实
  UpLink 图，用户态反射可通过 `'Ref` 与 `type_desc.resolve` 有限遍历；
- 成功编译只发布无权限的 `Option(ExecutionPlan)`；Host 可以静态核对 shape，
  再接收显式版本化 JSON。失败 lowering 得到 `None`，任意 Error 事件同时阻止
  evaluation 被发布为成功；
- 失败结果不发布 SQL，成功结果可以直接被 SQLite 执行。
- restriction 只是普通 JSON 输入；领域库负责解码和解释，成功计划显式记录其
  revision，Host 只负责执行前检查该 revision 是否仍然新鲜；
- recovery 与正式执行现在共享 persistent module roots，因此闭包、递归元数据和
  外部数据 provenance 不再经过会破坏 UpLink 的 legacy `Value` 往返。

## 上位验收框架

这个实验的重点不是构造一套本体系统，也不是逐项堆叠报表功能。本体只是可以
借由 Telora 代码表达、检查和 lowering 的领域知识；实验真正要验证的是 Telora
能否成为 Code Agent 与领域规则之间有效、可靠的编程接口：

```text
Code Agent 阅读 Telora 领域库
    -> 理解领域语境与可组合空间
    -> 生成表达具体意图的 Telora 代码
    -> Telora 领域库校验并 lowering
       -> 诊断：Code Agent 修复上一次生成的代码，再次尝试
       -> 成功：得到可执行且符合领域规则的执行计划
```

因此，规则代码本身也是提供给 Code Agent 的语境。它不应只在运行时给出一个
允许或拒绝的答案，还应让 Code Agent 通过类型、函数、数据结构和模块接口理解
领域中有哪些概念、它们如何组合、哪些前提会影响 lowering。诊断则是这个接口
的动态反馈部分，使 Agent 能够修正一次没有写对的 Telora，而不是退回到猜测
底层 SQL 或数据库行为。

智能报表只是为这条闭环提供足够真实的压力。实验中发现的表达障碍、诊断断点、
provenance 丢失、组合困难或计划保证不足，应当反过来推动 Telora 通用语言机制
和标准库能力，而不是用报表专用语法、编译器特例或 Host 侧平行规则掩盖。后续
扩展应从下面四个方面共同评估这条闭环。

这四项是观察路线质量的坐标，不是要求每个阶段全部达到的硬门槛。现实领域中
可能存在 Telora 暂时无法表达的规则、只能用较低阶形式表达的合理意图、不够
精确的诊断，或者仍需 Host 核对的产物。发现这些边界本身就是实验结果，不应
为了让案例显得完整而隐藏缺口、放宽测试或把 Host fallback 描述成 Telora 的
保证。每轮推进都应诚实记录：已经证明什么、没有证明什么、为何没有做到、
采用了什么 fallback，以及该边界对规则方、意图方和 Host 分别有什么影响。

单项未完全满足不自动意味着路线失败。更重要的失败信号是：真实场景反复要求
编译器特例或平行业务 checker；大量合理意图只能绕过领域库表达；诊断无法支持
有效修复；或者 Host 必须普遍重新解释和校验 Telora 已经声称可靠的计划。

### 1. 规则可表达性

规则定义方应当能够用 Telora 领域库定义他真正需要的约束与 lowering 语义，
而 Code Agent 应当能够通过阅读这些普通 Telora 定义理解领域语境，包括：

- 领域概念、semantic type、grain 与 aggregation semantics；
- relation、cardinality、drill、预聚合与 allocation policy；
- Context、授权、目标方言和资源边界带来的条件约束；
- 合法组合如何产生下一阶段计划，非法组合为什么被拒绝；
- 诊断类别、直接原因、规则位置和可用修复候选。

这里不仅要能表达静态的“允许/禁止”，还要能表达条件规则：只有当若干概念
采用明确的对齐策略，并且 Context 满足前提时，某种 lowering 才成立。规则应
主要由类型、普通数据和普通 transform 构成，并且只在 Telora 库中实现一次；
如果经常需要平行的 Boolean 兼容矩阵、Host 侧业务 checker、VM 指令或编译器
特例，就说明当前抽象仍然不足。

### 2. 合法意图完备性

当一个业务意图确实合理，并且领域库已经定义并授权其语义时，意图定义方应当
能够自然地用领域词汇生成 Telora 代码来表达它：

- 不接触物理表、join、CTE 或 SQL 语法；
- 不伪造领域库内部 descriptor；
- 不复制领域规则，也不依赖手写 SQL 逃生口；
- 表达规模接近业务意图本身，而不是接近最终执行计划；
- 能组合领域允许的开放集合，而不只是命中少量预制模板。

一个只会拒绝、却让大量合理意图无法表达的规则系统暴露了重要能力边界。对于
领域库声称支持的合法意图，完成 lowering 后应得到完整 Plan，不能留下
`plan = None` 且没有诊断的普通状态；暂不支持的合理意图则应被明确标记为能力
缺口，而不是被描述成业务上不合法。

### 3. 诊断可修复性

当意图不成立时，反馈应足以让意图定义方直接推进下一轮修改，而不需要阅读
生成 SQL、物理 schema 或领域库实现：

- 主位置指向具体高阶意图，secondary location 指向直接拒绝它的领域规则；
- 区分独立根因、级联结果、blocked 检查和资源中止；
- 一轮报告当前可靠事实能够确定的所有独立问题；
- 尽可能给出允许的候选、缺失前提或可选策略；
- 不用低阶 planner 或数据库错误替代领域解释。

衡量标准不是“存在错误消息”，而是 Code Agent 能否在不修改领域库、不绕过
规则的前提下，根据诊断修正上一次生成的 Telora 代码，并在下一轮显著接近
合法意图。

### 4. 产物可靠性

前三项保证规则可以定义、合法意图可以表达、非法意图可以修复；最后还要保证
一旦 Telora 接受这份意图，就确实产生 Host 可以执行、并且符合领域规则的可靠
计划。完整计划最终应将下列部分作为同一条 lowering 链的原子结果：

```text
SQL
  + Parameters
  + ResultSchema
  + RenderPlan
  + Assumptions / ContextRevision
```

完整实现时，它应保证引用全部 resolve、SQL placeholder 与参数一致、SELECT
输出与结果 schema 一致、render channel 只引用存在且类型适配的字段，并且不把
变量替换或业务决策留给 Host 猜测。Telora 不能保证数据库和网络永不失败，但
Host 不应在执行阶段才发现本可由领域 compiler 识别的结构或业务错误。如果
当前 Plan 仍需 Host 补充核对，wire protocol 必须显式表达这项责任，文档也不能
把它计入已经证明的产物保证。

### 当前覆盖

| 场景 | 规则可表达 | 合法意图可表达 | 非法意图可修复 | 产物可靠 |
|---|---|---|---|---|
| 单 measure、多 dimension | 已证明 | 已证明 | 已证明 | SQL 与基础 wire plan 已证明 |
| 非法 fan-out | 已证明 | 正确拒绝 | 已证明基础根因诊断 | 不发布计划 |
| 多 grain + 显式对齐策略 | Order 预聚合已证明 | NetRevenue + UnitsSold 已证明 | 缺失策略可修复 | 无 fan-out SQL 已证明 |
| 外部 restriction 与授权约束 | 基础 allow-list 已证明 | 授权内意图已证明 | 意图、JSON、规则三处来源已证明 | plan 记录 revision；新鲜度仍由 Host 核对 |
| SQL + parameters + result schema + render | 基础规则已证明 | 基础意图已证明 | 缺失 render field 可修复 | 基础原子计划已证明 |

后续 RFC 不应只陈述新增了哪些 planner 功能，还应说明它对这四项分别增加了
什么证据、留下了什么缺口，以及用了什么 fallback。最有价值的新场景，是能够
同时给规则表达、合法组合、错误修复和最终计划一致性施加压力的场景。

这也是该实验对 Telora 自身的产出：每个无法自然表达的规则、难以生成的合法
意图、不能支持修复的诊断或无法兑现的计划保证，都是语言机制与标准库路线的
具体输入。推进的目标不是让智能报表示例绕过这些问题，而是判断哪些缺口具有
跨领域价值，并用独立 RFC 将它们补入 Telora。

## RFC 0181 发现、RFC 0192 解决的边界

RFC 0181 最初发现：最自然的 SQL AST 是递归的，但递归类型元数据无法穿过
legacy module value boundary。实验当时使用 `SqlAtom -> SqlTerm -> SqlScalar ->
SqlSelectExpr` 的有界层级作为诚实 fallback，没有把它描述成通用表达式树。

RFC 0192 将既有 UpLink 模型延伸到了模块边界。当前 `sql.telora` 使用真正递归
的 `Expr`，调用、二元运算和聚合可以任意有限嵌套；普通 AST 值仍是有限、无环
的数据。program stage 使用权威 persistent graph，tool stage 只将递归 back-edge
有限投影为 `Any`。因此本实验已经证明递归元数据、构造器、renderer 与诊断规则
可以跨模块协作，但没有声称 legacy 静态 scheme 已获得完整递归 back-edge 精度。

## 尚未解决

这两轮伞 RFC 已完成，但它还不是通用查询规划器。当前 catalog 是有序、无代价
的有向关系集合，使用固定六轮闭包覆盖这个有界本体；它不在多条语义不同的
路径之间猜测。当前也
只有“保持 grain”与“fan-out”两级证明，尚未实现具体预聚合或 allocation
policy。后续阶段还需要：

- 预聚合与 allocation policy；
- drill 和更丰富的 chart/render 语义；
- 非位置参数、更多参数类型与 chart-specific channel 约束；
- 授权和 catalog 的显式 Context；
- provenance 穿过所有中间计划；
- CLI 失败输出完整呈现 Host 已收集的多条诊断；

诊断伞 RFC 还记录了一个不实现的远期扩展：由调用者显式使用
`call_with_diagnostics!(compiler(intent))`，在单个调用边界把子诊断重新数据化。
它将是 `interpreter!` 同级的受控内建语法，而不是普通函数或通用 effect
handler；只有嵌套意图编译器的真实需求足够明确时，才应另开 RFC 定义其类型和
Error 传播规则。

RFC 0190 已删除 `RequirementCompilation` 和诊断数组。当前实验没有证明需要
accumulation effect：可恢复的领域拒绝使用 `emit_error! + Option`，真正无法继续
的依赖链才使用 `raise!`。是否需要更细粒度恢复，应由新的真实场景重新举证。
