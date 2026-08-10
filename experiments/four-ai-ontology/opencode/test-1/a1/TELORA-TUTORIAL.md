# Telora 最小语言教程

这不是完整语言手册，是完成本项目所需的 Telora 语言基础。掌握这些之后，
你将阅读 a1/EDSL-DESIGN.md 并实现一个 ontology 嵌入式 DSL。

Telora 是一门确定性的、纯函数式的意图编译语言：不可变值、无副作用、类型即
数据。程序由绑定（let/def/type）和表达式组成，模块通过显式导入/导出连接。

## 值

```telora
42            # Int
3.5           # Float
"hello"       # String
'Ready        # Atom（单引号前缀的名字）
'True 'False  # 内建 Bool atom（注意：不是 True/False）
'None         # Option 的空
'Some(1)      # Tagged 值：Atom 加一个 payload
('Ok, 1)      # 这是 Tuple，不是 Tagged
[1, 2, 3]     # Array
{name: "Ada", age: 36}   # Dict（字段顺序不重要，按规范排序）
```

Bool 值一定是 `'True` / `'False`。不要写裸的 `True`。

## 绑定

```telora
let answer = 40 + 2;
def name = fn(value) { value };
```

`def` 用于命名函数；`let` 用于普通不可变绑定。块是 `{ ... 表达式 }`，
块的最后一行是它的值。

## 函数

```telora
fn(value) { value + 1 }
```

函数签名（用于标注）写作 `Fn(A, B) -> C`。泛型用 `for(...)`：

```telora
def identity: for(A) Fn(A) -> A = fn(value) { value };
```

参数和结果类型是可选的注解；省略时按上下文推断。

## 封闭类型

```telora
@enum type Entity = {
    Ticket: 'None,
    Agent: 'None,
};

@struct type Item = {
    name: String,
    nodes: Array(Entity),
};
```

- `@enum`：一组带可选 payload 的 tag。单元 variant 是 `'Ticket`；
  带 payload 的 variant 是 `'Tag(payload)`。
- `@struct`：命名字段的记录。字段用 `.field` 访问（如 `item.name`）。
- 枚举和结构体都是**封闭**类型：只能使用声明的 tag/字段。

## 模式匹配

```telora
match value {
    'Some(found) => found,
    'None => fallback,
}

match result {
    'Ok(value) => value,
    'Err(error) => error,
}
```

`_` 是通配符。标签 `'Tag(pattern)` 匹配 Tagged 值并绑定 payload。
匹配必须是穷尽的（封闭枚举）或提供 catch-all。

## 数组

```telora
array.find(values, predicate)      # Option(A)
array.all(values, predicate)       # Bool
array.map(values, mapper)          # Array(B)
array.flat_map(values, mapper)     # Array(B)
array.concat([left, right])        # Array(A)
array.filter(values, predicate)    # Array(A)
array.fold(values, initial, fn(acc, item) -> acc)
array.push(values, item)           # 返回新 Array
```

当泛型结果难以推导时（例如 `array.flat_map` 的结果类型无法从上下文确定），
定义一个带完整签名的辅助函数，或给局部 binding 写具体类型注解。

## 诊断与错误

领域规则拒绝一个值时，报告错误并返回 `'None`（或 `'Err`）：

```telora
let ignored = emit_error!("no capability defined for \{id}", authored_value);
'None
```

- `emit_error!(message, subjects...)`：报告一条带来源的错误，表达式本身的值是
  一个可丢弃的 BlameError 记录。它**不会**中止当前函数——后续独立检查继续。
- `blame!(message, subjects...)`：构造一个 BlameError（不报告）。
- `raise!(error)`：终止当前函数，抛出结构化错误。表达式类型是 `Never`。
- 字符串插值用反引号：`` `no capability for \{id}` ``。

## 模块

依赖清单是 JSON：

```json
{"dependencies":{"my-lib":{"path":"../my-lib"}}}
```

导入与导出：

```telora
import "std/array" as array;
import "./local.telora" { compile };
import "my-lib/types.telora" as types;

export def compile: Fn(Input) -> Output = fn(input) { ... };
export { Entity, Measure, ExecutionPlan };
```

## 类型即数据（TypeMetadata）

类型是普通值，可以用函数构造。`TypeOf(T)` 是"描述 T 的类型元数据"的精确见证。
内置类型族：

```telora
Option(T)          # for(A) Fn(TypeOf(A)) -> TypeOf(Option(A))
Array(T)
Struct(fields_dict)   # fields 是 Dict(String, TypeMetadata)
Enum(variants_dict)
Fn(A, B) -> C
```

用户函数可以构造具体结构类型：接受 `TypeOf(...)` 参数，用 `struct('None, {...})`
返回一个类型记录。消费它的声明会在 tool-stage 求值并得到完整结构类型。

```telora
def Capability:
    for(Id, Input, Output)
    Fn(TypeOf(Id), TypeOf(Input), TypeOf(Output)) -> Type =
    fn(Id, Input, Output) {
        struct('None, {
            id: Id,
            lower: Fn(Id, Input) -> Option(Output),
        })
    };
```

注意：用户定义的类型族在泛型签名里**不能**精确命名自己的结果（会退化为 `Type`），
因此共享函数通常"具体实例化类型 + 通过 typed 选择器/回调访问字段"。

## 调用约定要点

- 一个调用产生一个值；无副作用；同输入必得同输出。
- 泛型函数通过显式选择器/回调接收模型特定的投影（而不是依赖 `Any` 或类型擦除）。
- 保持类型精确：**不要用 `Any`、`Dyn` 或 String 身份来掩盖类型关系**。

## 你能读到的库

实现 eDSL 时，你可以使用 `std/array`（数组组合子）和语言内建（match、Tagged、
TypeMetadata 构造器）。不要使用本仓库中任何已有的 ontology/analytics 代码——
那些是参考实现，本实验要求你独立实现。

## 下一步

读完以上内容后，阅读 `a1/EDSL-DESIGN.md`，按其中的规范实现 ontology eDSL。
