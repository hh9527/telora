# TRIALS — A4 查询意图试验与攻击判断

实验对象：`a3/enterprise-model/model.telora` 导出的 `compile: Fn(Array(Measure)) -> Option(Plan)`。
所有查询一律通过 `model.compile([...])` 发起，未直接写 SQL / 物理计划 / 表结构，也未绕过 compile 构造 Plan。

---

## Trial 1 — 合法直接（`trial1_legal_direct.telora`）

| 请求 | 预期 | 依据 |
|---|---|---|
| `['FlightCount]` | **lowered** | Flight 为基准粒度，安全可达、已授权 |
| `['FlightCount, 'RouteOrigin]` | **lowered** | Flight→Route→Airport 全安全边，均在 `allowed` |
| `['FlightCount, 'AircraftType]` | **lowered** | Flight→Aircraft 安全边，已授权 |

目的：验证 PUBLIC_INTENT 直接列出的三类典型业务问题都能降级成计划。
攻击者判断：均符合公开意图面，无异常。

## Trial 2 — 合法新奇组合（`trial2_novel_combo.telora`）

| 请求 | 预期 | 依据 |
|---|---|---|
| `['FlightCount, 'RouteOrigin, 'AircraftType]` | **lowered** | 双维度×度量，粒度全安全可达 |
| `['RouteOrigin, 'AircraftType]` | **lowered** | 仅维度组合，粒度安全 |
| `['AircraftType]` | **lowered** | 单维度合法 |
| `['AircraftType, 'FlightCount]` | **lowered** | 顺序无关，组合规则不变 |

目的：验证公开概念可自由组合，未逐条列出但仍合法。
攻击者判断：组合面比 PUBLIC_INTENT 列出的更宽，但全部落在安全边内，合理。

## Trial 3 — 未知能力（`trial3_unknown_capability.telora`）

| 请求 | 预期 | 依据 |
|---|---|---|
| `['Revenue]` | **refused（缺失能力）** | `Measure` 枚举含 Revenue 但能力目录无对应 Cap → `complete=None` |
| `['FlightCount, 'Revenue]` | **refused（缺失能力）** | 原子发布：任一缺失则整体不发布 |
| `['RouteOrigin, 'Revenue]` | **refused（缺失能力）** | 同上 |

攻击者判断：边界牢固。`Revenue` 无法通过枚举外 tag 混入（枚举封闭）；枚举内唯一无 Cap 的身份即触发缺失能力诊断，且缺失项会拖累整次编译。

## Trial 4 — 粒度不匹配（fan-out）（`trial4_granularity_fanout.telora`）

| 请求 | 预期 | 依据 |
|---|---|---|
| `['Boardings]` | **refused（粒度扩张）** | At(Seat)，Seat 仅经 fan-out（Flight→Seat）可达 |
| `['FlightCount, 'Boardings]` | **refused（粒度扩张）** | Seat 混入 |
| `['Boardings, 'RouteOrigin]` | **refused（粒度扩张）** | 安全维度无法"治愈"Seat |
| `['Boardings, 'AircraftType]` | **refused（粒度扩张）** | 同上 |

攻击者判断（**关键发现**）：库门 `compile_with` 的发布条件是
`complete && unauthorized==0 && unreachable==0`，**fan-out-only 目标只被诊断、不阻断门**。
Seat 能被拦截，纯粹是因为企业模型在 `buildPlan` 里用 `isGranularityViolation` 兜底。
这是"门实现与企业兜底"两层才能拦住边界的结构——若企业漏写该检查，Seat 粒度会直接漏成 lowered。本次试验确认兜底生效。

## Trial 5 — 未授权实体（`trial5_unauthorized_entity.telora`）

| 请求 | 预期 | 依据 |
|---|---|---|
| `['AirlineName']` | **refused（未授权）** | At(Airline)，Airline 不在 `allowed` |
| `['FlightCount, 'AirlineName']` | **refused（未授权）** | 原子发布 |
| `['AircraftType, 'AirlineName']` | **refused（未授权）** | 同上 |
| `['RouteOrigin, 'AirlineName']` | **refused（未授权）** | 同上 |

攻击者判断：Airline 在安全图中可达（Flight→Aircraft→Airline），所以不会被误诊为不可达/扩张，
而由 `validateAllowlist` 精确诊断为未授权。边界正确。

## Trial 6 — 不可能请求（`trial6_impossible.telora`）

| 请求 | 预期 | 依据 |
|---|---|---|
| `['Revenue, 'Boardings, 'AirlineName']` | **refused** | 缺失能力 + 粒度扩张 + 未授权三重叠加 |
| `['Boardings, 'AirlineName']` | **refused** | 扩张 + 未授权 |
| `[]`（空数组） | **lowered（空粒度计划）** | 见下 |

攻击者判断：前两例语义上根本无法成立，拒绝正确。**但空请求数组会返回
`'Some({report:..., granularities: []})`**——`complete(0==0)` 为真、无任何 target，
发布门全绿，`buildPlan` 直接成功。这不会授予任何数据，但说明"发布计划"不要求
"至少一个能力意图"。属边界气味（见下）。

## Attack probes（`attack_probes.telora`）

| 请求 | 预期 | 判断 |
|---|---|---|
| `[]` | **lowered（空粒度计划）** | 最小缺口：空请求也发布空计划 |
| `['FlightCount, 'FlightCount']` | **lowered（重复粒度）** | 不去重，granularities 出现重复 Flight，鲁棒性小缺口 |
| `['AirlineName, 'AirlineName']` | **refused（未授权）** | 重复不救未授权，原子发布正确 |
| `['Boardings, 'FlightCount']` | **refused（粒度扩张）** | 验证企业 buildPlan 兜底确实拦截 Seat |

---

## 总结：可绕过点 / 边界判断

1. **（实）空请求数组被放行**：`compile([])` 返回 `'Some` 空计划。不构成数据泄露，
   但发布门不校验"请求非空"。若下游把"空粒度计划"误解为"无限制范围"，理论上有风险。建议企业拒绝空数组。
2. **（实）库门不阻断 fan-out-only 目标**：阻断完全依赖企业 `buildPlan` 的
   `isGranularityViolation`。这是一层很薄的企业兜底；模型作者一旦改动 `buildPlan` 或遗漏检查，
   `Boardings` 会静默漏出。攻击者层面：当前无法绕过，因为请求无法改变关系目录。
3. **（观察）重复请求不去重**：`['FlightCount, 'FlightCount']` 产生重复粒度，非安全绕过。
4. **（理论、未利用）`buildPlan` 等门函数被 export**：企业把 `buildPlan`、`isGranularityViolation`
   也导出了。理论上可绕过 `compile` 直接调 `buildPlan` 构造 Plan 值，但受"必须走 compile"的硬约束，
   本试验未使用。建议企业收紧导出面，只留 `compile` 与公开类型。
5. **不可达类拒绝无法触发**：本模型所有实体（含 Seat/Airline）都可从 Flight 经某个关系目录到达，
   因此"unreachable"诊断在查询侧不可达——未授权与扩张已覆盖所有越界形态。

边界总体判定：**不可绕过**（在硬约束下）。真实缺口是空数组放行与门函数的过度导出。
