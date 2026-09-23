# RFC 0310：从 Dyn 受检查地构造 struct 与 newtype

- 状态：已实现并合入 main
- 日期：2026-09-23
- 关联：RFC 0309（静态 TransformService 集合）
- 跟踪：[#223](https://github.com/hh9527/telora/issues/223)
- 工作分支：`feat/rfc-0310-from-dyn-fields`

验收：已验证具体具名 struct、newtype、空 struct、泛型已封闭实例、数量与
类型错误、同类型字段交换，以及两种名义类型的 `@check`。具名字段实际规范
索引按名称排序，与 `type_desc::fields` 一致。核心、Wasm、CLI 全套测试通过；
`telora run`、独立 runner 和发布快照后的 runner 均通过构造与连续请求回归。

## 动机

`type_desc::fields(T.type)` 已提供字段索引、名称和类型，`Dyn` 携带权威的
类型描述，`dyn::project_with` 能用静态 witness 检查投影。但普通 Telora
代码不能把一组类型各异的 `Dyn` 字段组成精确的名义 struct 类型 `T`。
多服务组合需要这一能力；它并非 service 特有，也不应要求把字段名序列化
为 `String` 或为每种 struct 生成专门的路由构造逻辑。

本 RFC 首期提供对具名字段 struct 和单载荷 newtype 的通用、受检查构造能力。
enum 留待后续单独设计，不在首期范围。本 RFC 不引入任意 `Dyn -> T`
转换、动态定义类型、用户可覆盖的内置布局规则或通用宏系统。

## 对外接口

标准库暴露编译器拥有的 trait、泛型入口和结构化错误：

```telora
trait FromDynFields {
    from_dyn_fields: Fn(Array(Dyn)) -> Result(Self, StructFieldError),
};

type StructFieldError = enum {
    Count((Int, Int)),       // expected, actual
    Field((Int, Type, Type)), // index, expected, actual
};

let result: Result(A, StructFieldError) = dyn::construct@[A]([b_dyn, c_dyn]);
```

`A` 为 `type A = struct { b: B, c: C };` 时，数组索引 0、1 分别对应
`type_desc::fields(A.type)` 中的 `b`、`c`。具名字段的规范索引按名称排序，
**不承诺源码声明顺序**；例如 `{number: Int, label: String}` 的索引 0
对应 `label`。泛型 struct 在所有参数确定后按具体实例处理。
所有可构造的具体名义 struct 和单载荷 newtype 自动具备该 trait 的内置
证据；用户不能重写这些实现。结构性 Record、enum 和未闭合模板不自动
实现。字段类型不要求自身实现 `FromDynFields`：只需匹配 `Dyn` 包含的
精确类型身份。newtype 的输入固定为长度 1 的数组，索引 0 表示唯一的
载荷；不要求给载荷人为创建字段名，也不把 newtype 当成一字段 struct。

`construct@[A]` 在标准库中使用普通的 trait 证据选择，并调用内置的
受信任构造原语。不引入额外的 `A::` 语法。
`Array(Dyn)` 是结构构造参数，不是长期稳定的外部序列化格式。调用方可以
从 `type_desc::fields(A.type)` 按索引生成 struct 的数组，不需要字符串化
字段名。newtype 先通过 `type_desc::resolve` 取得名义 `Ref` 的 body，
再由 `type_desc::children` 读取唯一载荷类型；`fields` 不会返回该载荷。

## 成功、失败与位置

先验证目标类型是具备完整布局的具体 struct 或单载荷 newtype，再验证
数组长度恰好等于 struct 字段数或 newtype 的单个载荷数。对每个索引
验证 `Dyn` 内权威类型与封闭的字段/载荷类型一致；不得借助
字符串名称、结构形状相似、隐式解码或不受检查的 `Dyn` 解包来放宽匹配。
全部输入通过后才按目标的已封闭布局组装并发布 `Self`，保持字段或载荷
中的引用与普通构造相同；不得在失败后留下可观察的部分构造值。

错误是可处理的 `Result`，至少区分输入数量与输入类型失配，携带索引
以及预期和实际类型身份；struct 的字段名可由目标类型与索引查得，
newtype 则报告载荷，供诊断显示。
同类型字段交换无法被类型检查发现，这是按位置构造的明确边界。
返回值位置采用普通构造点的来源；字段值原有位置继续随值传播，失败
应能定位到本次构造调用，不声称已获得每个输入的独立源码位置。

普通名义 struct 与 newtype 的 `@check` 构造检查不能被此入口绕过：在输入类型
匹配并组装候选值后、发布 `Ok(Self)` 前执行与普通构造相同的检查。
检查失败按既有检查语义报告，不伪装为字段类型错误。若现有动态解码
的检查/拒绝路径无法复用，必须补齐后才能宣称目标类型自动实现。

## 实现边界

`FromDynFields` 的方法身份和泛型 `Self` 在 MIR 中封闭。codegen 根据已知
`Self` 的字段布局产生检查与复制逻辑，或调用固定 ABI 的受信任原语并传入
该布局；Guest 不依据 Host 提供的字段名、地址或类型猜测布局。
`Dyn` 打包和投影仍沿用原有权威描述，不另造第二套类型 ID。

实现使用受信任 native/codegen 构造器和仅限名义 struct/newtype 的编译器
拥有的通用 trait 实现，不要求用户逐个书写 `impl FromDynFields for A`。
显式用户 impl 被静态拒绝，避免绕过安全保证。该操作可以消耗请求期资源，但不新增来源
注册、IO 或跨请求可变状态。

## 验收与阶段

1. 验证 `Dyn` 对名义类型、泛型实例和带引用字段的权威类型比较；确认
   struct 的 `type_desc::fields` 索引与封闭布局索引相同、newtype 的
   唯一载荷可由 `children` 获取，并覆盖空 struct。
2. 为具体具名字段 struct 与单载荷 newtype 建立内置 trait 证据和最小
   构造原型；测试成功、长度不足/过多、逐索引类型失配及同类型字段
   互换的边界。
3. 将现有 `@check` 构造检查接入该入口，覆盖失败、诊断及不可观察的
   部分构造；覆盖 newtype 检查，验证嵌套、泛型、捕获闭包和
   copy-collect 后的字段保活。
4. 完成独立语言和 Wasm 回归后关闭 RFC 0310 的验收；再启动 RFC 0309，
   在两槽位 service 中使用该接口构造真正的组合值，并比较 `run`、
   独立 runner、快照恢复后的行为。多服务验收不属于本 RFC 的完成条件。

本 RFC 只解决**受检查的结构装配**。从字段 property 取得构造函数、调用
各个 `TransformService.init`、以及 HTTP/method 路由由 RFC 0309 定义。
