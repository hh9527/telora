# RFC 0290：Native run/serve 总装

- 状态：已实施并验收；隐藏 native run/serve 落地，默认后端保持
- 日期：2026-09-13
- 分支：feat/native-cranelift
- 跟踪：[#185](https://github.com/hh9527/telora/issues/185)
- 前置：RFC 0282–0287 已验收；回收契约见 RFC 0288

## 目标

在既有 native check/eval/eval-with 上增加隐藏 run/serve --native，沿用现有
std/_entry/run、std/_entry/serve 与 actor/EES 语义。默认后端保持，不新增 VM 回退。

## 实施路线

1. 将 run mode、静态 adapter 源码和封闭协议签名独立于旧 bytecode codegen。
   两条路线复用静态输入；native 只消费 SealedExecutable 的具体函数实例和 TypeId。
2. 建立 native Configure → Initialize → Reduce 状态机。配置、资源注入、事件与
   effects 直接读取/构造 native 值；内部状态和闭包不转为 host Value 树。
   EES 本来就是外部 JSON 协议，仅在这个边界序列化；不调用旧 resources_provider。
3. 隐藏 CLI 接入 run/serve，复用 ProcessRunHost 的外部 IO/EES 调度。配置拒绝、
   来源、诊断捕获、失败终止、EOF 与终端 effect 顺序保持现有语义。
4. 按 RFC 0288 在安全事件边界回收 work，保留真实服务状态及 main 引用。
5. 用现有 .telora/CLI 资产对照 run、stdio serve、多事件、可恢复语言失败和
   EES 请求/回复；补验资源边界及连续请求内存，再记录 release 分段数据。

## 验收

最终实现 `1e34c81`，基础状态机/回收 `56f0135`，均已推送远端独立分支。
逐项证据、完整命令和资源边界见[验收记录](0290-native-services-acceptance.md)。
workspace all-features 通过，包含 86 项 CLI、146 项 native 单测和 3 项独立实验；
无 JIT 的 runtime 为 29 项单测和 3 项实验通过。真实 ontology run/20 请求 serve
与默认 stdout 逐字节一致。下面首个切片的未接入说明保留为历史进展。

### 首个实施切片

`entry_plan` 现独立提供 RunMode、适配源码和 RunContract；旧 codegen 仅复用它。
native `ServiceSession` 已实现 configure/initialize/reduce，参数/结果消费封闭契约，
错误后状态进入 Failed，重复调用不再执行或重置 quota。事件边界 collect 显式保活
state/reducer，额外 host 描述符必须列为根。

146 项 native 单测及 3 项独立实验通过；CLI 静态 adapter 两项测试、旧 codegen
协议拒绝测试通过。语言资产 service-state.telora 验证多事件累积状态、回收后的
闭包调用和共享 fuel。CLI 开关与 IO/EES 协议桥尚未接入，不据此宣称 run/serve 可用。

- run/eval-with/eval/check 默认测试继续通过，run/serve native 输出与既有协议一致。
- serve JSONL 连续输入、EOF、跨事件 state、语言失败后的后继请求、EES 回包可用。
- 静态错误不创建 native session；初始化失败不启动服务；资源 abort 不被语言捕获。
- 回收覆盖空根、共享、环、main 引用、缓存、句柄失效、失败原子性和内存回落。
- host 完成与错误路径都释放 native session、JIT 代码及外部资源。
- 隐藏参数不进入普通帮助/guide；实现与验收持续提交推送到远端独立分支。

本阶段不切换默认后端，不合入 main，不建设 HTTP transport、AOT 或执行期间 GC。
