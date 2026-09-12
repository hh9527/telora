# RFC 0283：Native 值、调用帧与运行时 ABI

- 状态：草案；由伞 RFC 跟踪，尚未实现
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#180](https://github.com/hh9527/telora/issues/180)

## 动机与范围

闭合 MIR 如何成为机器码与 Rust runtime 之间唯一的数据契约。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 物化值保持 loc:[u32;3] + TypeId:u32 的 16 字节头部，data 宽度按 RFC 0281；Unit 16 字节、Never 无运行时值，不能分配伪值；不假定所有类型最多 4 word。
- 定义参数、返回值和临时值的布局与对齐。首版每个临时值独占固定槽位，不做活跃区间复用；标量中间运算可采用 SSA，但物化与报错时不能丢失来源。
- 候选内部 ABI 为 function(context, args_ptr, result_ptr) -> status；明确调用约定、指针长度/生命周期、重入、递归、间接调用、错误状态及无结果路径。
- TypeId 直接使用 SealedMir 身份，HeapId 区分 main/work 并结合对象类别解释。具体 world 编码须在本子 RFC 实现前定案，不预设最高位方案。
- 明确跨 helper 的 panic 隔离、溢出检查和来源三元组映射；禁止 Rust panic 跨生成代码 ABI 展开。

## 实施计划

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

用简单 Rust ABI 单测验证混合宽度参数/返回、递归帧互不覆盖、错误不读取未初始化结果、来源完整保留。记录首个支持的 target 和 word/endian 约束；不宣称 ABI 跨 target 稳定。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
