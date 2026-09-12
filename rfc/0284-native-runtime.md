# RFC 0284：Native 分类对象表与 world runtime

- 状态：实施中；基础分类表和初始化发布已实现，剩余对象类别及 JIT helper 继续推进
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

### 基础表与发布进展

独立 `telora-native::runtime` 复用 RFC 0281 实验的存储设计，不导入旧 VM/Val/Heap。与 native ABI 共用完整 Value 描述，描述外携带 session 身份以拒绝跨 session 和过期初始化引用。分配发生在 work，读取根据 HeapRef 的 world 位选择 main/work。

已支持 String（inline/heaped）、Tuple/Record 共表、Array/slice、有序双列 Dict。`publish` 对整个根集合使用一次转发表复制，保留共享和来源；临时 main 全部构建成功后才替换状态，清空初始化 work 并更新 session 身份。发布失败不改变旧 world，重复发布拒绝。新 work 可引用已发布 main 对象，但不能写入 main 表。

验证：`cargo test -p telora-native --features jit` 通过 9 项测试，其中 3 项 runtime 测试覆盖分类表共享、持久更新、Dict/Array 嵌套别名发布、来源保留、初始化句柄失效和失败原子性。测试源码放在 `tests/fixtures/runtime.telora`。enum/闭包/dyn/native resource、真实环以及机器码 helper 尚待覆盖；不据此关闭 #181。

后续已支持 enum 的 nullary/full_value/ValueTable 间接 payload 及发布遍历。以已 seal 的 variant 表校验 tag、payload TypeId 和宽度，JIT 按已选择的 variant 构造，不按名字猜测类型。递归类型的有限嵌套及间接 payload 发布已有 .telora 资产验证，当前 native 总计 15 项测试；真实对象环、闭包和 dyn 等仍未据此宣称完成。

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

HashState 按权威 native type (16, 3) 存入独立 Vec<Sha256> 槽位，描述符保存 HeapRef。更新读取原状态并复制固定大小摘要上下文，直接借用 native String/Bytes；不修改旧状态，不复制输入对象树。发布按原 HeapId 转发，保留重复引用；main 状态可在新 work 中分叉更新。增量协议保留版本前缀、输入类型标记、变长输入的大端长度及 Int 大端编码。固定摘要向量、标准 SHA-256 向量和真实 CLI 初始化/entry 分叉更新已有验证。

单测构造/访问/更新、空容器、异宽与递归对象、浅层共享、Dict 排序与重复键、错误来源、跨 world 引用约束。每个新增类别用少量有意义的单测验收，不接入旧 VM。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
