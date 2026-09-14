# RFC 0297 / #193 验收记录

日期：2026-09-15。实现分支：`fix/193-directional-fit`，代码版本 `21a60d6d`。
基线：main `5f481180`。真实语料：lab-ontology `766f11d`。

## 语义与证据

| 要求 | 已核验的实现与用例 |
| --- | --- |
| 所有普通顶层值完整签名，私有/导出一致 | `mir/declaration_contracts.rs` 枚举模块顶层声明；`diag-top-level-contract` 覆盖标量、容器、元数据、函数、直接/改名导出；fn 内注解不能替代绑定签名 |
| 注解先于值证据闭合 | `type_resolve/contracts.rs` 在生成值约束前求解注解及类型骨架，保存 ready 结果；`export-contracts.telora` 检查最终 Known 不会豁免缺失契约 |
| 不完整/未解析契约拒绝 | `diag-top-level-contract-hole` 检查语法不允许的 `_` 注解；`diag-top-level-contract-unresolved` 保留类型别名的原 resolve 错误；seal 独立重验契约义务 |
| 局部推导及显式模板保持可用 | `test/type-inference`、`mir-static-contracts`、core generics/bounds 测试；std 全图契约检查并 seal 成功 |
| 模板引用独立实例化，普通值别名单次物化 | `template-materialization.telora`、`template-value-binding.telora`；核验独立引用的实例身份、闭合实例复用、shadow 后混用 Int/String 拒绝 |
| 值失配不污染共享契约 | `diag-shared-contract` 与 `diag-contract-local`；检查共享 Fn(Int)->Int、健康 Int/String 类型、两个根因、错误实现及泛型混合实参 |
| 特殊错误/既有 Conflicted 不覆盖契约 | `contract-error-boundaries.telora` 检查 TypeOf 见证、诊断参数、调用 arity 与失败值消费者；未知引用保留原失败，注解保留 Known |
| 模块清单、root、使用顺序不改变结论 | `rejected_uses_preserve_shared_contracts_and_independent_diagnostics` 反转清单、切换 root，并使用 `bad-reversed.telora` 反转证据顺序 |
| 精确静态来源 | Fit 保存来源注解，延迟生产者按语法使用保存契约；失败记录保存 contracts 与使用 HIR，secondary 指向原契约，交换顺序仍成立 |
| 静态错误保留 query 信息但禁止发布 | query 的 failed_constraints 与 Known 分开；core 测试清除诊断和契约记录后仍拒绝 seal；CLI 静态错误不创建 VM |
| 初始化归因 | `initialization-origin` 用例核验 primary 在 shared、subject 在 bad、initialization 为 bad.wrong，健康导入者不被归因，缓存失败不重复报告 |
| 重导出、构造、模式、property 与生成代码 | 语言 module-interfaces、enum/newtype/record/tuple、pattern、property/trait、run/serve/test 及 source 输入回归通过 |
| 文档及外部迁移 | README、docs、guide、examples 的顶层签名已迁移；独立 examples workspace 全库检查通过；ontology 真实测试及查询执行通过 |

当前表面语法不允许顶层 let 或模式绑定；本 RFC 不新增这些语法。类型参数显式绑定
不等于 Unknown；未使用模板仍接受带绑定参数的检查，不强制物化为运行时值。

## 测试

- `CARGO_INCREMENTAL=0 cargo test --workspace` 通过，包括 CLI 集成、语言 harness、
  各库测试及 doc-tests。随后新增的契约覆盖/顺序断言以完整 core 测试再次验证：149 项通过。
- release 语言 harness：427 项通过；原 workspace 运行时的 424 项加三个契约负例。
- Wasm 75 项、CLI 非语言 100 项均通过；source-size 和 diff 检查通过。
- 最终资产审计另迁移 LSP 健康源码、query 签名/记录上下文、性能夹具及 smoke 生成器；
  telora library 28 项、两个 query 模块的纯类型检查、性能夹具全库纯类型检查、
  生成器 test smoke 和最终语言 427 项通过。解析/恢复及其他预期失败资产保留其负例，
  不能把这些只验证前置阶段或失败诊断的片段当作健康模块。
- world-model / spider-model / dog-model 的 query：22 / 28 / 42 项通过。
- ontology：construction-rules 13、model-rules 10、intent 24、ontology 129、query 239，
  共 415 项正常测试通过；原诊断验证脚本核验 6 项预期失败的消息、规则模块与精确 subject。
- world-model 真实 `eval-with @src/bin/make-query:main --source input=...` 输出
  `SELECT c.Name FROM country AS c WHERE c.Continent = ?`，bindings 为 `["Asia"]`。
- 模型 query 使用 `--with-fuel 1000`；大型 ontology 测试使用 `--with-fuel 3000`。
  未修改默认配额。

## Release 对比

main 从 `git archive 5f481180` 独立导出并重建 release；分支使用上述实现版本的
release binary。两者均检查同一份已迁移语料。无并行测试/编译干扰，每命令 warmup 3 次、
测量 12 次，hyperfine 记录墙钟时间。RSS 为另外一次 `/usr/bin/time` 运行的峰值，
不是多次采样均值。

命令：`telora -C <lab-ontology>/<project> check --lib [--only-types]`。

| 语料/阶段 | main 均值±标准差 ms | 分支均值±标准差 ms | 时间变化 | main / 分支峰值 RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| ontology 纯类型 | 384.5 ± 5.9 | 417.8 ± 10.9 | +8.6% | 42.1 / 45.2 |
| ontology 完整 | 570.1 ± 13.2 | 614.5 ± 13.5 | +7.8% | 72.1 / 71.3 |
| world-model 纯类型 | 405.7 ± 20.4 | 436.8 ± 13.5 | +7.7% | 42.9 / 45.6 |
| world-model 完整 | 608.0 ± 7.7 | 646.1 ± 13.9 | +6.3% | 69.1 / 70.8 |

存在可观测的时间回退，不能称为性能优化。world-model 基线纯类型包含离群样本；
完整检查的单次 RSS 小幅涨跌不作稳定内存收益结论。新增契约预求解、保护标记和来源记录
增加了静态工作与存储；本次测量没有将各部分成本单独归因。

原始输出、两个二进制、main 导出源码及复现脚本保存在本次机器的
`/tmp/telora-193-final-perf-cS4Yly`；这里的表格是随仓库保存的验收数据。
