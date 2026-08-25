# A1 目标：实现 QueryBuilder

使用 Telora 实现 `query-builder/DESIGN.md`。这是领域无关的基础 package，负责标准
Plan 算子、PlanProfile、Plan 验证和确定性的 SQLite `Plan -> Query`，不得包含
ontology 概念或企业领域事实。本轮只实现 SQLite，不扩展其他 SQL 后端。

## 完整输入清单

固定路径输入（内容属于当前发布轮次）：

- `bin/telora`
- `docs/TELORA.md`
- `docs/TELORA-CLI.md`
- `query-builder/GOAL.md`
- `query-builder/DESIGN.md`
- `query-builder/telora-deps.json`
- A1 自己已经产生的 `query-builder/src/**` 与 `query-builder/tests/**`

动态输入：

- `qb-feedback` artifact 与 `query-builder/FEEDBACK.md`：Host 从
  `qb-feedback.a2/.a3` 原始审查中筛选、整合并发布的反馈。首次实现时可以不存在；
  更新后 `oc-task pull a1` 会重新返回 `qb.a1`。

不在清单中的文件不是 A1 输入。任务就绪与重跑由 `oc-task` 根据文件时间戳确定。

## 交付物

- `query-builder/src/`：可复用 package；
- `query-builder/src/bin/main.telora`：展示合法 Plan 到参数化 SQLite Query；
- `query-builder/src/bin/verify.telora`：验证 profile 覆盖、顺序与确定性；
- `query-builder/src/bin/invalid.telora`：展示越界或非法 Plan 产生 Host 诊断且无 Query；
- `query-builder/tests/query-builder.telora`：公共类型与模块契约检查；
- `query-builder/QUERY-BUILDER-TUTORIAL.md`：A2/A3 可独立使用的教程；
- `query-builder/PUBLIC-CONTRACT.md`：稳定公共类型、能力与保证；
- `query-builder/NOTES.md`：设计选择、验证结果和限制。

不得修改 `query-builder/telora-deps.json`。公共 API 不得使用 `Any`、`Dyn`、native
声明或 String 语义身份逃逸。运行时值必须进入 bindings。

## 验证

```text
./bin/telora run main -C query-builder
./bin/telora run verify -C query-builder
./bin/telora run invalid -C query-builder --best-effort
./bin/telora check @test/query-builder.telora -C query-builder
./bin/telora query exports @bin/main.telora -C query-builder
```

完成时报告真实交付、验证结果与剩余限制，不要求 Git commit。

## 反馈修订

`qb.a1` 首次运行时发布并验证可执行候选。Host `qb-feedback` 出现后，同一任务
重新就绪；完整读取 `query-builder/FEEDBACK.md`，修订实现、教程和契约并重跑全部验证。不得直接读取
企业或 ontology 的其他私有输入与源码。完成后用一次 `oc-task submit a1 ...` 提交本次
pull 返回的唯一 artifact。
