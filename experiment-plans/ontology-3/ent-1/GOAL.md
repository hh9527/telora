# A3 目标：建立物流 EnterpriseKnowledge

作为企业知识作者，使用 ontology eDSL 把 `ent-1/DOMAIN.md` 表达为 nominal entity
structs、member/type properties 与一个显式 ontology root。用合法和非法 Request 验证
完整链路：

```text
prepared EnterpriseKnowledge + Request -> Plan -> Query
```

不得查看上游私有实现，不得在企业 package 中重写 eDSL、Plan 或 QueryBuilder。

## 完整输入清单

固定路径输入（内容属于当前发布轮次）：

- `bin/telora`
- `docs/TELORA.md`
- `docs/TELORA-CLI.md`
- `ent-1/GOAL.md`
- `ent-1/DOMAIN.md`
- `ent-1/telora-deps.json`
- `ent-1/FEEDBACK.md`：Host 所有的私有模型反馈；仅在
  `ent-1-model-feedback` 发布后作为输入读取，A3 不得修改；
- A3 自己已经产生的 `ent-1/src/**` 与 `ent-1/tests/**`

动态输入由 `oc-task` 按任务依赖开放：

- `qb-feedback.a3`：读取 QueryBuilder 公共候选，依据 DOMAIN 完成能力审查并
  写入 `ent-1/QUERY-BUILDER-FEEDBACK.md`，但不开始最终建模；
- `edsl-feedback.a3`：读取 eDSL 公共候选，依据 DOMAIN 完成能力审查并写入
  `ent-1/EDSL-FEEDBACK.md`；
- `ent-1-model.a3`：在 `qb` 与 `edsl` 均发布后，读取 QueryBuilder 和
  ontology 公共教程与契约并开始最终建模。

不得读取 `query-builder/{GOAL,DESIGN,NOTES,src,tests}` 或
`ontology/{GOAL,DESIGN,NOTES,src,tests}`。
任务就绪与重跑由 `oc-task` 根据文件时间戳确定。

## 私有模型交付物（`ent-1-model.a3`）

- `ent-1/src/model.telora`：封闭 public vocabulary、nominal entity structs、字段/类型
  properties、显式 ontology root、PlanProfile，以及准备后的 EnterpriseKnowledge；
- `ent-1/src/bin/main.telora`：合法 Request 得到完整 Plan 和 Query；
- `ent-1/src/bin/verify.telora`：验证 Plan 覆盖、领域筛选 capability、未投影筛选路径、
  排序、Top N、非空 bindings、profile 约束和 Query 确定性；
- `ent-1/src/bin/invalid.telora`：非法场景产生 Host 诊断且无 Plan/Query；
- `ent-1/tests/logistics.telora`：公共契约检查；
- `ent-1/UPSTREAM-FEEDBACK.md`：按 QueryBuilder/eDSL 分区记录具体使用摩擦；
- `ent-1/QUERY-BUILDER-FEEDBACK.md`：最终建模前对 QueryBuilder 草案的能力审查；
- `ent-1/EDSL-FEEDBACK.md`：最终建模前对 eDSL 草案的能力审查；
- `ent-1/NOTES.md`：模型选择、验证结果和风险。

## 公共查询面交付物（`ent-1-query-surface.a3`）

- `ent-1/src/query.telora`：从同一份私有 ontology root/property 形成的公共 facade，只导出业务
  vocabulary、typed Request 与 `lower(Request) -> Query`；
- `ent-1/src/bin/query-surface.telora` 与 `ent-1/tests/query-surface.telora`：使用不针对
  隐藏意图的通用 typed Request，实际验证 facade 可加载、lowering 确定且结果可编码；
- `ent-1/QUERY-DESIGNER-TUTORIAL.md`：不懂物理模型的查询设计者可独立使用的教程；
- `ent-1/PUBLIC-QUERY-CONTRACT.md`：公共词汇、Request 形状、组合/grain/capability
  规则、筛选输入、排序/Top-N 约束、失败语义和 lowering 保证。

不得修改 `ent-1/telora-deps.json`。不得复制共享算法、定义替代 Plan、用任意 builder
手工组装 Plan、使用预渲染 SQL、平行 entity/attribute/relation 事实目录或 String 语义
身份。公共边界不得使用 `Any`/`Dyn` 擦除类型。

公共 Request 必须让查询设计者表达 DOMAIN 授权的 `OrderMonth`、`CustomerTier`、
`OriginRegion`、`ProductCategory` 有类型筛选，以及只引用已请求投影的排序和正整数
Top N。公共 JSON/codec 形状必须可由下游直接使用；不能要求下游提交 TypeId、物理
表达式或 QueryBuilder 节点。

公共查询面必须从私有 ontology root 的已发布 property 捕获 prepared lowering，不能
手工维护第二份领域知识。公共
文档和 facade 不得泄漏表名、列名、alias、join 路径/mapping、SQL 片段或模板。A3 永远
不得读取 `intent-1/INTENT.md`；公共教程不能针对具体隐藏意图预写 Request 答案。

合法入口必须分别保留并验证 Plan 与 Query：

```text
plan  = prepared_lower(ontology_root)(request)
query = query_builder.transform(plan)
```

## 验证

```text
./bin/telora run main -C ent-1
./bin/telora run verify -C ent-1
./bin/telora run invalid -C ent-1 --best-effort
./bin/telora check @test/logistics.telora -C ent-1
./bin/telora query exports @bin/main.telora -C ent-1
```

完成时报告真实结果与具体反馈，不要求 Git commit。

Host 需要修订私有模型时，写入 `ent-1/FEEDBACK.md` 并发布
`ent-1-model-feedback`；输入时间戳会使旧提交失效，A3 自动重新领取、修订和提交。
Host 审核私有模型并发布 `ent-1-model` 后，`oc-task` 才会返回
`ent-1-query-surface.a3`。完成公共 facade、教程和契约及其验证后，用一次
`oc-task submit a3 ...` 提交本次 pull 返回的唯一 artifact。私有模型与公共查询面是两个独立 Host
审核边界，但都由同一个 A3 session 和同一份 EnterpriseKnowledge 维护。

公共面阶段至少实际运行：

```text
./bin/telora run query-surface -C ent-1
./bin/telora check @test/query-surface.telora -C ent-1
```

## 上游更新复验

后续发布轮次的 `ent-1-model.a3` 就绪后，重新读取两层公共交付，保持同一题面和验收
场景，适配批准更新并重新验证；更新 feedback 与 notes，不推测上游私有实现。完成后
提交对应的 `.a3` artifact。
