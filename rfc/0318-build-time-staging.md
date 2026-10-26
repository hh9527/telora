# RFC 0318：构建期计算的双路产出

- 状态：工作分支实施，不合入
- 跟踪：Telora #248

## 目标与语义

源码在受限构建期作用域执行 A，一次得到 P1 与 P2。P1 是编译器内部状态，支撑运行程序 B 的特化、可达性裁剪与状态封存，可含类型身份和闭包。P2 是符合编码契约的外部信息，例如 Wasm custom section。P2 不因被发布就成为 B 的 GC 根。这个能力与 Service、配置重导入和 snapshot 命令的既有输入约定独立。

标准库提供 std::build::Output(P1) {runtime: P1, information: Dict(Value)}，以及 stage(prepare: Fn() -> Output(P1), program: Fn(P1) -> B) -> B。prepare 与装配 program 都在构建期作用域求值；返回的 B 是运行期根。stage 在普通运行环境中失败；本轮不添加任意阶段结果的导入入口。

A 的可见外部输入只允许显式纳入构建的源码／数据模块内容，不接受 Service 运行期数据源、请求或宿主环境。Guest 没有时间、随机数、网络、环境变量等 Host 导入；新构建入口核对宿主导入及数据源约定。禁止仅凭一次普通 Service 初始化成功就宣称具备该保证。

## 首版接口与内部实现

独立的 staged build API 首先支持导出的普通函数，用一个不依赖 Service 的程序验收 P1 中的闭包、类型和状态。Service 构建通过 --staged 作为适配入口，--snapshot 保留原约定；两种选项互斥。--staged 拒绝 --source 及任何运行期源声明，stage 只在显式构建路径执行。

封存与裁剪复用同一编译对象及其受控执行结果。保留 TypeId／函数表槽位，P1 不经 JSON 回灌；snapshot 是内部状态运输机制。通用根管理应能封存普通导出函数，不依赖 Service 生命周期。初版仍可保留 Ready getter 及不可达 trap stub，不承诺达到最小二进制。

P2 保存于唯一 telora.build.information 自定义段，UTF-8 JSON 信封 {version:1,entries:{name:value}}。领域键由调用方定义，名称非空且唯一。旧 telora.service.metadata 属于已有实验协议；兼容消费者可读取，但新构建不能同时发布相互冲突的两个信息段。所有发布在 A、状态封存及 B 验证成功后原子完成，失败保留旧输出。

## 验收与边界

- 普通函数示例在构建时执行 A，P1 捕获闭包／类型，恢复后执行 B；无 Service 声明或 Service create 调用
- P2 可直接从字节读取；未被 B 引用的信息对象与生成代码不是运行期根
- stage 在普通运行期拒绝，运行期源声明及 --source 在 staged build 拒绝；构建失败不覆盖旧制品
- Service 适配、旧 snapshot／源重导入约定与通用编译器回归通过
- relation model 验证同源 transform 与信息输出，插件验收新信息段并保留旧协议兼容

后续扩展多导出根、更多后端与静态效果分析时保留同一阶段边界；本轮不把 Service 类型提升为通用构建概念。
