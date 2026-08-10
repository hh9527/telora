# Telora 查询意图最小教程（Stage 4 专用）

你只需要掌握如何写"查询意图"——即调用 A3 的企业编译入口，把一个业务问题降级成计划。
不需要掌握完整 Telora。

## 基本形态

一个查询意图是一个 Telora 文件，调用企业模型导出的 `compile`，把结果作为 `output`：

```telora
import "./enterprise-model/model.telora" as model;
export let output = model.compile([...请求的度量/维度名...]);
```

- `compile` 接收一个请求数组，返回 `Option(ExecutionPlan)`：
  - 合法请求 → `'Some({...})`（计划）
  - 非法请求 → 不会返回 Some；会得到一条带来源的诊断（缺失能力 / 粒度扩张 / 未授权）
- 请求名是企业的 `Measure` 枚举值（用 `'Tag` 形式，如 `'FlightCount`）。

## 语法要点

- 单引号是 Atom/Tag：`'FlightCount`
- 数组：`['A', 'B]`
- 注释：`# ...`
- 字符串插值用反引号：`` `text \{value}` ``

## 合法 vs 越界

- **合法**：请求的度量/维度都在企业能力目录里，且它们需要的实体都可通过安全关系从基准实体到达，且在授权范围内。
- **越界**（会收到拒绝诊断）：
  - 请求一个不存在的能力（缺失能力）
  - 请求一个需要通过"一对多扩张关系"才能到达的粒度（粒度扩张 / fan-out）
  - 请求一个未授权访问的实体

## 你不能做的

- **不得直接写 SQL、物理计划、表结构或绕过 `compile` 构造计划**——所有查询必须通过企业的
  `compile` 入口降级成计划。这是硬性约束，主 Agent 会验收。
