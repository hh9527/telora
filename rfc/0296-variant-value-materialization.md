# RFC 0296：enum variant 的值物化与来源边界

状态：已实现；由 #188 跟踪落地。
关联：[#188](https://github.com/hh9527/telora/issues/188)。

## 动机与范围

当前 `let a = False` 的诊断 subject 可能追溯到 prelude 的 `export Bool.{True, False}`，
而非用户表达式中的 False。根因是把类型域成员身份的传播与值来源的传播混在一起。
Bool 的生成逻辑与其他 enum 也不一致；后者在 Wasm codegen 中沿 declaration initializer
追溯 variant，可能把普通值引用重新解释为构造动作。

本 RFC 统一 Bool、Option 的 variant 及用户 enum 的无载荷值和有载荷构造器。
不增加关键字，不按 True/False/Some/None 的名字识别身份，不改变运行时值布局。
newtype 构造器需在拆除共用后端追溯逻辑时明确保留其既有行为，不能作为兼容后门；
本 RFC 不另行扩展其表面语义。

## 类型域与值域

以现有语法 `type Foo = enum { A(Int), B };` 为例，Foo 及其 A/B 成员首先确定类型域
身份。类型声明不是预先构造用户值的执行位置。import、成员导入和 re-export 只为
这些身份提供可 resolve 的名字，不形成值来源链。

在需要值的表达式位置使用 variant，发生类型域到值域的物化：

| 表达式 | 物化结果 | 值来源 |
| --- | --- | --- |
| `Foo.B` 或导入的 `B` | 无载荷 enum 值 | 当前 variant 表达式 |
| `Foo.A` 或导入的 `A` | 有载荷构造器函数值 | 当前 variant 表达式 |
| `False`、`Bool.False` | Bool 值 | 当前 variant 表达式 |
| `Some` | 经类型求解确定实例的构造器函数值 | 当前 Some 表达式 |

这就是 variant 在行为上如同 literal 的含义。物化只在静态求解后执行，不能为确定
类型而运行构造器。泛型参数在 MIR seal 前完成约束求解；本规则不允许未闭合的
函数族成为运行时值。

Foo 本身仍不能隐式成为普通数据；`Foo.type` 是显式类型元数据值表达式。本次不
修改元数据的相等性、布局或反射契约，也不禁止 LSP 导航到类型/variant 声明。
“没有值来源追溯”不等于“没有定义引用关系”。

## 来源语义

```telora
let a = True;
#       ^^^^  值来源从这里开始
let b = a;
```

b 仍保留 a 中 True 的位置，不改成 b、a 的读取位置或 prelude 导出位置。
两次独立出现的 True 各有自己的来源；来源差异不改变 Bool 的相等语义。

```telora
def disabled = False;
let value = disabled;
```

disabled 是已经进入值域的普通定义。读取、导入或重导出 disabled 都保留定义中
False 的来源，不得沿 initializer 追溯后在读取点重新物化。
同理，`let wrap = Foo.A;` 产生构造器值，后续传递 wrap 保留 Foo.A 的来源。

对于 `arr |> map\(_, Some)`，Some 在该表达式物化为构造器值，不追溯到声明。
构造器被调用时，产生的 enum 外层值继承构造器物化来源；payload 保留传入元素自己的
来源。因此 map 内部的间接调用位置不会替换 Some 或数组元素的来源。
直接调用 `Foo.A(x)` 遵循同一规则，外层来源是 Foo.A，payload 来源来自 x。

本规则只定义值来源。fail!/raise! 的主 rule 位置以及已有 contextual 调用规则不因此
改写；subject 标签消费上述值来源。值经过字段访问、绑定、参数传递和返回时，不因
本 RFC 增加来源覆盖动作。

## MIR 与 codegen

1. resolve/symbol 信息区分类型域的 variant 身份别名与普通值符号。导入/重导出
   链条只闭合身份；不能把 def/let 的 initializer 当成别名继续穿透。
2. 类型求解在值表达式处记录显式物化事实：确定的 enum TypeId、variant 身份、
   无载荷值或构造器种类、构造器的封闭签名，以及该表达式 Loc。
   可在既有 HIR 节点上挂载 MIR 记录，不要求修改 lossless CST 或重建语法树。
3. seal 验证这些事实完整，包括构造器实例及其 payload 类型。后续阶段不重新解析
   符号、不猜测类型，也不沿绑定 initializer 推导构造器身份。
4. codegen 机械生成物化动作。普通值读取走现有引用路径。间接构造器调用利用
   构造器值携带的来源生成外层值；共享机器码可复用，不能把不同物化点的来源合并。
5. 删除 enum 后端的 initializer 追溯和 Bool 的分离特例。审计共用 selection
   查询的规划、类型应用、函数值生成及 newtype 调用点，全部消费明确的 MIR 事实。

运行时不原地修改共享值，不深拷贝 payload。物化所需的来源使用现有值头；如果当前
构造器调用胶水未传递该来源，应在生成的胶水中补齐，不能改写 payload 的来源替代它。
声明的装饰器、构造检查仍按既有规则执行，不因 literal 行为而绕过。

## 实施顺序

先建立 MIR 身份/物化记录并完成 seal，再切换 codegen 及构造器调用胶水，完整移除
旧追溯路径。最后用独立 .telora fixtures 验证来源，并同步 LANGUAGE、CONCEPT 和
IMPLEMENTATION 的当前契约。实现按这一个 RFC 和 #188 推进，不另拆伞 issue。

## 验收

- #188 的原始 Bool 用例：主诊断仍在 fail!，subject 精确指向用户代码中的 False。
- 无载荷用户 enum、Bool、None 的直接选择、成员导入、改名导入及多层 re-export，
  都在实际值表达式位置建立来源；同名用户符号不能被按文本误识别。
- 两个独立物化点互不污染；变量、普通 def 导出、参数、返回及容器中的既有值
  保持原来源。覆盖先物化再导出的值与直接重导出 variant 身份的区别。
- 有载荷 variant 覆盖直接调用、先绑定再调用、跨模块传递和 map 等高阶调用。
  分别验证 enum 外层值、构造器值（适用的诊断入口）及 payload 的来源。
- 隐式泛型推断与显式类型应用得到同一封闭构造器身份；不同源码位置只影响来源，
  不改变值相等语义。相关构造检查仍生效。
- LF/CRLF/CR 的位置映射一致；来源范围使用既有 Loc 表达，主位置规则不变。
- .telora 测试资产通过运行时读取，不使用 include_str!。确切来源/标签断言可保留
  少量 Rust Host 测试；MIR 测试验证物化事实和普通引用的区别，不能只检查成功退出。
- 不允许遗留后端沿 initializer 猜测 variant 的路径。

## 不采用的方案

- 把 True/False 变成关键字或按名称特殊处理：破坏 prelude 与稳定身份规则。
- 在所有 let/读取点重写 Loc：会丢失已有值来源。
- 沿声明追溯到 variant 后重新构造：混淆类型域身份别名与普通值绑定。
- 原地改写共享值来源：污染其他使用处。
- 只给 Bool 补上与 enum 一样的后端追溯：保留根本的域边界错误。

## 落地结果与验证

- MIR 增加逐表达式的 `value_materializations`，与既有的节点 TypeId、泛型实例类型
  和 Loc 共同形成完整物化事实。身份闭合只穿透成员导入/重导出，普通值绑定终止追溯。
  seal 校验事实完整性与构造签名，缺失、伪造物化记录或不匹配类型均被拒绝。
- 执行闭包不为类型域别名引入运行时依赖；模块检查不将此类别名加入初始化根。
  后端 enum selection 仅读取物化记录，pattern 单独读取已求解的匹配事实。
- enum 构造器按封闭签名和 variant 复用代码，函数值保留各自的来源头；零环境
  构造器调用将 callable 地址传给生成的胶水，外层复制该来源，payload 保留来源。
  普通 closure 仍使用非零环境句柄。内部 ABI 14 升为 15，Rust/JS Host 同步，值布局不变。
- seal 与胶水显式支持既有的 TypeOf(T) 到 Type 元数据擦除，例如
  `Some(Int.type)` 在 Option(Type) 上下文中仍合法，且不丢失 payload 来源。
- 独立资产位于 `crates/telora-wasm/tests/fixtures/variant-origins/`，Host 测试运行时
  读取。覆盖 Bool/None/用户 enum 的多层别名、普通值导出、泛型构造器值、多次物化、
  高阶 map、外层与 payload 的精确范围、主诊断不变，以及三种 EOL 的一致性。
  构造器相等性和 seal 负向测试同时覆盖。
- 验证通过：core 136 项、Wasm 75 项、CLI 97 项（包含 414 个语言测试入口）；
  新增来源/seal 测试在补齐多层 Bool/Option 别名后单独复验通过。
  Node 位置测试、ABI 15 debug smoke、源文件大小检查和 git diff --check 通过。
