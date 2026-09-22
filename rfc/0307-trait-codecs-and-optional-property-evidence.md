# RFC 0307：Trait Codec 与可选 Property Evidence

- 状态：草案
- 跟踪：[#221](https://github.com/hh9527/telora/issues/221)
- 分支：`feat/rfc-0306-static-modules-paths`
- 日期：2026-09-22
- 依赖：RFC 0258、RFC 0259、RFC 0260、RFC 0280、RFC 0303、RFC 0305
- 修订：以 `Encode` / `Decode` trait 取代公开 codec API 中的 `Type` 值分派；不在本 RFC 静态化 Property 的求值

## 摘要

Telora 将 `T <-> std::value::Value` 的转换能力表达为静态 `Encode` 与 `Decode`
trait。普通代码只依赖 trait evidence，不再向 `codec.encode`、`codec.decode` 传递
`TypeOf(T)`，也不直接查询 codec Property。

Codec 的声明配置暂时继续使用现有 Property 初始化机制。常规结构配置逐步合并为共享的
`CodecProp`；派生 trait impl 消费确定的 `CodecProp` 及少量扩展 Property。Property 的值仍在
初始化阶段按需计算，本 RFC 不把它改造成纯静态元数据。

本 RFC 同时引入通用 bound `?Property(P)`。它表示 impl 始终适用，但具体函数实例可以获得
P 的封闭可选 evidence。该 bound 不参与 impl 选择、优先级或消歧；MIR 封闭后必须成为确定的
`Present(PropertyId)`、`Absent` 或静态 `Conflicted`，运行时不得按 TypeId 探测。

## 动机

当前 `std/codec` 的公开接口为：

```telora
codec.decode(User.type, value)
codec.encode(Value.type, user)
```

`std/json.decode` 也把 `TypeOf(A)` 作为普通值继续转发。其实现最终已经依赖 Sealed MIR 中
确定的目标布局、PropertyRecord 和构造检查，但表面协议仍保留动态类型阶段的
形状。这产生三个问题：

1. 调用者直接把类型物化到值域，行为能力没有通过 trait contract 表达；
2. codec 实现需要同时理解目标 Type 值、多个独立 Property 类型和结构布局；
3. 外部代码可以逐渐依赖 Property 查询细节，使未来 Property 静态化或替换更加困难。

RFC 0303 已验证 `FromStr` 的正确边界：返回 `Self` 的能力可以通过静态 trait 实例封闭，
Property 只为选定 impl 提供声明数据。Codec 应采用相同分层。

另一方面，并非所有声明配置都适合强制存在。例如可选扩展名、格式提示、字段策略和调试名
只有部分类型提供。运行时动态查询会丢失依赖闭包；把每个可选项都塞进一个永久增长的公共
结构也不合理。因此需要 `?Property(P)` 表达稳定的可选依赖。

## 范围

本 RFC 包含：

- 定义公开的 `Encode`、`Decode` trait 与静态 helper；
- 将 `codec` 和 `json` 的类型化入口迁移为显式类型实例；
- 定义共享 `CodecProp` 的职责、默认值和 decorator 合并规则；
- 定义 `?Property(P)` 的类型、coherence、MIR、初始化和依赖闭包语义；
- 规定派生 codec 与 exact 用户 impl 的优先级；
- 删除公开 API 中基于普通 Type 值选择 codec 行为的旧路径。

本 RFC 不包含：

- 将全部 Property 值改为编译期常量；
- 动态 `decode_type(Type, Value) -> Dyn`；
- `dyn Decode` 或完整 dyn Trait；
- JSON/YAML/TOML parser 自身的格式策略；
- `String <-> T` 转换；该能力属于独立的 `FromStr` / `Display` trait；
- 允许 optional Property 参与 impl 选择或表达负约束；
- 立即删除所有 `get_type_prop` 类底层能力；第一阶段允许派生实现继续使用受信任桥接；
- 立即合并所有历史 Property。迁移允许先通过 optional evidence 消费旧配置。

## 公共 trait

`std/codec` 导出：

```telora
trait Encode {
    encode: Fn(Self) -> Value,
};

trait Decode {
    decode: Fn(Value) -> Result(Self, BlameError),
};

def encode: for(T: Encode) Fn(T) -> Value = fn(value) {
    Encode.encode(value)
};

def decode: for(T: Decode) Fn(Value) -> Result(T, BlameError) = fn(value) {
    Decode.decode(value)
};
```

典型调用为：

```telora
let wire: Value = codec.encode(user);
let user: User = codec.decode@[User](wire).unwrap!();
```

`Encode` 的类型通常可以从参数推导。`Decode` 的 `Self` 只出现在返回值中，调用点必须通过
显式类型实参或周围完整类型约束确定 T。`codec.decode(type_value, value)` 不再作为静态
入口保留。

`Decode` 返回裸 `Self`，因此与 `FromStr` 一样不是 dyn-compatible。未来若需要运行时目标
类型，必须定义独立的擦除后协议并返回 `Dyn`；不能让静态 API 暗中恢复 TypeId 分派。

## Value 与文本格式边界

`Encode` / `Decode` 只负责：

```text
T <-> std::value::Value
```

文本格式保持独立：

```text
JSON text <-> Value <-> T
YAML text <-> Value <-> T
TOML text <-> Value <-> T
```

这里是两个独立、可单独使用的转换边界，而不是一个同时理解文本格式与目标类型的 codec：

- `json.parse` / `json.stringify`、`yaml.parse`、`toml.parse` 负责 `String <-> Value`；
- `codec.Decode` / `codec.Encode` 负责 `Value <-> T`；
- `json.decode@[T]` 等便利接口只能按顺序组合上述两段，不定义额外的转换语义。

因此任意格式 parser 的输出都可以先作为 `Value` 被检查、修改或转交；同一个 `Decode`
实现也可以复用于 JSON、YAML、TOML 或直接构造的 Value。格式错误由 parser 报告，Value 与
目标类型的结构失配由 Decode 报告。

因此 `std/json` 的类型化入口变为：

```telora
def decode: for(T: codec.Decode) Fn(String) -> Result(T, BlameError) = fn(text) {
    match parse(text) {
        Ok(value) => codec.decode@[T](value),
        Err(error) => Err(error),
    }
};
```

JSON 的注释、数字、字符串和深度策略属于 parser，不进入 `CodecProp`。CodecProp 描述的是
Value 结构映射，例如字段命名和 enum 表示。

这一边界是强制语义。派生 codec 不得因为输入节点是 `Value.String` 就自动调用 FromStr，
也不得自动调用 Display 产生 `Value.String`。需要字符串 wire shape 的类型必须提供 explicit
Encode/Decode impl；该 impl 可以像普通代码一样显式复用 FromStr/Display。

## 共享 CodecProp

当前 `JsonRenameAll`、`JsonUntagged` 等 Property 分别发布，codegen 再按固定列表查询。目标
是用一个稳定的共享 Property 表达常规 codec 配置：

```telora
@property(PropertyTarget.Type | PropertyTarget.Member)
type CodecProp = struct {
    rename_all: Option(RenameCase),
    untagged: Bool,
};
```

上例只表达第一阶段字段。新增字段必须具有明确默认值；不能让 Property 记录的缺失和结构
字段的缺失共同决定行为。

Decorator 修改同一个 Property head：

```telora
def rename_all:
    Fn(RenameCase) -> Fn(Type, Option(CodecProp)) -> CodecProp =
fn(case) {
    fn(target, previous) {
        let prop = previous.unwrap_or(codec.default_prop);
        {...prop, rename_all: Some(case)}
    }
};
```

```telora
def untagged: Fn(Type, Option(CodecProp)) -> CodecProp = fn(target, previous) {
    let prop = previous.unwrap_or(codec.default_prop);
    {...prop, untagged: True}
};
```

现有 Property provider 顺序、previous 链、重复声明诊断、类型/字段/variant site 和初始化失败
语义保持不变。共享 Property 不允许用“最后一个 decorator 静默获胜”掩盖本应诊断的配置
冲突。

### 默认配置

未声明 decorator 的普通 record、enum、newtype 当前也能派生 codec，迁移后不能要求用户为
每个类型增加 `@codec.derive`。默认 CodecProp 按需提供：

- 只有可达的派生 Encode/Decode 实例要求它；
- 没有用户 provider 时使用 `codec.default_prop`；
- 有 provider 时，provider 链从同一默认值开始修改；
- 默认配置不得要求为全图所有类型预先分配运行时对象；
- 默认配置与显式配置产生同一种 sealed codec plan。

具体实现可以在 MIR/初始化计划中合成默认 provider，但不得通过运行时遍历全部类型来补齐。

### CodecProp 的边界

CodecProp 容纳普遍参与 `T <-> Value` 结构映射的配置。具有独立生命周期、只被少数扩展使用
或不适合稳定加入共享结构的能力可以继续使用单独 Property，并通过 `?Property(P)` 被派生
impl 可选消费。

`FromStr`、`Display` 本身仍是独立行为 trait，不变成 CodecProp 字段，也不由派生 codec
自动调用。现有 `DecodeByParse`、`EncodeByDisplay` 混合了 `String <-> T` 和 `Value <-> T`
边界，属于待删除的动态阶段遗留，而不是 CodecProp 的候选字段。

## 派生 impl

标量、Value 和特殊内置类型可以提供 exact impl。Array、Option、Dict、Tuple 等结构容器通过
递归 trait obligation 派生。Record、newtype 和 enum 的派生实现消费 sealed layout 与
CodecProp。

概念上：

```telora
impl(T: Property(CodecProp)) Encode for T {
    encode: fn(value) {
        codec.encode_derived@[T](value)
    }
};

impl(T: Property(CodecProp)) Decode for T {
    decode: fn(value) {
        codec.decode_derived@[T](value)
    }
};
```

这只是能力边界，不要求把 layout traversal 写成运行时反射。MIR 为具体 T 选择 impl、闭合
递归成员和 Property 依赖；codegen 机械生成 adapter；native RT 只执行满足既有 native 准入
原则的同构低层操作。

用户 exact impl 按 RFC 0260 的既定规则优先于派生 fallback：

```telora
impl codec.Encode for Endpoint { ... };
impl codec.Decode for Endpoint { ... };
```

两个适用 exact impl、两个适用 fallback 或无法证明唯一选择仍是静态冲突。不得根据 Property
值或运行时 TypeId 决定 impl。

## 可选 Property bound

### 语法与含义

通用 bound：

```telora
?Property(P)
```

表示当前泛型实例可以可选地使用 P。它不要求 P 存在，也不表示 P 必须不存在。例如：

```telora
impl(T: Property(CodecProp) + ?Property(JsonExtension)) Encode for T {
    encode: fn(value) {
        match property.optional@[JsonExtension]() {
            Some(extension) => ...,
            None => ...,
        }
    }
};
```

`property.optional` 是概念接口；最终表面 API 必须从当前 bound 环境取得 evidence，不接受普通
`Type` 作为 owner 参数，也不能查询未在签名声明的 Property。

## Property bound 与读取权限

长期语义应把 Property bound 与 Property 读取能力连接起来：

```telora
T: Property(P)
```

不仅证明 P 存在，也向当前泛型环境提供读取该确定 Property evidence 的权限；对应地：

```telora
T: ?Property(P)
```

提供 `Option(P)` 形态的封闭读取权限。函数体不能仅凭普通 `T.type`、`P.type` 调用开放的
`get_type_prop` 并搜索 Property 表。

目标接口在概念上类似：

```telora
property.required@[P]() -> P
property.optional@[P]() -> Option(P)
```

owner T 来自当前 bound environment，而不是值参数。MIR 将调用直接绑定到 required
PropertyId，或 optional evidence 的 Present/Absent 结果。

这一收口具有两个作用：

- 类型签名完整声明函数体可能读取的 Property，函数依赖图无需从动态 Type 值反推；
- 普通代码不能绕过 trait/bound contract，把任意 Type 与 Property 类型组合成运行时查询。

但本 RFC 第一阶段不强制一次性删除现有 `get_type_prop`。Codec trait 迁移可以先通过受信任的
compiler-owned derived impl 桥接现有查询；完成 required/optional evidence 的 MIR 表达后，
再迁移标准库实现并缩小或删除开放查询。桥接不得暴露为新的公共 codec API，也不得在
codegen 中重新做全表探测。

### 封闭结果

对具体函数实例，optional evidence 有三种静态结果：

```text
Absent
Present(PropertyId)
Conflicted(PropertyConflict)
```

- `Absent` 合法，函数体观察到 `None`，不产生 Property 依赖；
- `Present` 合法，函数体观察到 `Some(P)`，PropertyId 加入该实例依赖闭包；
- `Conflicted` 是静态错误，不能降级为 `None`。

Property provider 已声明但初始化失败属于 `Present` 的运行期失败，不属于 `Absent`。访问该
evidence 必须传播现有初始化失败，不能用默认行为掩盖错误。

### 不参与 impl 选择

Optional bound 只向已经选中的 impl 提供静态参数。Coherence、适用性、优先级和重叠检查
必须忽略 optional bound。因此：

```telora
impl(T: Property(A) + ?Property(B)) Encode for T { ... };
impl(T: Property(A) + ?Property(C)) Encode for T { ... };
```

两者具有相同的选择模式并发生重叠，不能根据 B/C 是否存在择一。

禁止把以下写法解释为“仅当 B 不存在”：

```telora
impl(T: ?Property(B)) Encode for T { ... };
```

`?Property(B)` 不是 negative bound。增加或删除 B 不得改变函数族或 impl 的选择身份，只能改变
已选具体实例内部的 optional evidence。

### 泛型与实例身份

模板 impl 可以携带 optional bound，但进入 Sealed MIR 的具体实例必须已经知道每项 evidence
是 Present 还是 Absent。相同函数原型在不同 T 上可以得到不同 evidence；它们本来就是不同的
封闭函数实例。

实例身份包含已解析 optional evidence，或通过确定的 TypeId/PropertyId 闭包唯一导出该信息。
不得在 codegen 时重新扫描 Property 表，也不得让 Wasm 在首次调用时缓存探测结果。

## MIR 与依赖闭包

Trait impl 实例至少记录：

```text
ImplementationInstance {
    required_properties: [PropertyId],
    optional_properties: [Absent | Present(PropertyId)],
    function_dependencies: [...],
}
```

求解顺序为：

1. 仅用 required bounds 和普通 trait bounds 选择唯一 impl；
2. 对具体类型解析 optional Property；
3. Conflicted 产生静态诊断；
4. Present 的 PropertyId 写入 impl/function dependency；
5. 沿已选 impl、递归成员 trait 和 Property provider 完成封闭；
6. seal 拒绝仍未确定的 optional evidence。

这与 RFC 0305 的函数体依赖互补。函数体通过 optional evidence 读取 Property 时，依赖边必须
来自 bound 的封闭结果，而不是从值域中的 `T.type` 反向猜测。

只有可达的 impl 实例保活 Present Property。Absent 不产生假根；未引用 optional evidence 的
实现可以由后续依赖裁剪省去对应初始化，但第一版允许保守地保活签名中全部 Present evidence。

## Property 初始化

本 RFC 保持现有初始化模型：

- Property provider 仍是普通 `.telora` 函数；
- provider 链仍可读取 previous 并产生运行时值；
- Property demand 仍在初始化 work world 中按需执行；
- 初始化成功、失败和循环状态仍由现有状态表管理；
- codec 调用只读取 sealed slot，不查询 `(TypeId, PropertyTypeId)` 全表。

因此“Property 是否存在”在 MIR 阶段封闭，“Property 的值是什么”仍可在初始化阶段计算。这是
有意保留的分层，而不是半动态兼容路径。

## 迁移计划

### 阶段一：trait 外壳

1. 在 `std/codec` 定义并导出 Encode、Decode；
2. 为现有 sealed codec planner 提供 compiler-owned derived impl；
3. 新 API 使用 `codec.encode(value)` 与 `codec.decode@[T](value)`；
4. 保持各文本模块的 parser 独立返回 Value；
5. `std/json.decode@[T]` 只组合 `json.parse` 与 `codec.decode@[T]`；
6. 保持现有 Property 和 runtime plan，验证行为与诊断等价。

### 阶段二：optional evidence

1. 解析 `?Property(P)` bound；
2. type resolver 将它与 required Property 分开记录；
3. coherence 忽略 optional bound；
4. MIR/seal 记录 Absent、Present、Conflicted；
5. 函数依赖图保活 Present PropertyId；
6. 增加 Property 缺失、存在、冲突和初始化失败测试。

同时为 required/optional bound 定义受约束的 Property 读取接口。该接口在本阶段可以先供
标准库和 compiler-owned impl 使用；删除所有旧 `get_type_prop` 调用不作为本阶段完成条件。

### 阶段三：共享 CodecProp

1. 定义默认 CodecProp；
2. 将 JsonRenameAll、JsonUntagged provider 改为更新 CodecProp；
3. 用 optional evidence 兼容尚未合并的扩展 Property；
4. codegen 只消费一个已封闭 codec 配置视图；
5. 删除已无消费者的旧 Property 类型和按固定列表查询逻辑。
6. 将依赖 `DecodeByParse` / `EncodeByDisplay` 的模型迁移为 explicit Encode/Decode impl，随后
   删除这两个隐式文本桥接 Property。

### 阶段四：删除旧 API

1. 迁移标准库、语言 fixture、lab-ontology 和外部示例；
2. 删除 `decode(TypeOf(T), Value)`、`encode(TypeOf(Value), T)`；
3. 删除运行时 codec TypeId 分派和名称匹配；
4. 更新正式文档，不保留永久双轨入口。
5. 将普通 Property 读取迁移到 bound evidence，并审计、收窄或删除开放 `get_type_prop`。

## 诊断

至少提供以下静态诊断：

- `codec.decode` 无法确定目标 T；
- T 没有唯一 Decode 或 Encode 实现；
- optional Property 存在重复/冲突 provider；
- impl 尝试读取未在 required/optional bounds 中声明的 Property；
- 两个仅 optional bounds 不同的 impl 重叠；
- seal 时 optional evidence 仍未确定；
- 派生 codec 遇到不支持的 sealed layout。

运行期 Property provider 失败继续使用现有来源链，不能被渲染为“没有 Encode/Decode impl”。
Decode 的数据失配继续返回带输入来源的 `BlameError`。

## 确定性与代码生成

- 相同 Sealed MIR 必须产生相同的 impl 与 optional evidence 列表；
- optional Property 的声明顺序不改变 PropertyId 选择或制品；
- codegen 不扫描类型表、Property 表或函数名；
- Absent 分支可在 codegen 中直接消除；
- Present 直接引用稳定 Property slot；
- CodecProp provider 的求值顺序沿既有确定性初始化计划；
- exact impl 与 derived impl 的选择在 MIR 阶段完成。

## 放弃的方案

### 继续公开传递 Type

这会保留动态类型阶段的协议，使调用者和 codegen 都依赖值域 TypeId，无法建立清晰的 trait
obligation 与函数依赖闭包。

### 每项配置永久使用独立 Property 并运行时探测

独立 Property 可以保留给真正独立的扩展，但运行时探测不能表达稳定依赖，也会重新引入开放
TypeId 查询。共享常规配置加 optional evidence 更明确。

### 所有配置强制塞入 CodecProp

这会让 CodecProp 无限增长，并让少数扩展污染所有类型。CodecProp 只容纳稳定、普遍的结构
映射配置；长期可选能力使用 `?Property(P)`。

### 让 optional Property 参与 specialization

这等价于存在/不存在约束。增加一个 annotation 就可能改变 impl 选择，破坏函数实例身份和
开放模块组合。本 RFC 明确禁止。

### 立即静态化全部 Property

Trait 化 codec 不要求先解决 Property 值的编译期求值。先建立公共行为边界和稳定依赖，后续
才能独立评估哪些 Property 可以变为常量布局。

## 验收条件

1. `codec.encode(value)` 只接受具有唯一 Encode evidence 的类型；
2. `codec.decode@[T](value)` 在 MIR 中持有确定的 Decode impl；
3. `json.decode@[T](text)` 不传递普通 Type 值；
4. exact 用户 impl 优先于 compiler-derived fallback；
5. 默认 record/enum 无需 annotation 仍可派生 codec；
6. rename_all、untagged 的行为和来源诊断在迁移前后等价；
7. `?Property(P)` 的 Absent 与 Present 都能封闭并生成确定代码；
8. 冲突 Property 不得降级为 Absent；
9. optional bound 不参与 impl 选择，只有 optional bound 不同的 impl 被诊断为重叠；
10. Present Property 进入函数依赖闭包，Absent 不产生运行时探测；
11. provider 初始化失败保持原始诊断，不伪装成缺失 trait；
12. Wasm 中不存在 codec 的全 TypeId/PropertyId 扫描；
13. 旧 TypeOf codec 入口完成仓库迁移后删除；
14. language fixtures、codec/regex tests、lab-ontology 和 release 性能基线通过。
15. 派生 codec 不调用 FromStr/Display；字符串 wire shape 只由 explicit codec impl 提供。

`get_type_prop` 与 bound evidence 的全面连接是后续验收项，不阻塞第一阶段 trait 外壳；但任何
新增 codec 路径都不得扩大开放 Property 查询面。
