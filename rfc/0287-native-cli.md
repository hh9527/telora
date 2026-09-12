# RFC 0287：隐藏 --native 接入与执行语义对齐

- 状态：草案；由伞 RFC 跟踪，尚未实现
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#184](https://github.com/hh9527/telora/issues/184)

## 动机与范围

在不切换默认运行时的情况下提供逐步可用的新路线。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 隐藏 --native 选择新后端，普通帮助、README 和 guide 不列出；默认行为保持旧路线。CLI 仅做必要分流。
- 首批支持一个最小 eval 纵向用例，随 RFC 0285 进展接入；随后覆盖 check/eval/eval-with 及批量 check --lib/--tests；本期到 eval-with 截止，native run/serve 明确报 unsupported。
- check --only-types 仍在静态闭合处结束，即使选择 --native 也不创建 native session；普通 check 到初始化完成，eval/eval-with 的既定输出语义保持，entry 调度沿用各命令语义。
- 对每条命令维护 supported/unsupported 清单；未支持组合明确报错，不静默忽略 --native、不回退旧 VM。
- query 等纯静态操作不因 native 选择而创建执行环境；具体开关放置位置和不适用命令行为在实现时明确。
- 用现有 .telora 测试资产比较语义与诊断；旧实现只可作为测试对照，不是新路线运行依赖。

## 实施计划

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

## 验收条件

验证默认路径不变、隐藏帮助、显式 unsupported、only-types 零执行、各命令停止阶段正确。先少量冒烟，再补完整 corner cases；完成总装后才测编译/初始化/执行耗时和峰值内存，不承诺性能收益。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
