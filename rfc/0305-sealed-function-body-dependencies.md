# RFC 0305：封闭函数体依赖图

- 状态：草案
- 跟踪：[#219](https://github.com/hh9527/telora/issues/219)
- 分支：`feat/rfc-0305-function-body-dependencies`
- 日期：2026-09-21
- 前置：RFC 0298（MIR Node mini-pass 调度）、RFC 0302（单 Vec 语言堆）
- 范围：Sealed MIR 尾段的函数体依赖分析；首版不改变语言语义

## 动机

Sealed MIR 已经封闭类型、泛型实例、符号解析和最终 executable binary，但目前没有
显式表达一个函数体在未来执行时可能使用哪些函数、顶层值和 property。运行时 GC
只能沿实际数据布局追踪 Record、Array、闭包环境等引用，无法从一个 FuncId 看出其
函数体将通过稳定 SymbolId 读取哪个顶层 demand。

RFC 0302 因此采用保守根集合：初始化完成后保留当前制品中所有 Ready demand。
这是正确但宽泛的首版策略。大型初始化图会同时保留最终结果和用于构造结果的所有
顶层中间值；它也阻碍精确 tree shaking、初始化调度和紧凑 snapshot。

本 RFC 在 Sealed MIR 与 codegen 之间增加后端无关的依赖分析，核心关系为：

```text
Func -> FuncProto -> Dep
```

`Dep` 来自函数体中已经 resolve 的逻辑，不表示数据对象直接持有的引用。

本 RFC 不承诺据此精确删除顶层 demand/property。完整可达性同时取决于运行时实际值、
动态 `TypeId`、property 结果和生成 helper；仅靠静态分析不能可靠恢复这些边。首版继续
把所有 Ready demand 作为初始化 collection 根，函数依赖元数据先作为可观察、可验证的
制品事实进入后端。顶层状态 tree-shake 需要独立 RFC 和反例驱动的正确性论证。

## 两类关系

实现必须区分函数体行为依赖与运行时数据依赖。

```text
函数体依赖：Func -> FuncProto -> Dep
数据依赖：  Value/Object -> Value/Object
```

函数体依赖包括：

- 直接调用或物化另一个函数；
- 读取顶层 SymbolId 对应的值；
- 查询已经封闭的 property 实例；
- 引用编译器生成的 codec、比较、解析或其他 helper；
- 调用函数值参数，形成待传播的高阶依赖位置。

数据依赖包括：

- Record、Tuple、Array、Dict 和 enum payload 中的值；
- 闭包环境捕获的局部值；
- property 结果或顶层值中实际保存的容器和函数值；
- Rust RT 资源句柄及其现有追踪规则。

数据依赖继续由布局驱动的 collector 追踪，不复制到 `Dep`。例如局部 `let value`
被 closure 捕获后是闭包环境中的数据边；顶层 `def value` 在函数体中通过稳定 ID
读取，则是函数体中的 demand 依赖。

## 身份与表示

### Func

`Func` 是 executable binary 中封闭的具体函数身份。它包含普通函数、泛型函数实例、
property provider 实例以及进入 executable 的编译器生成函数。每个 Func 具有稳定、
确定性的 FuncId，并指向一个 FuncProto。

同一个源码模板可以形成多个 Func：

```text
map@[Int, String]  -> map proto
map@[String, Bool] -> map proto
```

Func 携带实例化 FuncProto 依赖所需的类型替换、evidence 选择和其他封闭参数；这些
参数已经由 MIR sealing 确定，依赖分析不得重新执行类型推断或 trait 选择。

### FuncProto

FuncProto 表示共享的函数体及其依赖模板。用户闭包、具名函数体和确实拥有独立函数体
的编译器生成 helper 都可以具有 FuncProto。仅有别名而没有新函数体的实例不复制
FuncProto。

FuncProto 的身份不要求成为语言反射能力，也不进入普通 TypeId 空间。它是 MIR 尾段
和后端之间的编译身份。

### Dep

首版候选表示为：

```rust
enum Dep {
    Function(FunctionRef),
    TopLevel(SymbolId),
    Property(PropertyRef),
    FunctionParameter(u32),
    Generated(GeneratedFunctionRef),
}
```

实际落地可以统一 `Function` 与 `Generated` 的封闭目标表示，但必须保留以下区别：

- 已能从当前 Func 的替换环境确定的直接目标；
- 仍需从封闭调用点传播函数身份的高阶参数位置；
- 顶层值和 property 状态槽，而非普通函数代码。

Dep 集合按稳定身份排序并去重，不依赖 HashMap 迭代顺序。来源位置可以作为 debug
附属信息，但不参与依赖身份和确定性排序。

## 提取直接依赖

依赖提取只读取已经 sealed 的事实：

1. 扫描每个 FuncProto 的函数体。
2. 对具有 resolve slot 的函数/顶层引用读取最终绑定，不再按名称匹配。
3. 使用当前 Func 的实例替换把泛型函数引用物化为具体 FuncId。
4. 对已经选择的 property、codec、比较和解析实例记录其封闭身份。
5. 对调用的函数参数记录 `FunctionParameter(index)`，留给高阶传播。
6. 函数作为值被物化也建立函数依赖，不仅 `Call` 节点建立依赖。
7. 模式中的 enum variant tag 只消费已封闭类型布局，不因此建立值域函数依赖。

以下行为禁止：

- 通过函数、类型或 native 名称猜测依赖；
- 从最终 Wasm 字节码反汇编恢复语义依赖；
- 把闭包环境中的普通局部捕获重复登记为顶层 Dep；
- 因签名相同就把所有函数视为可能目标，而不先使用 executable 中已有的实例事实。

## 高阶依赖传播

函数体可以调用一个函数参数：

```telora
def apply: for(T, U) Fn(Fn(T) -> U, T) -> U =
    fn(f, value) { f(value) };
```

FuncProto 只能直接记录 `FunctionParameter(0)`。Sealed executable 是有限闭合图，分析
从所有封闭调用点向形参传播可能的 FuncId 集合，并迭代到不动点：

```text
parameter_targets(callee, index)
    += function_values(argument at every reachable call site)
```

函数值可以来自直接函数物化、闭包构造、参数转发、分支/匹配合流、Record/enum 中的
函数字段及其他 MIR 已封闭值流。首版采用保守并集，不做路径敏感分析；可以多保留
可能目标，但不能漏掉任何运行期可达目标。

若首版尚不能精确传播某类封闭函数值来源，必须以结构化 `UnknownFunctionFlow` 结果
暴露，或采用按封闭函数签名筛选的明确保守集合。不能静默产生缺边，也不能回退到
动态类型时期的名称匹配。

算法是有限集合上的单调增长，不设置任意迭代次数上限。work queue 在集合实际新增
FuncId 时才重新调度消费者，保证可停机和确定性。

## 解析后的依赖图

分析结果至少提供：

```rust
struct FunctionDependencies {
    functions: Vec<FuncId>,
    demands: Vec<DemandId>,
    properties: Vec<PropertyInstanceId>,
}
```

其中 `Func -> FuncProto -> Dep` 是构建与共享模型；消费者读取的是按 Func 实例化并完成
高阶传播后的依赖。传递闭包可以由统一分析结果提供，也可以由消费者从直接边计算，
但只能有一种规范排序和身份定义。

递归函数和相互递归函数形成 SCC，不是错误。SCC 内函数共享可达依赖集合或按普通
work queue 收敛；不按递归深度执行 Rust 调用栈遍历。

## 根与消费者

首版先形成分析和可观察结果，不立即改变所有消费者。

消费者按以下顺序接入：

1. debug/测试接口展示某 Func 的直接及传递依赖；
2. codegen 和链接验证依赖目标均属于同一个 sealed executable；
3. 将函数依赖元数据确定性地编码进 Wasm 制品；
4. collector 遇到函数值时能够查询对应元数据，但不据此缩小初始化根集合；
5. 后续独立 RFC 再评估 code/helper tree-shake、状态根裁剪和 snapshot。

若未来实施状态根裁剪，运行时保活必须是两种遍历的组合：

```text
从 service/property/命令结果追踪数据布局
    遇到函数值时追踪闭包环境
    并按 FuncId 查询函数体依赖
        保活可能读取的 demand/property
        再继续追踪这些槽中的数据
```

函数体依赖不能单独替代 GC；GC 也不能凭堆数据推断稳定 ID 指向的顶层值。反过来，
静态图也不能精确知道分支、容器、闭包环境和动态 property 查询最终产生的实际值。
因此函数依赖是运行时值追踪的语义补边，不是完整可达图。

`check` 的 best-effort 多根初始化与 `run/serve` 的运行期根可以不同。接入根裁剪前
必须分别定义命令语义，不能为了降低 service 内存而漏检 `check` 明确要求检查的项。

## 与源码作用域的关系

依赖分析不替代良好的源码生命周期表达：

```telora
def outcome = do {
    let snapshot = build_model(...);
    let ontology = build_ontology(snapshot);
    build_outcome(ontology)
};
```

局部中间值应优先写成局部值，由普通数据追踪决定是否随最终结果存活。若作者把
`snapshot` 和 `ontology` 写成独立顶层 def，它们具有独立顶层身份；依赖分析只判断
运行期函数是否仍可能按 ID 访问它们，不擅自改变语言作用域。

安全 collection point 是独立消费者：完整 demand 求值返回后已没有其 Rust/Wasm
局部栈根，才可以依据已发布槽和函数体依赖执行 collection。资源阈值不能代替依赖
正确性；是否以及何时触发 collection 由后续初始化调度 RFC/实现决定。

## 诊断与可观察性

首版提供测试用的稳定 dump，至少能表达：

- FuncId、FuncProtoId 与原始模块/符号；
- 直接 function/demand/property 依赖；
- 高阶传播得到的目标；
- 使用保守 fallback 的位置和原因；
- 从指定 executable root 得到的传递可达集合。

隐藏或测试接口可以先输出 JSON；未确定用户使用场景前不加入正式 CLI 文档。
dump 按稳定 ID 排序，同一 Sealed MIR 的全量构建结果必须确定。

## 实施步骤

1. 在 MIR/executable 层定义 FuncId、FuncProtoId 和 Dep 的最终归属，复用已有稳定实例
   身份，避免建立第二套互不对应的函数编号。
2. 提取无需高阶传播的直接依赖，并用 `.telora` fixture 覆盖顶层引用、普通调用、
   函数物化、泛型实例、property 和生成 helper。
3. 建立函数参数目标集合与 queue 固定点，覆盖参数转发、返回、分支合流和递归 SCC。
4. 提供确定性 dump 与图不变量检查；比较重复全量构建输出。
5. 先将 codegen/link reachability 切换到新图，确认行为和测试不变。
6. 保持所有 Ready demand 为初始化根，验证元数据接入不改变现有运行语义。
7. 另起 RFC 评审 demand 根裁剪；先覆盖运行时函数值、动态 `TypeId`、trait/property、
   property value 再含函数及生成 helper 等反例，并记录真实模型收益。
8. 实现稳定后更新 RFC 0302 的后继说明及当前设计文档；历史 RFC 不回写设计正文。

## 验收条件

- 依赖提取只使用 sealed resolve/instance/property 身份，无名称匹配。
- 相同 FuncProto 的多个泛型 Func 正确实例化为不同具体依赖。
- 函数作为值被引用但未立即调用时仍进入依赖图。
- 局部闭包捕获不被错误登记成 TopLevel demand 依赖。
- 高阶参数转发和递归调用图在有限 work queue 上收敛，无递归调度栈和任意轮数上限。
- 所有结果按稳定 ID 确定排序；重复全量构建的 dump 字节一致。
- 未支持的函数值流产生结构化保守结果，绝不漏边或按名称猜测。
- 接入 codegen 后现有语言测试行为不变，生成函数集合不增加。
- Wasm 制品携带按实际函数表身份索引的依赖元数据，collector 可以在函数值追踪时查询；
  初始化 collection 仍保守保留全部 Ready demand。
- 精确状态 tree-shake、生成 helper 的完整依赖身份和缩小初始化根集合不属于本 RFC 的
  验收条件，必须由后续 RFC 单独证明。

## 延后事项

- 路径敏感、常量条件和数据相关分支裁剪；
- 基于调用频率的函数布局或内联优化；
- 跨制品增量依赖缓存；
- 将依赖图作为公开语言反射 API；
- 精确资源成本或 fuel 计费；
- 仅为绕过 Wasm32 上限而引入阈值式中途 GC。
