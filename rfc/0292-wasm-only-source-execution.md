# RFC 0292：统一 Wasm 源码执行路线

- 状态：已实现并验收；位于 feat/wasm-backend，尚未合入 main
- 日期：2026-09-14
- 跟踪：[#187](https://github.com/hh9527/telora/issues/187)
- 前置：RFC 0291（Wasm check/eval/eval-with）、RFC 0290（服务语义参考）

## 动机与决策

Telora 不再长期维护 default bytecode、直接 Cranelift native 与 Wasm 三条
执行路线。本期将 Wasm 确立为唯一代码生成与语言运行时路线，补齐 run、serve、
test，然后删除旧后端及分支。使用一个 RFC 和一个 issue 跟踪全部实施与验收，
不拆子 RFC 或子 issue。

所有需要执行语言代码的用户命令默认从源码开始，复用模块图、符号求解、
类型求解和 SealedExecutable，生成内存中的 Wasm，交给 Wasmi 执行。
不引入独立 fast-interpreter，不在本期更换 Wasm 引擎。

本 RFC 修订 RFC 0291 的“独立隐藏路线、不切换默认后端、先提供发布产物”
边界。撤下隐藏 `wasm build/eval/eval-with/check` 产物命令与 `--wasm`、
`--native` 后端开关，暂不提供用户可见的 Wasm 发布或文件执行接口。
模块发布、产物格式兼容和缓存策略另行决策，不为试验性旧产物保留兼容路径。

## 已有观察与取舍

world-model 的 `@src/bin/make-query:main` 涉及 21 个源码模块、10,762 行
源码。2026-09-14 release 观察：预热两次、测量七次，以下为中位数；不是冷缓存数据。

| 边界 | 耗时 |
| --- | ---: |
| 源码 → SealedExecutable | 416.9 ms |
| Wasm 生成与链接 | 141.3 ms |
| 发布打包与写出（新默认路径不需要此步骤） | 13.9 ms |
| 完整发布构建进程 | 589.7 ms |
| 从发布文件执行的完整进程 | 64.5 ms |
| 其中引擎加载及数据准备 / initialize / entry 执行与输出 | 25.3 / 21.4 / 9.3 ms |

同一任务此前 default 从源码执行约 454.8 ms。发布构建加文件执行约 654 ms，
只能作为边界参考，不能当作尚未测量的新默认内存流水线耗时。接受源码单次
执行可能慢于旧 default 的取舍，换取单一编译后端与运行时；最终另测统一路径。

含可读函数映射的产物为 4,167,482 字节，其中指令 section 2,021,998 字节。
来源信息为 source 名称与 bols，不含逐 span 行列号；生成函数名称带来源、
职责及实例类型。产物大小用于解释构建/加载成本，不是本期发布接口承诺。
以上只支撑路线选择，不证明 run/serve 吞吐、尾延迟或长期内存已经合格。

## 命令边界

| 命令 | 执行边界 |
| --- | --- |
| check --only-types | 静态求解与类型闭合；不 codegen、不链接、不创建 VM |
| query | 消费 MIR 查询结果；保持静态边界，不因切换后端而创建 VM |
| check（含 --lib/--tests） | 所选模块闭合、代码生成、数据注入、初始化与现有检查；不执行 Test 回调 |
| eval / eval-with | 数据与初始化完成后，按既有导出/Eval 契约执行 |
| test | 按现有 Test/fixture/诊断预期协议执行、隔离与报告 |
| run / serve | 按现有静态 entry adapter 与服务协议完成 configure、资源注入、initialize、事件 reduce |

其他静态或工程管理命令不受影响。保留模块选择、入口、`--source`、环境、
参数、资源输入、退出码与 Host 外部协议；不借后端切换重设计语言或标准库。
CLI 从源码运行无需用户安装 Rust 编译器或 wasm-ld；RT 在构建 Telora 时
预编译、预链接为 Wasm 模板，构建工具链可使用 Rust 自带 linker。

2026-09-14 补充决策：“不落盘”同时覆盖用户执行阶段的代码组装过程。
基于已经预链接的 RT 模板，在内存中追加类型、函数、函数表项、全局和数据，
保留 RT 既有索引；通过 wasmparser/wasm-encoder 完成组装，不调用外部
wasm-ld，不写临时对象、RT archive 或 Wasm 文件，不保留 linker 回退。
RT 静态区、生成程序需求槽位/类型镜像与 heap 起点显式分配，必要重定位仅
作用于我们生成的代码。模板可能保留更多 RT 代码，本期不承诺逐程序裁剪 RT；
记录其尺寸与加载代价，不引入通用 Wasm 对象链接器或依赖兼容性框架。

## 编译与运行时边界

全图 resolve 与类型闭合均不访问 VM。后端仅消费 SealedExecutable 中的稳定
类型、实例、引用与执行闭包；不重新推导，不根据用户名称识别内置能力。
各命令的根集合由公共静态准备确定，保留 check/test 与 entry 求值范围的区别。

Rust RT 与生成胶水静态链接；语言堆、闭包、property、初始化需求表与操作
均在 Wasm 中执行。Host 负责资源、事件、IO、计时与诊断展示，不通过旧
Val/Heap/VM 执行语言运算。类型确定的胶水仍由 codegen 机械生成。

位置继续使用 `(source_id, start, end)` 三个 u32，UTF-8 字节偏移语义不变。
Host 使用源码名与 bols 求行号/行内字节偏移，UTF-16、终端显示宽度不是本期
产物契约。保留 rule 与 subject 来源、debug 输出及诊断捕获边界。

## run/serve 与 work 生命周期

复用既有 entry_plan、静态 adapter、Configure → Initialize → Reduce
状态机与 Host IO/EES 调度。保留 EOF、终端 effect、请求/回复及失败终止顺序。
资源配置或初始化失败不得启动服务；静态失败不得创建执行 session。

初始化只执行一次，类型骨架、初始化顶层值与 property 结果在 main 区固化。
后续事件在 work 区执行。服务状态并非无状态请求：跨事件 reducer state、
闭包环境、主区引用与必要的 Host 句柄必须作为显式根保活。

在安全事件边界采用 work copy-collect 或等价的精确根回收，依据既定类型
布局遍历，不猜测值类型，不借用 native/default collector。回收须维护共享、
环和稳定的 main 引用；所有可移动 work 引用同步更新，不能留悬空 Host 偏移。
正确性优先，可先每个安全边界回收，频率调优延后；不能以持续追加不释放
作为 run/serve 完成交付。Wasm 线性内存可以保持高水位，但后续请求应复用
空间，存活数据固定时不得随请求数线性增长。

语言可恢复错误沿用现有会话协议，资源 abort/trap 不伪装为可捕获语言错误。
fuel 是限制不确定执行/失控循环的粗粒度保护，不是跨后端精确计费承诺；
优先直接使用引擎配额，不为复刻旧指令收费而扩展设计。服务终止释放全部
执行资源；不能用“失败后重建 session”偷偷丢弃协议要求保留的状态。

## test

现有 std Test 描述、名称筛选、fixture 加载、回调、预期诊断与汇总退出码
保持不变。普通 check 只构造和检查描述，不运行测试回调；test 真正执行。
每个用例的失败、诊断、配额与可变 work 状态不污染后续用例，fixture 与
闭包的来源和寿命须明确。优先复用 .telora 测试资产，不建设第二套测试语义。

## 实施与删除

1. 补齐 Wasm 服务会话、work 回收、run/serve 和 test；旧后端暂作语义对照，
   新模块不得依赖将被删除的 VM、native runtime 或其转换桥。
2. 将代码组装改为预链接 RT 模板上的纯内存追加，明确所有索引与地址边界。
   切换所有命令为源码 → 内存 Wasm，统一公共准备与 Host adapter；撤下
   后端开关和产物 CLI。对照验证后摘除旧模块，不留运行时 fallback。
3. 删除 telora-native、旧 bytecode codegen/VM、专属 Heap/Val 转换、配置、
   构建依赖与测试；公共静态/Host 能力先独立出来。清理直接 Cranelift/JIT
   依赖，不能按文件名字批量误删仍被前端使用的定义。
4. 完成全流程回归、服务寿命与内存验证，记录 release 时间分布，刷新 docs、
   帮助和工程说明。历史 RFC 保留，不把新决策静默写成历史事实。

阶段性成果在同一个 issue 更新；切换最终提交必须完整删除替代路径。
中间可编译/可测试进展不等同于迁移验收。现有工作分支继续承载准备工作，
本 RFC 不自动授权合入 main。

## 验收条件

- 无后端参数的 check/eval/eval-with/test/run/serve 均走 Wasm；query 和
  check --only-types 仍不创建 VM，不调用 linker。
- --lib/--tests、源与数据导入、property 初始化、泛型实例、诊断及来源、
  Test/fixture/筛选/退出码均有语言或 CLI 回归；旧语言资产的结果对照完成。
- run、stdio serve、多事件状态、EES 往返、EOF、终端 effects、语言失败
  后继事件、初始化拒绝、资源终止均通过；不新增 HTTP/browser 交付要求。
- 固定存活状态的长事件流验证 work 回收与内存平台期；另验真实增长状态、
  共享/环/闭包根、main 引用与收集后再次调用，不能靠丢弃状态得到平稳 RSS。
- workspace 构建与相关完整测试通过；依赖和调用点审计确认无旧 codegen/VM、
  telora-native 执行路径、运行时重新求类型或兼容 fallback。
- 用户执行阶段不调用 linker，也不写临时代码产物；发布式
  CLI 与后端选择开关不再接受。内部格式保留多少只由源码执行和测试需要决定。
- release 记录 world-model 等实际入口的 frontend、codegen/link、engine
  load、data、initialize、entry，以及服务多事件延迟和峰值/稳态内存；只记录
  可比边界，不把“从已发布文件运行”冒充新的默认源码启动时间。

## 延后与非目标

本期不设计 Wasm 模块发布、持久化缓存、初始化 snapshot、Wasm JIT/AOT
引擎选择或独立 fast-interpreter；不进行为体积而做的泛型合并、指令优化。
浏览器已有 demo 尽量保持，但 run/serve/test 的浏览器支持不阻塞 CLI 验收。
已有来源索引与函数映射成果继续保留，后续优化必须以观察为依据。

## 阶段记录

2026-09-14：Wasm RT 已实现精确 work copy-collect。Host 仅提供根句柄，
RT 使用已闭合的物理类型描述复制对象；保活 main、共享/环、闭包环境、
解释器配置缓存。Regex 的不可变编译结构留在 main 时不被请求缓存反向引用，
work Regex 在回收时按既有模式重建；这是一项正确性优先的成本取舍。
收集前消费诊断和 debug，收集后只保留显式根，服务不会重建/重置状态。

无后端参数的 run/serve 已切换到 Wasm，保留资源、EES、EOF 与终端 effect
协议。512 次固定存活状态事件验证回收后堆大小恒定；复合值与解释器循环
测试通过。Wasm 库 98 项通过（另 1 项语言对照 opt-in 未运行），现有 CLI
run 筛选 15 项、serve 筛选 13 项、Wasm 筛选 15 项通过。筛选中包含相邻
功能测试，不将其解释为同等数量的独立服务场景。完整迁移验收仍未完成。

2026-09-14：RT 预链接模板与纯内存代码组装已实现，删除执行阶段的外部
linker 模块和 tempfile 依赖。Rust RT 在构建 Telora 时链接并嵌入；用户代码
追加到模板的类型、函数、全局、表和数据区，启动函数设置生成静态区之后的
分配起点。保留函数名映射，RT 索引不变，不提供外部 linker 回退。
Wasm 库 98 项通过（1 项 opt-in 忽略），完整 CLI 101 项通过。
world-model make-query 源码执行输出符合预期；strace 的 process/file 跟踪
仅出现 Telora 自身 execve，没有子进程或以写入方式打开文件。该观察使用
debug 构建验证执行路径，不作为 release 性能数据。

2026-09-14：默认 test 已切换为 Wasm。初始化后按闭合 SymbolId 读取 Test
导出，描述与闭包留在 Wasm 堆，Host 负责 fixture 文件协议、展开和 JSONL
报告。测试边界区分正常返回、语言失败与引擎 trap：仅语言失败允许继续，
预期错误不重复输出，但保留警告；trap/fuel 耗尽终止会话，不满足 should_fail。
不重建 session、不重置累计 fuel。待执行描述/工厂作为精确根在用例间回收，
包含嵌套 fixture 捕获的输入值。

生成调用传递 source/start/end，Test 保存真实调用位置，fixture 相对于
声明调用所在模块解析。所有数据模块先解析并汇总错误，全部有效后才注入
和执行初始化。Wasm ABI 更新为 11，不兼容旧实验产物。

验证：Wasm 库 99 项通过（1 项 opt-in 忽略）；完整 CLI 102 项通过，包含
语言验收脚本；追加用例间回收后的 test_command 专项 8 项通过。覆盖嵌套
JSON/YAML/TOML fixture、重导出模块相对路径、预期失败后继续、警告保留、
输入来源、多数据错误汇总与初始化环。其他命令与旧后端删除仍需继续。

2026-09-14：无参数 check/eval/eval-with 已切到 Wasm，删除这些 CLI 路径的
旧 bytecode 编译、链接和 VM 调用；query 与 only-types 保持静态边界。
eval 入口契约在 codegen 前由已闭合的标准导出身份验证；JSON 输出不再把
时间类 Value 隐式转换为字符串。初始化环通过稳定全局 ID 的失败单元列出
涉及的全局名称。非尾递归限制仍由 Wasmi 执行，语言用例采用引擎的
`call stack exhausted` 诊断，不模拟旧 VM 的配额文字。

同时清理 debug site 遗留的预计算行号及重复模块名，仅保存原始位置，
Rust/浏览器 Host 根据 source 名称与 bols 构造显示事件；ABI 更新为 12。
验证：完整 CLI 102 项、Wasm 库 99 项通过（1 项 opt-in 忽略）；之后的
debug 元数据变更通过库专项 1 项、CLI 专项 3 项及浏览器 Host JS 语法检查。
旧 native 显式开关与产物命令仅暂留供迁移中的对照，下一步完整删除，
不把这些中间态当作 RFC 验收完成。

2026-09-14：撤下 --native/--wasm 与 wasm build/check/eval/eval-with 命令，
删除 native CLI、产物 CLI、telora-native crate 及直接 Cranelift 依赖。
55 份原 native 语言资产迁入 tests/runtime，保留结果与协议断言，撤掉后端
对照和已取消产物接口的测试；新增所有执行命令拒绝旧接口的验收。
测试按普通 Rust 模块组织，未用 include! 拼接新增模块。

迁移覆盖补齐两处缺口：局部 Bool 常量别名仍需要闭包捕获，不能通过追踪
其初始化值而漏捕获；未初始化函数的调用与复制产生带源码位置的语言诊断，
不再落为 Wasm 空表项 trap。静态入口测试改为验证 SealedExecutable 与
RunContract，删除无调用者的旧数据 linker 桥接。

验证：完整 CLI 99 项（含语言验收）通过，Wasm 库 99 项通过、1 项 opt-in
忽略；之后测试分模块专项 15 项、静态入口专项 2 项通过，浏览器 Host JS
语法检查通过。Cargo 依赖树与 lock 中已无 telora-native/Cranelift。
删除内容可从 Git 历史恢复。核心旧 VM、bytecode、Heap/Val 及其剩余公共
协议依赖仍在仓库中，需要下一阶段完整摘除；未完成最终运行寿命与性能验收。

2026-09-14：删除 core 的旧 bytecode/codegen/LIR/VM、Heap/Val、执行链接与
执行图，以及专属测试和 CLI Host 桥接。保留纯静态 MIR、类型镜像、入口计划、
数据解析计划与 Host 协议。包摘要改用 sha2 crate，保持输入协议；Wasm RT 的
HashState 实现尚未调整，状态相等契约需另行审视。

静态查询与 TestPlan 测试直接构造 MIR；JSON/YAML/TOML 测试改为验证扁平解析
计划，保留内容、来源、错误范围及配额边界。MIR 示例只提供静态阶段 dump。
正式实现文档刷新为 SealedExecutable → 内存 Wasm → Wasmi 路线。

验证：workspace 全 target/feature 编译检查通过；完整 workspace 测试通过，
其中 CLI 99 项（含语言验收）、core 181 项、Wasm 99 项通过，1 项 opt-in 忽略。
旧执行模块的调用点搜索为空。删除代码保留于 Git 历史。最终长服务与 release
测量仍待完成，这一阶段不声称已完成 RFC 验收。

2026-09-14 后续验收观察：固定状态服务扩展至 4,096 次事件，每次验证累积状态与
fuel 递减，回收后 heap 保持恒定，预热后 Wasm memory 大小也保持恒定。新增真实
增长状态用例：128 次追加后逐次校验完整历史与移动后的额外 Host 根；显式清空
后 heap 回到初始规模，随后仍可继续调用。三项服务/回收测试通过。这不替代
CLI/EES 长期 Host 来源数据库与输出缓冲的寿命审计，后者仍待完成。

同一 world-model make-query，release 从源码执行，预热 2 次、测量 7 次：

| 边界 | 中位数 |
| --- | ---: |
| frontend（静态准备与闭合） | 382.6 ms |
| Wasm codegen / 内存组装 | 89.3 ms |
| engine load | 30.5 ms |
| 静态数据注入（此入口无数据模块） | 0.002 ms |
| initialize | 22.1 ms |
| entry 输入 | 0.10 ms |
| entry 执行及输出编码 | 10.5 ms |
| 完整进程（time 百分之一秒精度） | 0.54 s |
| 峰值 RSS | 67,816 KiB |

7 次完整进程范围 0.53–0.55 s，峰值 RSS 67,532–67,972 KiB。各阶段中位数
独立计算，不以其总和替代进程时间。输出均为预期 Asia 查询。release strace
仅发现 Telora 自身 execve，无子进程及写方式打开文件；没有执行期 linker 或
临时代码产物。此次仅观察当前源码路线，不作跨版本性能归因。

2026-09-14：Host 寿命审计发现每条 EES 回复的来源记录永久保留。现由精确回收
追踪存活值/内联 payload/Blame 的 source-id，保留冻结来源，清理无引用动态来源；
Host 同步清理 manifest 并复用仅属于运行输入的 SourceDatabase 槽。数据注册只取
计划实际引用的来源，避免复活历史输入。ABI 更新为 13，不兼容旧试验产物。

输入事件队列改为 64 项背压队列；终止通知解除阻塞发送，持续有事件时也及时
收割已完成异步任务。Output 仍按原契约缓冲至 terminal success；这部分属于
存活的待发布数据，不承诺在持续累计输出时进程 RSS 恒定。

专项验证：256 次丢弃输入后来源表/heap 恒定，闭包保留的 Blame 仍引用最初
数据位置；128 次真实 SQLite EES 测试保留第 0 条回复并返回第 127 条，稳定期
Host 来源槽、有效来源、Wasm heap/memory 均恒定。协议失败测试加入超过队列
容量的后继输入，验证终止不被发送等待阻塞且不发布部分输出。

release 384 次 EES 回复观察：正确输出第 0/383 条；reduce 中位数 0.0278 ms，
collect（含 Host 来源清理）中位数 0.1119 ms。稳定期 heap end 538,264 字节，
Wasm memory 589,824 字节，Host 来源槽 23 个、有效来源 22 个；20 ms 间隔采样
的稳定期 RSS 为 22,376 KiB。该短任务采样不是 OS 内存的精确峰值证明。
尝试 4,096 次同样 EES 往返时，现有累计 fuel 在第 478 次 reduce 附近耗尽，
正确非零退出且无 stdout；未提高或重置额度，也不把该次运行算作完成 4,096 次。
此前简单 reducer 的 4,096 次回收验证仍独立有效。

最终 world-model release 复测在 workspace 测试退出后进行，预热 2 次、测量 7 次，
stdout 与计时 stderr 分开采集：完整进程中位数 0.53 s（0.52–0.54 s），峰值
RSS 中位数 67,524 KiB（67,280–67,812 KiB）。阶段中位数 frontend 371.1 ms、
codegen/内存组装 84.5 ms、engine load 29.9 ms、initialize 21.9 ms、entry 输入
0.093 ms、entry/输出 10.5 ms；无静态数据模块的 data_input 约 0.002 ms。
并发测试期间的一组受负载干扰、合并输出流交错的采样未用于本结果。最新 release
strace 再次确认只有自身 execve、无子进程或写文件操作。

## 最终验收证据

2026-09-14：workspace 所有 target/feature 编译通过；完整 workspace 测试通过，
其中 CLI 100 项（含完整语言验收）、core 182 项、Wasm 101 项。之后仅删除旧
迁移对照 runner 并再次运行 Wasm 库，101 项通过、0 项忽略。JS Host 语法检查与
git diff 检查通过。各项要求按下表完成，未合入 main。

| 要求 | 实现与验证入口 |
| --- | --- |
| 单一源码 Wasm 路线；query/only-types 保持静态 | CLI `wasm_cli` 消费 SealedExecutable；`static_cli` 保留静态分支；`cli/backend_surface.rs` 验证所有执行命令拒绝旧接口，`cli/part-01.rs` 覆盖纯类型与普通检查差异 |
| RT 预链接、执行期不落盘/不调用 linker | `build.rs` 预链接并嵌入；`template.rs`/`compose.rs` 内存组装；最新 release 的 process/file strace 无子进程及写文件 |
| 删除旧 native/default 与桥接 | telora-native、旧 core VM/bytecode/LIR/Heap/Val 已删除；Cargo 清单/lock 无 Cranelift/native crate；执行调用点无旧模块，无 linker fallback |
| 数据、property、泛型、来源与 CLI 契约 | `cli/source_runtime/`、`cli/wasm.rs`、完整语言验收覆盖注入、初始化、eval-with、诊断及封闭实例；数据解析保留纯计划 |
| test 描述、fixture、失败恢复与退出码 | 正式 `TestSession` 与 CLI runner；嵌套/重导出 JSON/YAML/TOML fixture、预期失败、警告、初始化环和筛选验收；删除迁移期按用例重建 session 的旧对照 runner |
| run/serve IO/EES/EOF/终止顺序 | CLI `part-02.rs`、`part-03.rs`、`source_runtime/services.rs` 覆盖声明资源、并发 EES、重复请求、协议失败、EOF 和语言失败后续事件；满队列失败仍退出且不发布部分输出 |
| 固定/增长状态、共享/环、闭包与来源寿命 | Wasm `tests/services.rs`：4,096 次固定状态、128 次增长/清空、共享与解释器环、保留 Blame 来源；CLI 128 次真实 EES 验证 Host 来源池，release 384 次观察内存与延迟 |
| 构建、完整回归、文档与性能 | `cargo check --workspace --all-targets --all-features`、完整 `cargo test --workspace`；README、LANGUAGE、IMPLEMENTATION 刷新；上文记录源码完整路径和服务观察 |

实现保留既有累计 fuel 与终止语义，不以指令精确计费为验收目标。累计待发布输出、
真实增长的状态和未完成的外部请求仍是存活数据，内存可能随之增长；固定存活根的
回收平台期不意味着任意应用内存恒定。浏览器服务、发布格式、缓存与引擎替换仍为非目标。
