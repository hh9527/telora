# 基于初始化结果的运行期专门化

设计约定见 [RFC 0316](../rfc/0316-service-runtime-specialization.md)。

```text
M → 保守生成 A → 执行 A → A.result
M + A.result → B
```

M 包含同一程序的声明和函数；当前 A 是现有 Service 初始化，A.result 是类型化 snapshot。B 由同次构建中保留的代码、链接依赖和 A.result 派生，不是另写一个模型，也不经外部文件回灌。

## 状态与代码依赖

运行期根包含 Service handler 与请求 materializer。封闭布局扫描器精确遍历闭包及其环境，并跟踪对全局 demand 的读取。链接器从 direct-call 与 function-pointer relocation 构造生成代码依赖，覆盖普通闭包、生成 codec 和静态数据模块。

函数读取 Ready demand 时，运行期依赖的是它的值；依赖遍历在此停止，不继续保留求值代码。扫描该值中的闭包时，再递归保留其行为依赖。没有扫描任意整数来猜测函数指针。

B 保留原 TypeId、函数表槽位、堆引用和静态 demand 地址。Ready demand 的实现变成 getter，支持原有 caller-owned 输出缓冲约定；不可达生成函数体成为 trap stub。原始函数表、签名和元素段不重排，因此 snapshot 中的闭包仍有效。

`--snapshot=full` 保留全部 Ready 初始化根；专门化 publication 在 packing 时从运行期根出发，清除不可达 demand。packing 完成后恢复 A 实例的原始状态。

## 构建与验证

```sh
telora build @src/lib --snapshot -o runtime.wasm
telora build @src/lib --snapshot=full -o full.wasm
```

A.result 只在本次构建内部传递，没有独立缓存匹配、签名或重复模型校验协议。A 失败时沿用原有构建失败与原子发布流程。模型自检由程序选择是否放入初始化，语言层不认识 ontology、Dataset 或 transform。

使用 `--report-usage` 或 `TELORA_WASM_TIMINGS=1` 时，`telora.runtime-specialization/v1` 诊断报告生成及保留的函数数量、Ready getter、stub 数量和代码字节。`runtime_specialization` 计时属于 snapshot 阶段。依赖计划是编译器内部的类型化数据，不写入 Wasm 自定义段。RT 以一次导出返回快照、函数槽位和 Ready demand 索引，Host 不解析快照内部布局。

`ServiceBuild` 私有地持有编译字节和依赖计划；加载、初始化、封存后的能力借用同一个 build。调用方不能导入任意字节、快照和依赖根的组合。

CLI 回归覆盖路由、初始化外部源、静态数据模块、删去源文件后的运行、跨阶段 Type 身份、构造检查、重复请求，以及初始化失败不发布。ontology 的 JS 规模样本在 5／20／40 Dataset 上比较普通 snapshot 和 B 的完整响应。

## 当前边界

完整 RT、Wasm bootstrap、静态类型镜像和函数表槽位仍保留；这不是完整的索引压缩，也没有消除全部初始化支持代码。输入 codec、构造检查和错误响应仍是 B 的运行期依赖。

当前先生成完整 A，再专门化 B，因此节约主要体现在制品与加载阶段。只把 B 裁剪前移不能省去 A 必须生成的代码；构建提速需要单独缩小 A 的依赖闭包或优化实际耗时阶段。跨构建复用、任意入口 A/B、自定义跨阶段返回值仍未提供公共接口。


## 生成成本的评估

正式接口的 IC 验收构建中，前端（含类型检查）约 7.96 s，代码生成与链接约 2.52 s，Service 准备约 4.32 s，B 专门化约 0.084 s。数据来自单次集成验收，不能据此判断构建提速。

B 的依赖计划已经以 Rust 类型在编译器内流动，消除了临时 Wasm 自定义段的 JSON 往返及 Host 对 snapshot 的前缀解析。继续前移 B 的裁剪只会影响这段派生开销，不能消除约 2.52 s 的完整 A 生成。

A 必须具备现有初始化会执行的 provider、构造检查与间接调用闭包。创建闭包与调用闭包需要区分，但仅凭源码函数图不足以覆盖生成 codec、materializer 和函数指针；当前精确依赖取自链接 relocation。Property required／optional evidence 可以封闭调用方需求，仍不意味着可以跳过保守初始化中的 provider。回归明确验证：不被 B 使用的 provider 在 A 中失败时，runtime 与 full 均拒绝发布。

因此，本轮保留 A 的既有执行语义；A 的范围收缩和 provider 调度变更分别评估，不作为本轮体积及冷启动收益的前提。实测支持先交付连续构建能力，不能以 B 变小替代完整构建提速证据。
