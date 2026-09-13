# RFC 0291：独立 Wasm 发布与执行路线

状态：推进中。分支：`feat/wasm-backend`。跟踪：[#186](https://github.com/hh9527/telora/issues/186)。

## 目标与范围

新增独立的 `telora-wasm` 实现，与默认 bytecode、native 路线并存。
隐藏的命令局部参数 `--wasm` 选择新路线，与 `--native` 互斥；不加入
普通 help、README 或 docs。第一期完成 check、eval、eval-with；不扩展
run/serve，不切换默认后端。一个 RFC 和一个 issue 维护全部设计与进展。

首要交付是可落盘、免源码发布的 Wasm 代码产物，同一产物可以在 CLI 和
浏览器执行。初始化在加载后进行；初始化 snapshot 留作第二部分目标。
Wasm 引擎仍可能需要编译，不能将可移植产物等同于无编译启动。

## 编译边界

复用模块加载、resolve、类型求解和 seal，后端输入为 SealedExecutable。
check 使用既有 seal_modules 建立执行闭包。后端只消费已经确定的类型、
函数实例、引用和稳定 ID，不新增推导，不按内置名称猜测身份，不回退
到旧 VM 或 native 执行。静态失败不进入初始化。

Wasm codegen 根据闭合节点机械生成 Wasm 指令。生成与运行分离：产物
拥有运行需要的全部静态信息，执行端不持有 MIR 或源码解析器。
未实现的表达式明确拒绝，不借助其他后端伪装成已支持。
发现动态类型阶段的遗留时删除对应路径，补充语言回归验证；不建立兼容层。

## Runtime 与 host

值、分类对象表、闭包、初始化需求状态及 property 计算位于 Wasm 内部。
不能通过逐次 host 调用旧 Val/Heap 来实现语言运算。host 仅承担外部
输入输出和资源接口，CLI 与浏览器遵循同一协议。

第一版采用 wasm32、单个线性内存，以逻辑区域表达 main/work；不依赖
multi-memory、Wasm GC、线程或 SIMD。沿用稳定 TypeId 与分类 HeapId，
Tuple/Record 共表、Dict 有序 keys/values 双列等已有语义。native 指针
宽度或 Rust 对象布局不属于 Wasm ABI，物理偏移须由 Wasm 布局明确计算。

第一条纵向链路优先使用无 host import 的标量与控制流程序验证真正的
可移植执行，再逐步补齐聚合和 runtime。可复用与目标无关的静态计划，
不强迫 native runtime 变成 Wasm 的依赖。

## 产物与来源

落盘产物包含可执行 Wasm、类型及函数索引、必要静态数据、来源位置和
格式/runtime ABI 版本。运行入口使用明确的导出协议，浏览器不需要
Cranelift 或 Telora 编译器。源码文本可不携带；文件标识与位置保留，
源码上下文可作为可选调试资料。免源码发布不等于内容保密。

产物装载验证版本、索引和内存边界；不得序列化 host 地址。布局、初始化、
诊断及调用协议在本 RFC 内随首条实现收敛，不另拆子 RFC。

## 初始化与执行

装载代码和静态描述 → 注入数据模块 → 求值顶层值与 property → 发布
main world → 创建 entry work world → 执行 eval-with。
初始化维持需求驱动、缓存和循环诊断，完成后主动求完应初始化的图。
check 在初始化完成后结束；only-types 保持纯静态路径，不创建引擎。
失败不发布最终结果。第一期不保存部分初始化结果或进程内存快照。

## 资源与工程约束

配额不作为首期主要工作：引擎有简单 fuel/内存限制接口时桥接，否则
明确记录暂未覆盖的部分，不新建复杂精确计费体系。fuel 用于约束失控
执行，不承诺指令级公平计费；浏览器长任务可由 worker 生命周期控制。
仍须保持基本内存安全和边界检查。

Rust 按职责用普通 mod 拆分模块，不用 include! 拼接代码规避行数限制。
优先复用 .telora 对照用例，Rust 单测集中于 ABI、生成器与装载契约。

## 推进与验收

- [x] 独立分支与单一 RFC。
- [x] 单一跟踪 issue、远端分支。
- [x] 最小 SealedExecutable → Wasm → 独立引擎执行；落盘后独立重载。
- [ ] 同一产物在浏览器执行，验证无需源码和 Rust host 语言运算。
- [ ] 分类堆、闭包、泛型实例、来源诊断和 runtime 基本操作。
- [ ] 数据注入、property/顶层初始化，隐藏 check/eval/eval-with 接入。
- [ ] 语言与 CLI 对照、发布物重载、浏览器 demo 验收。
- [ ] 分开记录 frontend、Wasm codegen、引擎装载/编译、initialize、entry
  与内存观察；在端到端完成后做性能评估，不每步重复基准。

引擎选型以首条链路的可用性为依据，解释器与 JIT 是运行策略选择，
不改变发布 ABI。最终验收不得以子集支持代替完整 eval-with 语义。

## 首条链路记录

2026-09-13：新增 telora-wasm crate，wasm-encoder 生成真实 Wasm；
Wasmi 2 作为当前测试解释器，不绑定最终 CLI 引擎选型。
最小测试使用只声明 native Int 身份的 prelude，完成正常 resolve/type/seal；
只接纳整数常量导出，metadata/check 初始化与其他表达式明确拒绝。
这不是完整 prelude、CLI eval 或通用初始化的支持声明。

`cargo test -p telora-wasm`：两项通过，验证生成确定性、释放 MIR 后解释
执行、零 host imports 和不支持表达式拒绝。
`cargo run -p telora-wasm --example scalar -- /tmp/telora-wasm-scalar.wasm`
生成独立文件；另起 Node 进程通过 WebAssembly API 加载，返回 42。
浏览器页面在 examples/scalar.html，使用相同 WebAssembly API 和文件选择器；
尚未实测浏览器，Node 的验证不计作浏览器验收。

本阶段没有增加 CLI 参数、runtime 兼容层或新配额系统。
下一步建立携带 TypeId/来源的值 ABI、函数/控制流和 Wasm 内存布局，
再接初始化及 CLI；当前 scalar 导出协议不冻结为最终 ABI。
