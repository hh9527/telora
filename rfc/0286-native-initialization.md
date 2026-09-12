# RFC 0286：Native 整图初始化、property 与发布

- 状态：实施中；多入口代码计划与运行时需求状态表已建立，整图调度接入中
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#183](https://github.com/hh9527/telora/issues/183)

## 动机与范围

在 native 路线重建既定三阶段语义，不半发布用户结果。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 阶段一为无 VM 静态求解；阶段二为初始化；阶段三为 entry 执行，后两阶段完全消费已求解类型。
- 创建一个整图 Initialize WorkWorld，先解析/注入数据模块，再计算顶层导出与 property；所有模块共享初始化上下文。
- ExportId 和 (TypeId, PropertyTypeId) 为稳定需求键；native runtime 管理未开始/计算中/完成/失败状态，函数代码访问该状态，不与 codegen 共用可变求值表。
- 主动驱动整图顶层值/property 完成；内部按需求值处理相互依赖，遇到真实递归需求给出环诊断，Failed 不重复报告。
- 全部成功后将可达初始化数据一次复制到 main-world 并固化；保留别名、共享和来源，TypeId 不重分配。
- 初始化失败禁止执行 entry 和发布成功 session；依旧允许输出已收集诊断。

## 实施计划

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

### 当前实施证据

`compile_roots` 在一个代码内存 owner 中注册多个 HIR 入口，按 HIR ID 排序去重，共享函数及间接调用分派。已验证同一计划执行工厂函数生成闭包，将闭包及捕获发布至 main world，再由另一入口调用；初始化旧句柄被拒绝。

Runtime 需求键使用已 resolve 的导出 SymbolId，或 `(TypeId, PropertySite, PropertyTypeId)`；field/variant property 必须保留 site，不能与类型本身的 property 混淆。状态为 Pending / Evaluating / Ready / Failed。首次递归请求将状态置为 Failed 并返回环错误，后续请求只传播 Failed。发布前所有已注册需求必须 Ready，需求值与显式根共用一轮别名复制，发布后状态表持有新的 main-world 描述符。

上述状态逻辑已独立验证，但尚未将全图 exports/property 清单注册及生成代码中的需求访问连接起来；数据注入、provider 链执行和 CLI 初始化入口仍未完成，不据此宣称整图初始化已可用。

用 .telora 用例覆盖数据依赖、跨模块导出、顶层值/property 双向依赖、真正求值环、单次计算/失败传播、发布后值一致性。静态阶段证明不持有 native VM/context。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
