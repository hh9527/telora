# RFC 0284：Native 分类对象表与 world runtime

- 状态：草案；由伞 RFC 跟踪，尚未实现
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#181](https://github.com/hh9527/telora/issues/181)

## 动机与范围

实现不依赖旧 VM/Val/Heap 的独立对象运行时。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 从实验布局实现提取或迁移独立生产模块，消费已闭合类型描述；旧运行时可参考但不能成为新运行时依赖。
- 分类表采用 Vec<Item>，Item 拥有缓冲区；Tuple/Record 共表，Dict 使用 ArrayTable 的有序 keys/values 两列和二分查找。
- 逐类实现 String、Array/slice、聚合、enum、闭包、dyn 和必要 native resource，全部引用可由 TypeId 精确遍历；不保留未知类型兜底。
- main-world 固化后只读；work 可引用 main，main 不得引用 work；初始化发布的复制需保存共享和来源。
- helper 使用 RFC 0283 ABI，不借用旧 Val 转换桥；明确 borrowed view 在分配和回收期间的有效期。

## 实施计划

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

单测构造/访问/更新、空容器、异宽与递归对象、浅层共享、Dict 排序与重复键、错误来源、跨 world 引用约束。每个新增类别用少量有意义的单测验收，不接入旧 VM。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
