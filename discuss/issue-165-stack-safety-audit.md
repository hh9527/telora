# #165 栈安全审计记录

分支：`fix/165-parser-stack-safety`。本记录区分已验证路径与待完成路径，
不将有限的压力测试解释为任意输入的全流程栈安全证明。

## 解析边界

| 路径 | 保护方式 | 验证 |
| --- | --- | --- |
| Telora 括号、数组、块、插值 | lexer token 定界符预检，最多 32 层；错配闭括号不能抵消深度 | parser 与实际 CLI 的 1 MiB 栈测试 |
| 一元前缀、Fn 返回箭头 | grammar 循环收集，语义动作反向闭合 CST | 长链、末尾缺失、CST 重建、HIR lowering |
| 同级二元、后缀 | lelwel Pratt 循环；二元右侧递归受优先级层数限制 | 优先级与结合性语言用例；全流程后缀审计尚未完成 |
| else-if / if-let、块内顺序表达式 | grammar 循环，反向闭合原有节点 | 长链与畸形输入 |
| 条件、match 输入、return 值、fn 返回注解中的控制表达式 | RFC 0300 显式定界符边界，在内层解析前检查 | 独立语言诊断及小栈长前缀测试 |
| 模式和类型参数嵌套 | 递归跨 `()` / `[]` / `{}` 受限边界 | 核心及语言回归 |
| JSON 对象/数组 | token 定界符预检 | 32 层允许、超限及错配诊断 |
| TOML 数组/inline table | token 定界符预检；同级数组元素循环解析 | 10000 项数组、超限及畸形输入 |
| TOML 点分键 | lexer 回调循环扫描，table 路径迭代处理 | 10000 段键，普通及 inline table |
| YAML 生成 parser | `document: Line*`，不沿缩进递归 | 数据回归 |
| YAML 手写 block/flow/anchor 解析 | 进入递归前检查解析深度，block 与 flow 共享预算 | 深缩进、flow、anchor 前缀正常诊断 |

生成代码通过项目现有 parser 生成工具产生。没有给生成器或生成结果打补丁。
上述语法限制保护的是 Host 调用栈；Wasm fuel 和内存上限不能保护尚未进入引擎的解析过程。

## 解析之后

ValidatedDataPlan 使用 Id 边。后序排序、TOML table 封闭和既有数据配额遍历已改为显式
工作栈；深共享图不产生同深度的 Rust 调用栈。没有增加数据图深度配额，也没有在本轮删除
既有逻辑数据配额。

Wasm codegen 已消除一元、二元左链、Block 结果链、if/if-let else 链的递归。
后续审计发现 `.ty!(Int)` 链仍递归，现也改为显式 continuation；每节点的 adapt 和
value adjustment 仍在原有顺序执行。2000 次注解的完整 debug check 小栈测试通过。

管道长链会 lowering 为嵌套 Call，原先 callee/argument 的递归求值也会溢出。
现使用显式 CallCallee / CallArgument continuation，先求 callee，再依次求每个参数；
各参数求值后立即 adapt，最后发出调用。2000 段恒等闭包管道已通过 debug 下两种 check
的小栈测试，未增加管道长度限制。

## 当前证据与剩余事项

`a1621d40` 阶段：核心 174、数据 37、Wasm 75、独立语言验收 436 项通过；
完整 CLI 回归另验证 84 项通过（跳过已独立执行的语言测试）。
实际 CLI 在 Linux 1 MiB 主线程栈上验证：32 层函数成功，33/180/2000 层得到正常诊断。
包含注解链改写的 release 构建也已通过两项完整 CLI 小栈测试，覆盖源码与数据输入。

字段/调用、索引/调用和传播/调用的交替长链现也已进入显式调度。
递归名义类型保持固定身份，分别用 2000 次 `.next()`、`.next()[0]`、`.next()?`
验证完整 debug check 与类型检查；结构更新和具备目标类型的字段投影长链也通过。
此轮全部 75 项 Wasm 测试通过。

顺序 let-else 和普通解构 let 分别形成 LetElse / 隐式 Match 链，两者的成功分支现都
通过显式 continuation 求值；尾调用位置分析同时改为显式工作栈。2000 条两类语句
均已通过完整 debug 小栈检查。Match 改写后的全部 75 项 Wasm 测试通过。

Debug 的输入，以及诊断消息和 subjects 也改为显式调度；不为后置 macro 增加独立配额。
连续 2000 次后置 `.dbg!()` 与 section `\(_)` 也通过实际 CLI 小栈检查。
剩余表达式回调中的递归对应受定界符约束的子表达式：数组/元组元素、索引参数、
记录字段/展开、分支体/guard、闭包体和插值。类型引用消费封闭 MIR，解析侧的循环
reduce 不会重新构造递归 Owned AST。类型展开的既有保护与这次解析保护相互独立。

最终 Wasm 75 项、完整 CLI 84 项和 release 的两项实际 CLI 小栈测试通过。
重新执行 `cargo run -p telora-parser-gen` 未产生文件差异；源码大小与 diff 检查通过。
最终独立语言验收 436 项通过。Windows 尚未实测，需要原报告者回归；Linux 1 MiB 栈验证
证明本机的测试边界，不替代 Windows ABI、工具链和平台的实际回归。
