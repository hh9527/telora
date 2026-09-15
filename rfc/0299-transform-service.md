# RFC 0299：以静态 TransformService trait 统一查询入口

状态：已实现并完成回归，独立分支待审阅。
跟踪：[#201](https://github.com/hh9527/telora/issues/201)。
实现分支：`feat/transform-service`。

## 动机与范围

Telora 聚焦于数据求解：Host 提供知识库来源和查询输入，Telora 输出数据。
单次执行与持续服务应共享一个查询协议，不要求用户构造事件 reducer、Effect 或 EES 调用。
模块负责组织代码和导出入口类型，trait 负责检查执行契约。

本 RFC 用一个具体类型的 `TransformService` 实现替换 `entry.Eval`、`entry.Run(State)`、
`entry.Serve(State)` 三套入口。单次查询使用 `run`，多次查询使用 `serve`；
删除旧 `eval-with` 命令及被替代的入口协议，不保留兼容调度路径。
`eval` 继续读取普通 `Value` 导出；check、test、transform 的既有职责保持不变。

这是对 RFC 0257、0265 及既有 Entry/EES 执行约定的修订。
原有 entry 机制均可调整，以简化并收口 entry 为目标，不以保留旧内部 ABI 为约束。
不新增跨请求可变状态、TransformService 间消息发送、邮箱、用户态外部 I/O、并行调度、
Wasm 发布格式或磁盘快照。imos 继续服务包管理，其 Host 能力不因删除应用 EES 而删除。

## 现状调查与缺口

以下保留立项时的源码调查，描述被本 RFC 替换的实现；落地结果见文末。

| 部分 | 当前证据 | 本次处理 |
| --- | --- | --- |
| 静态 trait | `modules/std/fmt.telora` 的 Display 使用 `Fn(Self)`；MIR 的 MemberSelection 记录选中的 trait 实现，executable 收集对应根 | 复用静态证据和实例物化；先验证仅从返回值确定 Self 的 init 调用 |
| property | `_codec.telora` 等 decorator 接收 previous 并返回名义 property | 新增 Sources property 和 source 归并函数，不新增装饰器语法 |
| 单次入口 | CLI 的 `wasm_cli/eval_contract.rs` 校验 std/entry.Eval 的名义身份；Wasm `entry.rs` 读取 config/evaluate | 替换为类型入口与已封闭的方法调用计划 |
| 服务入口 | core `entry_plan.rs` 生成策略模块 adapter，CLI `wasm_cli/run.rs` 执行 configure/start/reduce 与 EES | 用 init/transform 两阶段会话替换 |
| 状态保存 | Wasm `service.rs` 保留有类型的 Wasm 句柄，`collection.rs` 调用 RT 复制回收 | 复用值句柄与回收基础，不把 Self 解码成 Host JSON 再编码回来 |
| 失败恢复 | 内置 std/_entry/serve 已用 rt.with_diagnostics 包裹 reducer；Wasm capture.rs 捕获语言失败并恢复诊断作用域，ServiceSession 直接调用失败才进入 Failed | 在新内置 entry 中复用诊断包装，不另造 Host 语言失败恢复状态机 |
| 固化 | RT tables/collect 已有 freeze 和 MAIN_END | 调查二次冻结与表、来源、资源的关系；不是已有任意快照恢复能力 |
| 真实使用 | lab-ontology 的 world/spider/dog-model `src/bin/make-query.telora` 使用 entry.Eval，source input 承载查询数据 | 迁移为 transform 参数；模型 payload 或固定来源进入 Self |

因此这不是引入通用动态 trait object，也不是只改标准库名字。
主要工作在静态入口根、Host 生命周期和旧协议删除。暂无证据表明必须改变通用 trait
语言设计；无接收者 init 的推断和 codegen 仍需最小用例确认，不能仅由 Display 用例推定。

## 用户可见协议

`std/transform-service` 提供：

```telora
type Context = struct {
    sources: Dict(Value),
};

trait TransformService {
    init: Fn(Context) -> Self,
    transform: Fn(Self, Value) -> Value,
};
```

入口示意（省略导入和领域类型定义）：

```telora
@transform_service.source("a")
@transform_service.source("b")
type MyService = struct {
    a: A,
    b: B,
};

impl transform_service.TransformService for MyService {
    init: fn(ctx) {
        let a: A = codec.decode(A.type, ctx.sources["a"]).unwrap!();
        let b: B = codec.decode(B.type, ctx.sources["b"]).unwrap!();
        { a, b }.ty!(Self)
    },
    transform: fn(self, input) {
        // 使用 self 的知识库，对 input 求解并返回 Value。
        input
    },
};

export { MyService as MainService };
```

MainService 是类型导出，不是类型元数据值或 TransformService 实例导出。必须是所有类型参数均已
确定的具体类型，具有唯一且完整的 std/transform-service.TransformService 实现。允许正常的导入、别名和重导出；
不按类型拼写或成员 shape 识别协议，不要求用户类型必须叫 MyService。

Self 为实例的明确类型。init 的实现选择来自入口类型，即使参数中没有 Self，
也不能在运行时猜测它。transform 不返回新 Self；实例在查询之间保持语义不变。
Value 是封闭的数据类型，不代表 Unknown 或任意动态函数/资源。

### 来源与查询输入

source 是普通 property decorator。多次应用产生一个 Sources property，其中保存
逻辑名称集合；链式归并时重复名称报告诊断，最终向 Host 提供按名称稳定排序的清单。
没有 Sources property 表示空清单。不存在通过读取 TransformService 实例才能知道来源的路径。

Host 提供的初始化来源必须与清单一致：缺失、多余、重复绑定均在 init 前报错。
来源可以复用已有 JSON/YAML/TOML 解析、限制和 Loc 机制；Context 只包含已解析 Value，
不携带物理路径、文件句柄或网络访问对象。source 名称不等于文件路径。

查询输入独立于初始化来源。serve 每条请求的数据成为 transform 的第二个参数，不触发重新
读取来源或调用 init。环境变量和字符串参数不再是额外 Context 能力；业务如需这些数据，
由 Host 显式构造来源或查询 Value。不新增 env/args decorator。

## 静态闭合与内部调用

在求解前加入普通静态 adapter，或在 MIR 中登记等价的类型化调用根。优先复用现有
adapter 机制，避免新增平行求解器。目标是让普通 trait 证据求解确定 init/transform 的实现，
并在 SealedExecutable 中保存具体 TypeId、函数实例及完整输入输出类型。

入口计划显式保留来源 property 的访问根、init 根、transform 根及其依赖，不能因为
MainService 是类型导出而被 tree-shaking 丢弃。无 Sources 时不生成必然失败的 property 读取。
封闭后 codegen 仅生成确定调用；Host 消费封闭计划，不再按字段名字探测用户入口形状。
这是编译器的宿主调用描述，不是新增面向用户的函数包或 module shape。

类型求解不执行 property 或任何 Telora 代码。只确定 property 的存在及类型，
来源名称的实际值在后续初始化阶段产生。来源元数据若求值失败或有依赖环，初始化失败；
不允许用 TransformService 初始化结果反过来补齐模块求解。

## 生命周期、内存与失败

执行顺序固定为：

1. 全图 resolve/type-resolve、seal、codegen。
2. 注入源码图依赖的数据模块，完成普通模块顶层值和 property 初始化。
3. 读取 MainService 来源清单，由 Host 准备 Context。
4. 调用 init，保存具体 Self，建立查询开始前的稳定基线。
5. 每条输入调用 transform，输出结果与诊断，回收请求临时数据。

实例和初始化依赖留在 Wasm 内，Host 只保留有类型句柄。仅输入、输出和诊断跨数据边界。
首版可以用明确持久根配合现有 collector 保留实例；不强制立刻将所有 init 分配物
物理晋升到 MainWorld。语义固化不等于无选择地冻结所有初始化垃圾。
后续若采用“回收后再次 freeze”，必须验证表、闭包、资源、来源和所有句柄的有效性。

init 失败不发布实例。普通 transform 失败只产生本请求的诊断，后续请求仍面对相同实例。
业务错误可由普通 Value 表达；transform 签名不为宿主失败另加 Result。

诊断边界在内置 entry 中实现，复用当前名为 `rt.with_diagnostics` 的能力
（讨论中的 with_diagnostic），不为本 RFC 另加同义 API。其包装调用概念上是：

```text
with_diagnostics(input => TransformService.transform(concrete_self, input))(input)
    → Ok((output, diagnostics))
    → Err(diagnostics)
```

包装器本身作为普通静态调用根在 MIR 中封闭。用户只实现 transform，Host 消费内置 entry
的类型化结果。普通语言 failure 被包装成 Err，不会使外层 Wasm 调用失败；同一个 Self
因此可以继续服务。Warning 与失败前诊断一并保留，已捕获诊断不得被 Host 重复输出，
不得沿用旧 serve 只抽取 message 的简化转换而丢失 labels、notes 和 severity。
init 也可以使用相同包装，但 Err 必须结束初始化，不能进入查询阶段。

现有 with_diagnostics 不捕获引擎级 fuel 耗尽等 trap，Wasm capture.rs 也不将它们
转换成 Err。这是需要补齐的执行器边界，不能据此把请求配额耗尽升级为整个 serve 退出。
普通语言失败继续由内置 entry 捕获；fuel 或请求内存耗尽由执行器结束当前请求，
返回结构化诊断，恢复到查询前的可用状态，然后接受下一条请求。
在服务间隙 reset 执行环境，使下一次调用从初始化完成后的干净、确定基线开始。
这不要求 trap 后原地修补状态，也不指定快照、复制或回收算法；不得重新读取外部
来源或改变 TransformService 的知识库。磁盘快照不在本次范围。
真正的引擎或宿主故障另行报告，不能把预期配额耗尽当作不可恢复故障。

沿用 runtime 配额与 CLI override；fuel/memoryLimit 均针对单次查询，不累计到整个
serve 生命周期。前一请求的消耗和失败不扣减后一请求的预算。配额用于保证可停机，
不要求精准计费；具体页级限制、常驻数据计量及 reset 实现不构成语言契约。
初始化本身不是一次 transform，其资源失败不发布 TransformService。
持续请求不得无限累计诊断、临时值或来源 ID；复用现有请求来源回收机制并验证 Loc 不串号。

## CLI 与迁移范围

草案采用以下入口选择方式：命令接收模块 ID，并固定选择导出 MainService，
不继续接受选择旧 Eval/Run/Serve 值的 export selector。

```text
telora run <module> --source <name>=<source> ...
telora serve <module> --source <name>=<source> ... --bind stdio://
```

run 从 stdin 读取一个 JSON 查询，stdout 输出一个 JSON Value；失败走现有结构化诊断
与非零退出码。serve 从 stdin 接收 JSONL 查询，沿用已有 ok/error/diagnostics 响应
封装，每条请求恰有一个响应，串行处理。二者均拒绝把 stdin 同时作为初始化 source。
本次仅要求 stdio transport；不新增 HTTP/WebSocket。这些 CLI 细节随 RFC 一并确认。

删除应用 EES 配置、--ees-var、用户态 Effect/事件 reducer、旧 std/entry wrappers、
std/_entry 策略与对应 Wasm service ABI。按引用关系清除仅服务旧路径的 std/_rt、
std/ees 和 Host 桥接代码；不要误删 imos 包管理或其他仍使用的通用能力。
Dyn 不再用于 TransformService 状态擦除，但不因此删除整个语言的 Dyn 功能。

迁移 tests/runtime/service-entry、CLI entry_services、Wasm service fixtures 和现有
eval-with 测试；同步 docs/design、guide/EXEC-MODE.md、guide/TELORA.md 及示例。
旧 RFC 保留历史，不回写成新协议。lab-ontology 三个 make-query 及调用脚本需要迁移，
以原查询输入得到等价输出为验收，而不是把每条查询误固化为初始化 source。

## 实施路径

用一个跟踪 issue 推进以下三段，不拆成多个子 RFC：

1. **打通单次调用**：先用独立 .telora 用例验证 init/transform、Self 返回类型、跨模块
   impl 与重复 source 归并；实现 TransformService 标准库、静态入口计划和 run。确认已解析
   MainService 的类型身份驱动实例化，无新增动态派发或二次类型求解。
2. **打通多次查询**：实现会话持久根和内置 entry 诊断包装，接入 stdio serve。
   先保证成功/语言失败/成功连续请求正确，再覆盖配额耗尽后继续服务与来源生命周期。
3. **总装清理**：删除旧入口与 EES 应用路径，迁移真实模型、脚本、测试和文档，
   完成全量回归与一次 release 性能观察。不在每个内部改动后反复跑性能。

请求配额隔离与恢复是 serve 落地的必要条件。
第一项若发现通用 trait 实现缺口，在本 RFC 范围内补齐现有静态语义；若必须新增
存在类型等语言机制则暂停重新讨论，不以兼容旧动态路径绕过。

## 验收条件

- MainService 缺失、导出为值、泛型未确定、impl 缺失/歧义、签名不符在静态阶段诊断；
  别名和跨模块重导出成功，sealed 调用目标与 ID 可重复构建。
- init 返回具体 Self 的静态调用通过；transform 使用真实知识库字段；来源 property
  可重复归并，空来源有效，重复声明/缺失/额外绑定在 init 前产生准确诊断。
- only-types 不求值、不读取 TransformService 外部来源；普通 check 完成模块初始化，但不调用
  TransformService.init/transform。运行入口才获取其来源并构造实例。
- run 与 serve 对同一初始化数据和输入返回相同 Value；serve 初始化只做一次，
  连续成功、语言失败、再成功不会污染状态或串接诊断/Loc。
- 分别验证成功/fuel 耗尽/成功、成功/内存耗尽/成功，配额失败只结束当前请求，
  后续请求获得完整预算且看到同一初始化状态。服务间隙 reset 后持续分配仍能被拦住。
  普通语言 failure 经内置 entry 包装后仍能处理下一条请求，诊断不重复、不丢字段。
  长期请求的内存和来源登记不随次数无界增长。
- TransformService Self/闭包不经过 Host JSON 往返；collector 后持久根仍有效。
- lab-ontology 的 world/spider/dog 查询真实用例跑通，原数据结果保持等价。
- 全量测试通过，旧入口/EES 应用调度无残余生产调用。语言行为优先外部 .telora
  测试，不用 include_str!；宿主恢复和资源不变量保留必要 Rust 测试。
- 同机 release 记录改造前后的编译、模块初始化、TransformService 初始化、首条/后续查询时间
  与峰值 RSS，不预设协议简化必然提高所有指标。

## 备选方案

导出 `initialize: Fn(Context) -> TransformService` 会引入 trait 作为返回值的额外语义；本次不采用。
保留 reducer 状态转换增加跨请求更新与副作用规约，不符合只读查询目标。
强制立即实现快照/新堆/并发 TransformService 会扩大本次范围，优先复用现有类型化 Wasm 值和回收。

## 落地记录（2026-09-15）

- 标准库提供 TransformService、Context、Sources/source；静态 adapter 只引用 MainService
  类型并调用 prepare，MIR 确定 init/transform 实例。impl 中的 Self 通过普通词法类型别名
  绑定实现目标，支持具体泛型目标；没有新增运行时类型推断。
- CLI run/serve 共用一个执行器；普通失败由内置 with_diagnostics 捕获。旧 eval-with、
  Eval/Run/Serve、应用 EES/reducer 与对应浏览器示例兼容入口删除；包管理 IMOS 保留。
  浏览器示例本轮只保留普通值和具体函数调用，未增加浏览器 TransformService 调度器。
- 初始化后 collector 保留 handler 及其闭包依赖，保存线性内存和可变 globals。
  每请求复用已编译 Module，创建新 Store/Instance 并恢复初始化基线；不重新求解、编译、
  读取来源或执行用户 init。首版复制完整已分配内存，不承诺最优 reset 成本。
- 请求 memory 上限暂按基线字节数加配置预算实现，usage 报告对应有效线性内存上限；
  fuel 每次重新补充。serve 的 --report-usage 逐次报告已进入请求处理的用量。
  初始化及 Host 内存不属于精准计费契约。
- 工作区测试全部通过（CLI 83 项、Wasm 75 项）；语言验收 431 项通过。
  后续 usage/timing 收尾经 5 项入口测试复验。覆盖重导出、确定性 codegen、具体 Self、
  来源检查、诊断标签、成功/失败/成功、fuel/内存 trap 后恢复和连续 256 次 reset。
- lab-ontology 三个 make-query 及脚本已迁移，同输入结果与旧入口逐字一致。
  四个 crate 的 check --lib 通过。world/spider/dog 查询测试分别 22/28/42 项通过，
  完整套件使用 --with-fuel 10000；默认预算运行会提前耗尽，未修改默认配置。

同机 release 单次观察（墙钟非统计基准；RSS 含 Host 与 Wasm）：

| 模型 | 旧 eval-with | 新 run | 旧峰值 RSS KiB | 新峰值 RSS KiB |
| --- | ---: | ---: | ---: | ---: |
| world | 0.58 s | 0.60 s | 73388 | 71364 |
| spider | 0.58 s | 0.62 s | 71852 | 69664 |
| dog | 0.60 s | 0.61 s | 70752 | 69436 |

补充计时后的 world run 为 0.62 s / 71968 KiB：前端 409.3 ms、codegen/link 93.1 ms、
引擎加载 79.6 ms、模块初始化 22.3 ms、服务来源/init/基线建立 3.1 ms、首次 reset 2.2 ms、
首次输入/转换/输出 8.5 ms。world serve 的成功/null 失败/成功/成功序列正确：后两条成功
请求 reset 为 1.01/0.92 ms，输入/转换/输出为 0.52/0.48 ms，进程峰值 RSS 71664 KiB。
这些观测不推出性能收益结论，首次与后续调用也不要求 fuel 消耗相同。
