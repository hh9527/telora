# RFC 0322：宿主提供带来源 offset 的结构化输入

- 状态：提案，已完成机制 POC，尚未合入
- 跟踪：[telora #257](https://github.com/hh9527/telora/issues/257)

## 动机与范围

DARI 等宿主已经完成自身源码的解析，并持有业务输入结构和 AST 的字节范围。Telora 目前由 JSON/TOML/YAML 解析器生成值及来源；宿主若只能传文本，就要先生成另一份数据文本，再处理这份文本与原始源码之间的映射。嵌套 JSON 字符串的普通 decode 还会让内部值继承整个字符串来源，失去字段级定位。

本 RFC 增加结构化输入入口：宿主直接提供值及来源 offset，替代业务字符串解析这一步。导入后复用 Value 物化、codec、普通值传播、blame/fail 和诊断机制。Telora 模型无需认识 DARI 的语法，也无需增加来源 ID 参数。

本轮交付 Telora 的结构化导入、可供集成使用的 Wasm/runner 输入通路及验收。Telora 完成后，DARI 再将 query_remote! 意图改为这种输入，并将诊断 offset 映射为 DARI 源码范围。数据库执行和 SQL lowering 语义不在本 RFC 中改变。

## 临时来源语义

使用现有临时输入来源 source=0，诊断显示名固定为 `<input>`。范围为宿主原始输入的 UTF-8 字节半开区间 `[start, end)`，并非生成后的业务 JSON 的偏移。

- 每个值和对象键分别可携带来源范围
- 多个生成值可共享一个来源；生成字段不必与源码逐字段对应
- 来源可缺省，缺省不虚构用户代码位置
- 不传文件名、文档版本、源码文本或行索引
- Telora 不计算原始文档的行列、不解释 CRLF；宿主完成 UTF-8 offset 到编辑器范围的转换
- 数据来源沿用现有临时输入约定；Telora 规则位置仍指向模型源码
- 来源不属于业务对象字段，不参与业务值相等性和业务意图摘要

source=0 的来源使用现有 packed byte range，`[0, 0)` 与无来源均编码为零，本轮保留这一已有约定。宿主可以为非空表达式提供 `[0, n)`；它不同于无来源。

一次请求的 `<input>` 只代表该请求，不得把旧请求的 offset 用于新文档。输入及诊断受现有请求回收边界管理；宿主在将 offset 转为文件位置前须持有对应请求的源码。请求值若被显式保留，其来源也只对原请求有意义，不会自动重绑定到后续请求。

## 结构化契约

逻辑形式如下；这是宿主交换结构，不是 Telora 用户语言中的新类型。

```text
Span         = { start: u32, end: u32 }
LocatedValue = { span: Option(Span), value: InputValue }
Field        = { name: String, span: Option(Span), value: LocatedValue }
InputValue   = Null | Bool(Bool) | Int(Int64) | Float(Float64)
             | String(String) | Array(Array(LocatedValue)) | Object(Array(Field))
```

Int 与 Float 明确区分，避免跨语言交换时丢失 Int64 精度或改变 codec 分支。对象字段名必须唯一，键来源与字段值来源互不覆盖。对象顺序不改变 Value 对象的业务语义。首版范围与现有 JSON 可表达的值集合一致；Bytes 和 temporal scalar 可在有独立需求时扩展，不让普通字符串承担其隐式转换。

例如宿主源码 `project({ a: x.field })` 可以生成 `{a: "cpu_usage"}`。键 `a` 的来源指向 `a`，字符串值的来源指向 `x.field`。若编码的成员引用变成一个多字段对象，各生成字段可共享 `x.field` 的范围。模型仍接收普通 Value，执行 `codec::decode` 后，`fail!("unsupported member", intent.a)` 使用已有 subject 来源。

导入器必须在发布值前验证：结构完整性、范围满足 start <= end、offset 不超过当前表示上限、有限浮点数、唯一对象键、节点数、深度和输入存储大小。没有源码文本时无法验证 UTF-8 字符边界或实际文档长度；这两项由宿主负责。无效输入是边界错误，不能作为正常业务值进入 handler。

## 接口与执行

Rust Host 提供 `Session::import_located_value(value)`，不接收 SourceFile，不注册源码。返回值与现有解析接口返回的 transport::Value 具有相同使用规则。

物化器继续使用既有 Value 布局和 GC 根协议。可以复用内部后序节点包，但内部 TypeId、Guest 指针、节点包布局不能成为宿主提供的业务值。导入器负责把公开结构转为物化器输入；宿主不需要访问最终封闭类型或构造 Guest 堆对象。

Wasm/runner 必须提供从结构化输入到同一导入路径的适配，使 DARI 集成不依赖 Rust 私有 API。传输解码与业务文本解析分开：传输可以使用版本化的结构包，传输层校验后直接导入 value/span；不得把其中的业务 value 重新 stringify/parse 来取得来源。现有文本请求入口保留自身用途，不隐式猜测带有 span 的普通业务对象。

具体传输编码随实现交付，以独立版本或显式输入入口识别，不依据业务字段名识别。边界契约不绑定某一 Wasm 内存布局，所有执行适配应观察相同的 Value 与来源语义。

## 诊断

codec 错误与领域错误继续使用现有诊断结构。报告中保留模型规则和有序 subjects；source=0 的 subject 以 `<input>` 的原始 byte range 输出，不借用模型规则行号表示输入 offset。DARI 在自身适配层选择输入 subject 为用户主位置，模型规则作为相关信息保留。

`fail!(message, subject)` 可以引用输入来源；只有消息、没有输入 subject 的失败不能凭空获得 DARI 位置。panic/运行器 trap 仍属于实现或执行错误。

现有 record codec 对未知字段使用字段值作为 subject，字段路径包含未知键名；结构化导入不暗中改变这一策略。键范围也须保留，之后可以独立改进 codec 的 subject 选择。独立错误的收集策略由模型负责，不将 fail! 的立即终止语义改为自动累积。

## 备选方案

- 生成业务 JSON 并建立 JSON 到 DARI 的 source map：增加一次编码、解析及映射，宿主已经拥有所需结构
- 为所有业务节点增加 origin_id：重复已有值溯源机制，让模型额外传播来源身份
- 注册具名 DARI SourceFile：机制 POC 已验证可行，但临时输入无需文件、文本、CRLF 或行列处理
- 让宿主直接写入最终 Value 内存：依赖封闭类型及布局，绕过输入验证、所有权和 GC 合同

## 实施计划

1. 单独提交本 RFC，保留机制 POC 作为证据
2. 将导入入口收敛为 source=0 + offset，覆盖值、键、数组和对象
3. 提供显式版本化的服务/runner 适配；检查 malformed 输入、配额、请求恢复和诊断 byte range 输出
4. 同步当前实现文档及 Host 使用指南，完成定向回归
5. 汇报 Telora 验收结果后再推进 DARI query_remote! 接入；未经授权不合入、不部署

## 验收

- 同一结构化输入经过实际 Wasm handler/codec/领域检查，返回与字符串解析路径相同的业务值
- 合法输入成功；codec 类型错误和领域拒绝均精确指向宿主提供的 `[start, end)`
- `[0, n)` 正确保留；缺省来源不伪造位置；重复来源被现有诊断去重
- 键与值范围独立保留；嵌套对象、数组、空集合、长字符串和 Int64 边界正确
- 中文及 CRLF 原始源码的 span 仅按 byte offset 往返，不要求 Telora 解析原文
- source=0 输入不增加具名 source registry；GC 后仍保留值和临时来源
- 重复请求回收、错误请求后恢复、配额耗尽和 malformed 数据符合现有执行边界
- 真实服务/runner 通路可用，既有文本输入及诊断回归通过
- 没有 DARI/IC 专有逻辑进入 Telora；DARI 接入独立实施
