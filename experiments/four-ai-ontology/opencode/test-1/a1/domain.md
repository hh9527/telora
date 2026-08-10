# 航班运营分析平台 — 领域题面（Stage 3）

你是这个航班运营平台的首席建模工程师。请根据以下业务知识，用共享的 ontology eDSL
（见 `a2/EDSL_TUTORIAL.md` 与 `a2/AI3_CONTRACT.md`）定义企业本体，并写出验证文件。

## 业务背景

一家中型航空公司需要运营分析报表：按航线、机型、航空公司维度查看航班量与客运量，
并能在需要时按座位粒度分析登机情况。数据来自航班计划系统与登机系统。

## 领域概念

### 实体（Entity）

- `Flight`：一次航班（一个具体的起降任务）
- `Aircraft`：执飞机型实例（一架飞机）
- `Route`：航线（起飞机场→降落机场的固定配对）
- `Airport`：机场
- `Airline`：航空公司（本平台只含本公司，但保留概念）
- `Seat`：航班上的一个可售座位（登机粒度的最小单位）

### 度量（Measure）

- `FlightCount`：航班数。语义：航班量；自然粒度：`Flight`；聚合：计数。
- `Boardings`：登机旅客数。语义：按座位实际登机的旅客数；自然粒度：`Seat`；
  聚合：计数（每个座位最多一名旅客）。

### 维度（Dimension）

- `RouteOrigin`：航线起点。由 `Route` 提供，其值为 `Airport`。
- `AircraftType`：机型。由 `Aircraft` 提供，其值为机型标识（用 `Aircraft` 实体表示即可，
  不必引入 `Type` 实体）。
- `AirlineName`：航空公司名。由 `Airline` 提供。

### 关系（Relations）

安全关系（多对一，粒度不扩张）：

- `Flight` -> `Route`：一次航班属于一条航线
- `Route` -> `Airport`：一条航线有一个起点机场（用 `Route -> Airport` 表达起点）
- `Flight` -> `Aircraft`：一次航班由一架飞机执飞
- `Aircraft` -> `Airline`：一架飞机属于一家航空公司

扩张关系（一对多，会扩大粒度）：

- `Flight` -> `Seat`：一次航班有多个座位。**按 `Seat` 粒度聚合度量时，从 `Flight`
  出发会经过这条关系（粒度扩张，应被拒绝或要求预聚合）。**

### 物理映射

每个实体对应一张表（表名与主键你自己命名，但必须与实体一一对应）：
Flight、Aircraft、Route、Airport、Airline、Seat 各一张表。关系通过外键表达。
度量在 SQL 层面用简单计数/求和即可（例如 `COUNT(flights.id)`）。

## 期望的报表

以下报表必须是**合法**的（应产出计划）：

1. 航班量（按 `Flight` 粒度）
2. 航班量按航线起点（`RouteOrigin`）分组
3. 航班量按机型（`AircraftType`）分组

以下报表是**非法**的（应被拒绝并给出诊断）：

1. 请求一个本平台不存在的度量（如 `Revenue`）——缺失能力
2. 请求登机旅客数（`Boardings`，自然粒度 `Seat`）却未提供预聚合——粒度扩张（fan-out）
3. 请求一个未获授权访问的实体（例如假设本平台 `Airline` 属于外部数据源，未在授权范围内）——未授权

## 你的交付

在 `a3/` 下创建：

- `a3/enterprise-model/`：企业模型（Telora 源文件，含类型、capability、关系、构建器）
- `a3/valid.telora`：一个合法报表的顶层验证文件（`export let output = ...`）
- `a3/invalid.telora`：一个非法报表的顶层验证文件
- `a3/PUBLIC_INTENT.md`：给未来查询方（A4）看的**公开意图面**——说明该平台能回答哪些
  业务问题、合法组合方式、以及顶层编译入口的形状。**不得**包含表名、列名、连接谓词、
  SQL 或物理计划构造细节。
- `a3/STAGE3_NOTES.md`：你的建模决策、遇到的坑、对 eDSL 可用性的判断。

所有企业代码必须通过共享 eDSL 表达，不得复制 eDSL 的编排逻辑；只表达企业知道的
私有知识（类型、能力、关系事实、物理映射、策略、构建器）。
