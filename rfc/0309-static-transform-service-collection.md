# RFC 0309：静态 TransformService 集合与传输路由

- 状态：草案，尚未实施
- 日期：2026-09-23
- 修订：扩展 RFC 0299 的单个 `MainService` 入口，不引入应用 effect system
- 前置：RFC 0310（从 Dyn 字段受检查地构造 struct）
- 推进顺序：先完成 RFC 0310 的独立验收，再启动本 RFC 的实现

## 动机

一个制品目前只能公开一个实现 `TransformService` 的 `MainService` 类型；
`run --serve` 的 HTTP 入口固定为 `POST /transform`，Guest 只保存一个 handler。
应用若有多种独立查询，只能在一个 `transform(Self, Value)` 内手动解析请求、
分派并组织输出。希望在类型域声明一组静态服务，由编译器封闭每个实现，
让传输层按明确的标识路由，同时保留各服务自己的实例类型。

本 RFC 不引入动态 trait object、服务间消息、跨请求可变状态或 Host effect。
`TransformService` 的 `init` / `transform` 签名不变。

## 声明与身份

设想的公开入口如下（字段装饰器和 `PropertyTarget::Field` 已受支持，
具体 `http` / `method` property 尚需定义）：

```telora
pub type MainService = struct {
    @http.post("/lower-content")
    @method("lower-content")
    lower_intent: LowerIntent,

    @http.get("/svg/{entity}")
    @method("svg")
    svg: SvgRender,
};
```

`MainService` 是**真实的名义 struct 类型**，初始化后的值包含各字段的
`TransformService` 实例。每个字段声明一个服务槽位，其字段类型必须是完整
确定的具体类型，并具有唯一的 `TransformService` 实现。字段名是组合值的
普通字段名，也是编译期生成调用时的投影依据；`@method` 指定
对外稳定的、非空且唯一的方法名；`@http.*` 是可选的传输路由。没有 HTTP
路由的槽位仍可由 method 协议调用。所有字段在 MIR 封闭时作为入口根，
不根据请求流量增量实例化模板，也不在运行时做 trait 选择。

入口必须具备一个表示“可对外伺服的静态服务集合”的 trait 证据（暂称
`ServiceCollection`）。这个 trait 约束的是整个 `MainService` 类型，不取代
字段上的 `TransformService`；字段 property 只描述路由，不独立授予伺服资格。
集合 trait 消费字段 property 的构造与路由信息；它可以经
`type_desc::fields` 和 `get_field_prop` 遍历字段，对每个字段生成携带权威
类型身份的 `Dyn`，最后通过 RFC 0310 的 `FromDynFields` 受检查地组装
`MainService`。是否需要显式 `impl ServiceCollection for MainService` 作为
选择开关，须在原型中验证现有 trait 证据与字段 property 能否配合；
不能仅凭运行期字段 shape 猜测入口。此规则不要求普遍的 struct `derive`。

保留现有的 `pub use ... as MainService` / 单个具体 `MainService: TransformService`
作为单服务入口；它继续使用现有的 `POST /transform` 与原始 JSON 请求协议。
两种入口形态由静态 trait 证据区分，不用运行时 shape 猜测。集合形态的路由不隐式
套用单服务默认路由。其他普通 struct 的字段装饰器不因此获得服务语义。

编译时拒绝重复 method、同一 HTTP 方法与路径的冲突、无效的路径模板、
不支持的 HTTP 动词、缺少 trait 实现或无法封闭的字段类型。不同 HTTP 动词
可以使用相同路径；具体路由歧义判定须覆盖静态路径与参数路径的重叠，
而不能仅比较路径字符串。路由及 method 清单作为制品中确定性的元数据，
runner 无须读取源码。

## 初始化、请求与快照

所有槽位在同一个制品实例中初始化，顺序由静态字段顺序确定。Host 先取得
所有槽位声明的 source 名称的并集并注入一次；同名 source 指向同一份已解析
Value，各字段的构造函数获得相同的只读 `Context.sources`。字段 property
提供或指向与字段类型对应的构造能力；它必须保证返回的 `Dyn` 包含
相应 `TransformService.init(ctx)` 的结果，或显式使用等价的受约束策略。
按字段索引收集全部 `Dyn` 后，使用 RFC 0310 的 `FromDynFields` 检查并
组装一个 `MainService` 值。构造过程不调用用户另写的集合级 `init`；
构造出的每个字段就是后续 transform 的 receiver。任一 source 失败或任一
`init` 失败，整个服务集合初始化失败，不发布部分可用的路由。初始化 fuel
与内存预算属于整个制品的一次初始化，不因槽位数自动倍增。

全部初始化成功后，以组合值作为保活根建立共同基线；快照必须包含完整的
组合值、路由清单及已初始化状态。某槽位收到请求时，由静态计划从组合值
投影相应字段，调用该字段类型的 `TransformService.transform`，再沿用
`with_diagnostics` 生成现有 `telora.service/v1` 响应。实现可以为每个槽位
缓存闭合的 handler，但这些 handler 必须捕获或引用同一个组合值中的字段，
不能成为与 `MainService` 脱节的另一组独立 service 根。
不同 method 的请求同样相互隔离；每次请求重新获得完整的请求 fuel 预算，
不把其他槽位的 `init` 计入请求。陷阱后沿用实例恢复机制，不能复用已污染实例。
集合不能在请求之间动态增删槽位或重新初始化单个槽位。

## 调用与传输边界

HTTP 路由只负责选择槽位并将外部输入转换为一个 `Value`；语言侧的
`transform(Self, Value) -> Value` 不因 HTTP 动词改变。HTTP 层的 404/405、
输入过大等传输错误继续与语言诊断区分。`stdio+jsonl://` 无 URL，必须有
显式 method 标识；无 `--serve` 的单次 `run` 也遵循同一规则。

请求格式还不能直接沿用“每行就是 transform 的 JSON 输入”：需要定义
method 与业务输入的无歧义边界，同时不把业务对象中的 `method` 键当作
路由信息。HTTP 的路径参数、query、body（尤其 GET 无 body）如何规范化
为 `Value`，响应是否只用 JSON、以及是否允许无 HTTP 路由的 method，
须在实现前以可运行用例确定，并同步定义请求协议版本。不能让 runner
基于字段名或 URL 猜测请求结构。单服务既有协议不受这些选择影响。

## 实施范围与验收

1. 复用现有字段 property，验证 `@http.post` 等命名空间调用和静态值
   提取；定义 `ServiceCollection` 证据及受限派生的触发条件，完成路由
   冲突校验。字段 property 不等于组合值字段的运行期内容。
2. 在 RFC 0310 的构造接口上验证字段 property 提供的构造函数能否
   取得对应字段类型的 `TransformService` 证据、打包 `Dyn`，并组合为
   `MainService`；如需受限的 witness 适配，明确其准入与闭合规则。
   验证 `Context` 可被各 `init` 共享，初始化失败不发布部分构造值。
3. 扩展 Wasm/RT 合约为组合值根和静态路由/调用表；copy-collect、reset、
   快照导出和恢复必须保持字段与闭合调用的对应关系。Host 和
   `telora-run` 从同一制品清单获取可用路由。
4. 定义并测试 method 请求协议、HTTP 输入映射及诊断；同步更新正式文档。
5. 验证不同字段类型、共享 source、重复路由、部分初始化失败、交替请求
   reset、请求陷阱恢复和 snapshot 还原后路由一致性。以 `run` 和
   `telora-run` 对同一制品的行为一致作为验收条件。

现阶段只确立静态集合与生命周期方向；请求编码和 HTTP 参数映射未决，
在其规约确定前不改变现行单服务传输协议。
