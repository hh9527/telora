# RFC 0315 验收记录

所有构建均使用 `/tmp` target，未替换消费方工具。验收日期：2026-10-23。

## 全量与增量边界

全 workspace（独立运行语言 fixtures）通过：

```sh
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target CARGO_BUILD_JOBS=4 \
  cargo test --workspace --offline -- --skip language_acceptance_fixtures_pass
TELORA_BIN=/tmp/telora-rfc0312-target/debug/telora \
  TELORA_LANGUAGE_BUILD_ROOT=/tmp/rfc0315-language bash scripts/test-language.sh
```

语言 fixtures 441/441。workspace 包括 CLI、core、data、Wasm、shared 与 doc tests。
该轮早于最终故障注入及 Guest 内存失败分类补强；后续以针对性回归验证，
不将先前全量结果冒称为最终新增测试也已全量执行。

```sh
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target cargo test -p telora-wasm-shared --offline
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target cargo test -p telora-wasm --lib --offline runtime_allocation
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target cargo test -p telora-wasm --lib --offline transform_service
```

shared 35/35：边界、内部指针、独立地址、环共享 oracle、失败所有权、资源释放。
runtime_allocation 6/6：源 actor/pin 不变、分配历史不进入镜像、初始化垃圾排除、
共享 property/actor 与递归闭包、provider 不重跑、恢复后普通强制 GC、
截断及旧 ABI 拒绝、溢出/managed/Guest 总内存失败分类与终止实例。
transform_service 8/8：普通根跨请求、初始化与强制 GC、输入来源、诊断、
fuel/内存 trap 后从快照恢复。

## 产品路径

```sh
TELORA_BIN=/tmp/telora-rfc0312-target/debug/telora \
  TELORA_RUN_BIN=/tmp/telora-rfc0312-target/debug/telora-run node scripts/build-run-smoke.mjs
CARGO_TARGET_DIR=/tmp/telora-rfc0312-target cargo test -p telora --test cli --offline \
  collection_http_transport_routes_and_reports_missing_endpoints
```

普通 build、snapshot、源码、独立 runner 的 smoke 输出一致；HTTP 端点路由测试通过。
最终 CLI entry_services 回归 11/11（单独已验收的 HTTP 测试不重复运行），覆盖初始化预算与请求预算、资源耗尽恢复、源码/普通制品/快照输出。debug 事件消费游标另有针对性回归。

## 静态检查

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path rfc/0315/runtime-probe/Cargo.toml -- --check
git diff --check
bash scripts/check-source-size.sh
```

规模检查只有既有 `type_resolve/tests/generics.rs` 1515 行 review 提示。

## 失败与修正记录

- 沙箱禁止 HTTP 临时监听，隔离测试经授权在可监听环境复验；不是产品失败。
- 显式 copier 曾漏保留反馈根，已修正并覆盖诊断/debug。
- 恢复重定位曾消耗请求预算，改用初始化预算；下一请求保留完整请求预算。
- 正常 reset 与 trap 恢复的 debug 游标分别修正，防止漏事件或重复旧事件。
- 10,000 次周转保留规模，提高 fuel 测试预算至 40 亿；对照回退见性能报告。
- 增长序列原 256 KiB 用例先耗尽 fuel，配额停止测试采用 64 KiB，
  仅证明限额停止，不能视为原性能等价。

静态图免扫描与新尾坐标用途均延期；当前以普通 GC 与长期根证明保活。

## RFC 完成项对照

|要求|实现/验收证据|
|---|---|
|独立分配、Layout 配对释放、范围/内部指针、失败原子性|`telora-wasm-shared/src/direct_heap.rs` 及其 tests；登记失败与存储失败分别注入|
|准确非移动 GC、闭包/循环/Dict/Raw/Single/部分输出|`rt/gc_mark.rs` 与 `rt/collect_trace.rs` 共用 `trace`；`root_lifetimes`、`sequences`、`small_arrays`、`content`、`inline_aggregates` tests|
|自动关闭/常规/强制 GC|`forced_collection_preserves_codec_parse_format_and_sequence_results` 对比 interval=0/1；常规 workspace 与语言 fixtures|
|pin 生命周期与资源仅释放一次|`host_pins_follow_collection_and_reject_stale_or_foreign_capabilities`、captured-resource tests、shared direct_heap tests|
|正常 reset 清调用状态而非存储截断|`actor_roots_survive_requests_and_garbage_is_collected_without_truncation`、`dynamic`/`transform_service`、CLI 资源失败交替请求|
|已发布值及长期边保留解释|`content` 的 Raw 视图/分叉、`small_arrays` 的分支 push、`sequences` fold reuse tests|
|trap/fuel 后禁止重入与快照恢复|`Session::ensure_live`、`transform_service` 恢复测试、CLI request fuel 与 memory exhaustion tests|
|发布根、共享/环、property/provider、actor、初始化垃圾|`runtime_allocation`；根图 fixture 显式共享引用和互递归闭包，warn 验证 provider 只执行一次|
|连续镜像独立所有权、源图不变|`image_packing::Packing` 保存/恢复源存储与根；导出前后 pin 地址/值及 actor 请求验证；额外分配历史下镜像字节相同|
|镜像恢复后普通 GC、数据模块与跨边|`runtime_allocation` 强制 GC；`data::compiled_data_roundtrip_preserves_values_without_source_text`；CLI publication 模块及静态来源恢复|
|ABI/transport/output 同步|ABI 38、`heap`/`output`/`transport` 接入；CLI 旧协议拒绝；错误 ABI/静态区坐标/extent/截断/尾随字节镜像拒绝并使实例终止|
|低 managed/Guest 限额、溢出、分类、fuel 停机|`runtime_allocation`、`root_lifetimes` 配额与 collection fuel tests；typed growth trap 分类|
|源码/build/snapshot/runner/HTTP/JSONL 一致|workspace CLI、441 fixtures、build-run smoke、HTTP 端点测试|
|三轮分阶段性能与显著回退调查|`results.md`、两组原始 JSON、probe 与 provenance；未把静态图免扫描扩为正确性要求|
|当前文档与检查|CONCEPT/LANGUAGE/IMPLEMENTATION、GUIDE、统计 API 注释；fmt/diff/source-size|

异常镜像验收证明上述损坏类型稳定拒绝；快照仍绑定匹配 Wasm 制品，
不承诺任意第三方字节的通用反序列化验证或跨代码版本迁移。
