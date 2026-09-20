# RFC 0303：静态 FromStr 与 regex 类型绑定解析

- 状态：已实现
- 跟踪：[#214](https://github.com/hh9527/telora/issues/214)
- 分支：`explore/213-binding-lifetimes`
- 日期：2026-09-20
- 依赖：RFC 0258、RFC 0259、RFC 0260、RFC 0280、RFC 0291
- 修订：替代 `regex.prepare(regex, Type, TypeDesc)` 的全图动态类型分派；不修改 RFC 0301 的 `interpreter!` 求值语义

## 摘要

Telora 将字符串到类型值的转换表达为静态 `FromStr` trait。`string.parse` 通过
`T: FromStr` evidence 选择已经封闭的实现，Wasm 只生成实际需求中的
`Special::Parse(T)`。String、Int 与 Float 使用直接实现；regex annotation 发布
`RegexParse` 类型 property，由标准库的 property-constrained fallback impl 提供
`FromStr` evidence。

Regex capture contract 在确定 owner 的 property 初始化中生成并验证。实现不再把
`Type` 当作运行时开放分派键，不遍历全图 Record，也不保留旧动态路径。

`FromStr` 返回 `Self`，是静态可用但不满足 dyn-compatible 的 trait。本 RFC 不引入
trait object；未来动态 parser 必须使用返回 `Dyn` 的擦除后 companion capability。

## 动机

当前 `std/regex.parse_by` 源于动态类型阶段：provider 调用
`prepare(regex, ParseBy.type, target)`，其中 `target` 是普通 `TypeDesc` 值。Wasm
为 `regex.prepare` 遍历 `plan.layouts` 中全部 Record/Nominal，并展开一条
`owner == TypeId` 分支。无关类型也增加代码、临时 local 和加载成本。

真实 imaster-cloud `@src/bin/ask` 包含大量模型 Record，但仅少数类型声明
`@regex.parse_by`。当前制品仍生成一个 43,892 locals 的 `std/regex.prepare` 函数，
超过 wasmi 的 30,000 参数与 locals 合计上限。为每个分支复用 scratch local 可以解除
上限，却不能消除错误的全图依赖。

与此同时，MIR 已经具有完成此改造的静态事实：

- PropertyRecord 含确定的 owner TypeId、property TypeId 与 provider；
- RFC 0260 支持 trait、`Self`、`Property(P)` bound、blanket impl 和 exact-over-blanket；
- `string.parse` 已按实际目标规划 `Special::Parse(T)`；
- sealed layout 已含 Record 字段名、顺序、类型和 optional 结构。

因此解析行为不应在运行时重新猜测目标类型。

## 用户语义

### FromStr

`std/string` 导出：

```telora
type ParseError = struct { message: String, value: String };

trait FromStr {
    from_str: Fn(String) -> Result(Self, ParseError),
};

def parse:
    for(T: FromStr)
    Fn(String) -> Result(T, ParseError) =
    fn(input) {
        FromStr.from_str(input)
    };
```

调用写作 `string.parse@[T](input)`。显式类型实参直接表达静态实例选择；trait member 由
bound 中的 T 决定，不接受普通 `Type` 值，也不根据运行时位模式搜索实现。若未来需要动态
入口，应另行定义 `parse_type(Type, String)` 并返回擦除后的值；本 RFC 不提供该能力。

标准库为 String、Int、Float 提供 exact impl。实现可以返回普通 `Result`，其失败保留
输入值来源及实现规则来源。

### Regex-derived implementation

用户保留声明式写法：

```telora
@regex.parse_by(regex.compile(
    r"^(?P<year>[0-9]{4})-(?P<month>[0-9]{2})$"
))
type YearMonth = struct {
    year: Int,
    month: Int,
};
```

`parse_by` 发布类型 property：

```telora
@property(PropertyTarget.Type)
type RegexParse = struct { regex: Regex };
```

标准库提供唯一 property fallback impl：

```telora
impl(T: Property(RegexParse)) FromStr for T {
    from_str: fn(input) {
        # 概念表达；实际使用 sealed T parser intrinsic。
        regex.parse_record(T.type, input)
    },
};
```

Property 是 regex pattern 与配置的声明数据；FromStr 是行为。Trait membership 只依赖
property evidence 是否发布，不检查 payload 内容。payload 的求值或 contract 验证失败时，
property 不发布，因而没有 evidence 逃逸。

### 普通实现优先

用户可以为同一类型提供 exact 实现：

```telora
impl string.FromStr for Endpoint {
    from_str: fn(input) { ... },
};
```

它按 RFC 0260 的固定规则优先于 `Property(RegexParse)` fallback impl。普通结构 impl 也
优先，例如 `impl(T: FromStr) FromStr for Option(T)`；否则 Option 与任意 owner 的 property
fallback 在开放世界下必然发生模式重叠。两个适用的普通 impl、两个适用的 property
fallback 或重复 regex property 仍是冲突，不按声明顺序选择。

## Self 与 dyn-compatible

`FromStr.from_str` 返回裸 `Self`。对封闭实例 `FromStr@[T]`，返回布局就是 T 的 sealed
layout，因此静态调用没有不确定性。现有 `TransformService.init: Fn(Context) -> Self`
已经证明 trait contract 可以表达返回 Self。

这不表示 `dyn FromStr` 合法。未来 dyn-compatible 检查至少拒绝：

- 返回类型包含裸 `Self`；
- Self 出现在 receiver 以外且不能擦除的参数位置；
- 方法保留未封闭类型参数；
- 调用结果布局依赖隐藏 concrete type。

需要运行时异构解析时，应定义擦除后的能力：

```telora
trait DynParser {
    parse: Fn(Self, String) -> Result(Dyn, ParseError),
};
```

从 `T: FromStr` 到动态 parser 的包装属于后续 dyn Trait RFC，可以使用普通闭包或
`interpreter!` 完成 pack/unpack。本 RFC 不让静态解析经过 Dyn。

## MIR 与 evidence

类型求解为 `string.parse@[T](input)` 建立 FromStr obligation。成功封闭后，调用点
持有确定的 implementation instance：

```text
(TraitId::FromStr, TypeId::YearMonth)
    -> property fallback impl instance
    -> Property(RegexParse) evidence for YearMonth
```

SealedExecutable 必须同时保留：

- 被调用的 FromStr implementation instance；
- 支撑 blanket impl 的唯一 PropertyRecord；
- 递归字段解析需要的 FromStr obligations；
- 对应的 `Special::Parse(T)` 函数闭包。

后续阶段只接受这些结论，不重新通过名称、TypeId 全表扫描或 property payload 猜测实现。

## Property 初始化与提前诊断

RegexParse property 的 demand 仍在初始化阶段求值：

1. 求值 configured provider 与 `regex.compile(pattern)`；
2. 从 PropertyRecord.owner 取得确定的 sealed Record layout；
3. 生成 capture contract；
4. 调用固定 native regex RT 验证捕获名称、required/optional 和字段解析能力；
5. 成功后发布 property 值，失败则缓存 Failed 并报告 annotation/provider 来源。

验证只访问该 PropertyRecord 的 owner，不遍历其他类型。即使初始化后从未解析文本，显式
声明的无效 annotation 仍会在初始化阶段失败；这与现有 property 发布语义一致。

Capture contract 至少包含有序字段名、optional 标志和字段 parser capability。字段类型
本身的 FromStr obligation 在 MIR 阶段封闭；运行时 contract 不再用 TypeId 搜索能力。

## Wasm codegen

`Special::Parse(T)` 继续作为有限、可递归的类型专用 parser。规划器只从实际 parse/decode
根出发，沿 Option 与 Record 字段闭包加入 parser。

对 regex-derived Record，codegen 已知：

- 目标 T；
- 选定的 RegexParse PropertyRecord；
- T 的字段布局；
- 每个字段的 FromStr parser；
- property demand 的稳定函数与状态槽。

生成代码直接读取该 property demand 并调用 regex RT。禁止：

- 遍历 `plan.layouts`；
- 接收任意 Type 后构造 owner 分派；
- 遍历同一 owner 的全部 property 并按运行时 property TypeId 选择；
- 在解析调用中重复生成已在 property 初始化中验证的 contract。

Native regex RT 保留 pattern 编译、匹配、capture 和 contract 验证算法。它接收确定的 regex
handle 与 contract packet，不理解 Telora 全局类型图。

## Codec

当前 codec Properties 记录通过 `parse_by: Type` 与 `decode_by_parse: Type` 在运行时选择
路径。本 RFC 分两步迁移：

1. `string.parse` 与直接 regex parsing 先改用 FromStr evidence；
2. `Special::Decode(Source, T)` 对声明 DecodeByParse 的 T 建立静态 `T: FromStr`
   obligation，并直接调用选定 parser。

完成第二步后，codec 不再用 `parse_by` TypeId 选择行为。其他仍属于纯元数据的 codec
property 可以保留。

### Builtin 边界

Native 不是标准库控制流的默认实现位置。一个能力只有满足下列至少一项时才进入
Rust/Wasm native 层：

1. native 实现有明确的性能优势，例如 regex engine、数值文本转换和批量文本扫描；
2. 性能接近，但使用一个 dynamic/isomorphic 原语可以替代大量按类型复制的 Wasm 代码，
   从而显著减少 code size；
3. 能力具有普通 `.telora` 代码不可获得的特权，例如 Host ABI、来源注册、诊断出口或
   Guest 内存边界操作。

第二类 native 只允许复用已经封闭的同构运行时操作。它不得根据名称、开放 TypeId 或
property 搜索重新选择语义，也不得成为动态类型系统的兼容入口。若一个实现不满足上述
条件，其策略、Result 组合、错误传播和普通递归控制流应默认位于 builtin `.telora`。

`decode_with` 当前由 Rust codegen 完整实现，因而同时混合了两类职责：

- 根据 Sealed MIR 的布局枚举 record/enum 成员并生成具体递归调用；
- 组合 Result、路径、诊断、property 和用户可见的解码控制流。

迁移不能把前一类能力伪装成 `.telora` 运行时反射，也不能让 Rust 根据类型名称重新实现
trait selection。目标分层为：

1. MIR 为每个可达的 `Decode(Source, T)` 生成封闭 adapter 描述；描述包含成员布局、递归
   decoder，以及每个文本桥接位置已经选定的 `FromStr` 实现；
2. codegen 机械生成这些 adapter，不再判断某个类型“看起来是否可解析”；
3. builtin `.telora` 实现拥有 Result 组合、递归调度、错误传播和 property 组合等主体控制流；
4. Rust/Wasm RT 只保留 Value 表示、文本/regex/data parser 与布局访问等低层原语。

因此 `DecodeByParse` 本身不应成为 Rust native provider。它是 `.telora` property；Rust 只消费
MIR 已封闭的 owner/property identity。任意用户 exact `FromStr` 实现必须通过 adapter 中的
已选回调进入 regex 字段和 codec 文本路径，禁止 `regex_field_parsable` 一类按
Int/Float/String/property 分类的二次猜测。

## 与 lab-ontology 的关系

lab-ontology 推荐的生命周期是 property 声明、显式根集合、一次 build_root 准备、运行期
消费 PreparedPayload。Regex parsing 采用相同的声明与准备边界，但不照搬普通
`Fn(TypeDesc, ...)` 动态分派：行为由 FromStr evidence 静态选择，TypeDesc 只作为需要时
可读取的 metadata value。

EntitySource、RelationDef、ColumnMap 等仍是数据 property，不应仅因本 RFC 改为 trait。
未来可以独立评估其 provider 是否也应按 owner 泛型实例化。

## 迁移

本 RFC 不保留旧动态路径：

- `ParseBy` 更名或收缩为仅供 derive bridge 使用的 `RegexParse`；
- 删除 public/native `regex.prepare`；
- 删除 `regex_prepare.rs` 的全图分派；
- `string.parse` 增加 `FromStr` bound；
- 标准库、语言用例、lab-ontology 与 imaster-cloud 按新 contract 更新；
- 历史 RFC 仅注明被 RFC 0303 修订，不重写原设计正文。

是否保留用户表面的 `@regex.parse_by` 名称由实施中的迁移结果决定。名称可以保留，动态
语义和旧 ABI 不保留。

## 实施计划

1. 增加独立 `.telora` 探针：返回 Self 的 trait、generic evidence、跨模块 impl、
   exact-over-property-blanket 和递归返回。
2. 定义 FromStr、基础 exact impl 与 bounded `string.parse`，先沿用现有 parser intrinsic。
3. 定义 RegexParse property bridge，使 trait selection 完全静态。
4. 将 contract 验证移入确定 owner 的 property initializer。
5. 让 `Special::Parse(T)` 直接引用选定 property/impl，复用准备结果。
6. 迁移 DecodeByParse codec 路径。
7. 删除 regex.prepare、全图分派和旧 ABI；更新正式设计文档与指南。
8. 执行完整语言/Wasm/CLI 测试，并用 lab-ontology 与 imaster-cloud 验收。

阶段性提交推送本分支，并在 #214 汇报语义、测试与真实制品数据。#213 保留为发现该
动态展开问题的运行时故障记录。

## 验收

- FromStr 的直接、泛型、跨模块和递归实例均封闭到稳定 implementation ID；
- 返回 Self 不导致 Unknown，且不产生运行时类型猜测；
- String、Int、Float 和 regex-derived Record 解析结果及来源正确；
- 普通 exact/structural impl 优先于 property fallback impl；同级冲突产生静态诊断；
- 无效 regex、匿名 capture、字段缺失、多余 capture、required/optional 不符和字段缺少
  FromStr evidence 均有明确诊断；
- property 失败不发布 evidence，不产生半初始化 parser；
- 增加无关 Record 不改变 regex 相关函数数量、最大 locals 或分派代码；
- imaster-cloud ask 不再生成全图 std/regex.prepare，完整初始化成功；
- codec JSON/text 路径与迁移前合法结果一致；
- 比较 Wasm 大小、最大函数 locals、构建时间、初始化时间和首请求时间，仅据测量报告结果。

### 实施结果（2026-09-20）

- `string.parse@[T](text)`、基础/Option 实现、regex property fallback、用户 exact
  实现和 codec text bridge 均通过 sealed evidence；活动代码中已无旧公开动态入口。
- 泛型 regex-decorated owner、递归 Record、跨模块/泛型 trait evidence、构造校验与失败来源
  均有 `.telora` 或 Wasm 回归覆盖。
- 回归测试直接比较加入无关 Record 前后的 parser 数量及全部 Wasm 函数 locals 分布，结果
  完全相同。
- lab-ontology 的 ontology、world-model、spider-model、dog-model 均通过 `check --lib`
  类型闭合与完整初始化。
- imaster-cloud `@src/bin/ask` 在迁移 19 处旧调用后达到 0 conflict、0 unknown、0
  unproven bound，并完成初始化。以 release 编译器和 1000M fuel 观测：静态阶段约
  7.12 秒，执行初始化约 2.24 秒，消耗 791,830,712 fuel，Guest 线性内存
  54,132,736 bytes。
- ask 制品为 32,334,462 bytes；`wasm-tools validate` 通过。制品含 14,444 个定义函数，
  最大函数为 2,044 locals；`std/regex.prepare` 函数数为 0，实际封闭的
  `std/_parse.parse_with` 函数数为 22。
- section 尺寸中 code 为 22,348,747 bytes、manifest 为 4,594,426 bytes、name 为
  2,277,066 bytes、data 为 3,070,371 bytes。数据仅记录当前结果，不改变本 RFC 的
  静态选择结论。
- 独立 `telora-run` 从该制品执行真实 browse 请求时，read 为 19.94 ms、Wasm module
  load 为 180.54 ms、instance 为 5.68 ms、initialize 为 5,577.59 ms、reset 为
  0.002 ms、首请求为 4.14 ms；请求消耗 1,185,043 fuel。端到端进程时间约 5.84 秒。
  大型 service 的发布初始化需要显式提高构建预算；该预算现状与本 RFC 的 parser
  静态选择相互独立。

## 延后事项

- dyn Trait 布局、vtable 与对象安全的一般规则；
- dyn FromStr（明确不支持）；
- 返回 Dyn 的通用动态 parser registry；
- annotation 自动生成新 HIR impl 节点；
- associated type、默认 trait member 与 specialization；
- 与 regex 无关的 TypeDesc provider 泛型化。

## 放弃的方案

- **仅复用 regex.prepare 分支 locals。** 能解除 wasmi 上限，但保留全图代码和错误依赖。
- **提高 wasmi locals 上限。** 不解决生成规模随无关类型增长的问题。
- **把 Parse 全部实现成 interpreter!。** 静态 T 已知，不应先擦除再恢复。
- **直接提供 dyn FromStr。** 返回 Self 没有固定对象 ABI；需要单独的擦除后能力。
- **保留旧动态路径作为 fallback。** 会掩盖未迁移调用，使 codegen 继续承担全图分派。
