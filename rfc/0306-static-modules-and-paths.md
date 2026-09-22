# RFC 0306：静态模块挂载与 `::` 路径

- 状态：草案
- 跟踪：[#220](https://github.com/hh9527/telora/issues/220)
- 分支：`feat/rfc-0306-static-modules-paths`
- 日期：2026-09-22
- 前置：RFC 0280（全图 MIR）、RFC 0305（封闭函数体依赖）
- 范围：代码模块挂载、类型化静态数据声明、静态路径语法及迁移

## 动机

Telora 当前用带路径的 `import` 同时承担源码发现、模块加载、名字绑定和静态数据引入。
这个模型在文件规模较小时直接，但它把四类不同关系混在一起：

- 一个 crate 包含哪些模块；
- 模块在逻辑模块树中的身份；
- 当前作用域使用哪些名字；
- 一个静态数据来源应按什么格式解码成什么类型。

这种混合也阻碍宏展开。宏定义体引用另一个模块的符号时，展开器不应拼接路径、注入
隐式 import，或在调用方作用域重新按文本猜测。它应能携带定义位置已经解析的稳定
模块和符号身份。

另一方面，`.` 当前同时出现在值投影和静态名字选择中。MIR lowering 必须借助左侧身份
判断一次选择属于模块域、类型域还是值域。完整静态化之后，这些域应在语法上可见。

本 RFC 将职责拆开：

```text
mod   构造代码模块树
data  声明类型化静态数据
use   在作用域中建立静态名字绑定
::    选择静态身份
.     投影运行时值
```

## 目标

1. 每个代码文件具有唯一、确定的模块身份和子模块路径。
2. 模块图由 `mod` 声明显式构造，不由 import 或目录扫描隐式扩展。
3. 静态路径与值投影在 CST 时即被区分。
4. 数据来源直接解码为声明类型，不强制产生临时语言 `Value`。
5. cname、ModuleId 和 SymbolId 不包含本地物理路径。
6. 为未来 hygienic macro expansion 提供 definition-site 静态身份。
7. 保持 `check --only-types` 不读取或解析数据内容。

## 非目标

- 本 RFC 不实现 Property-as-trait-evidence。
- 本 RFC 不新增运行时文件读取 effect。
- 本 RFC 不允许模块声明计算路径或格式。
- 本 RFC 不采用 Rust 的 `a.rs` / `a/mod.rs` 双重文件规则。
- 本 RFC 不要求在第一阶段实现宏系统，只固定宏可依赖的身份边界。

## 代码模块

代码模块使用声明驱动的单一映射：

```telora
mod query;
```

若当前模块逻辑路径为 `P`，该声明挂载逻辑模块 `P::query`。物理来源采用 Rust 新式
子模块布局，但只保留一种形式：

```text
crate root 中的 mod query;       -> src/query.telora
src/foo.telora 中的 mod query;    -> src/foo/query.telora
src/foo/query.telora 中的 mod x;  -> src/foo/query/x.telora
```

不搜索或接受 `query/mod.telora`。同一个子模块名在同一父模块只能声明一次。未被 `mod`
挂载的 `.telora` 文件不进入普通模块图；测试清单的特殊根选择保持独立规则。

`mod` 只接受标识符，不接受字符串路径、计算表达式或替代文件属性。模块发现因此是有限、
确定的图遍历。文件缺失、重复挂载和模块环在模块图阶段诊断。

crate root 由 crate manifest 选择；manifest 不枚举模块树。package Host 只把 crate root、
依赖 crate roots 和源码访问能力交给编译器。

## 静态路径 `::`

`::` 只在静态命名空间中选择：

```telora
crate::ontology::Query
self::helper
super::shared::Rule
Bool::True
Option::Some
Trait::associated
T::associated
```

可作为静态路径首段的身份包括 crate、module、type、trait、类型参数以及由 `use` 建立的
静态别名。每一段在 symbol/type resolve 中解析为稳定身份；Sealed MIR 不保留待运行时
解释的静态路径字符串。

`.` 只用于运行时值域：

```telora
record.field
value.method(argument)
```

类型元数据也遵守域的区分。由静态类型取得 metadata 的写法迁移为 `T::type`；对一个
运行时值做值域操作仍使用 `.`。本 RFC 不借 `::` 引入运行时 property 查询。

enum variant 属于类型域。`Bool::True`、`Option::Some` 等路径解析为 variant identity，
随后可以按既有规则物化为值；该物化不形成普通值引用的来源追溯关系。

## `use` 与可见性

`use` 只把已经挂载、已经解析的静态身份引入当前作用域：

```telora
use crate::ontology::Query;
use crate::ontology::{Intent, Rule};
```

`use` 不读取文件、不创建 ModuleId，也不能使未由 `mod` 挂载的源码进入模块图。名称冲突
在静态作用域中诊断，不通过导入顺序覆盖。

导出机制继续决定跨模块可见性。迁移期间可以先让现有 export 表达式消费新的静态路径，
是否改成独立 visibility 语法不属于本 RFC。

## 类型化静态数据

静态数据使用模块级 `data` 声明：

```telora
data config: Config = import(json) "config.json";
data ontology: Ontology = import(yaml) "ontology.yaml";
data package: Package = import(toml) "package.toml";
```

`import(format) "source"` 是 `data` 声明专用静态语法，不是 Call expression：

- `import` 和 format 不参与名字解析；
- format 不能被绑定、覆盖或动态计算；
- source 必须是无插值字符串字面量；
- 该语法不能出现在普通表达式中；
- 数据读取由编译和初始化管线管理，不构成用户程序 effect。

首版 format 集合为 `json`、`yaml` 和 `toml`。显式 format 是权威语义，文件扩展名不对其
施加约束：

```telora
data config: Config = import(json) "config.yaml";
```

上例始终按 JSON 解析。内容若是合法 JSON 就成功，否则报告 JSON 诊断；编译器不额外
报告扩展名不匹配。

source 按声明所在模块的逻辑资源目录解析并规范化，必须位于当前 crate 的源码访问边界。
Host 仍负责把规范化 source cname 映射到物理文件；物理路径不进入编译器身份。

### 迁移简写

迁移期接受：

```telora
data config = import "config.json";
```

它在 HIR lowering 时规范化为：

```telora
data config: std::value::Value = import(json) "config.json";
```

省略类型固定表示 `std::value::Value`，不是从使用处反向推导。省略 format 时才从已知、
大小写敏感的后缀确定 format：`.json`、`.yaml`、`.yml`、`.toml`。缺失或未知后缀产生
静态诊断。

规范化之后，MIR 不区分简写和完整写法，只包含明确的目标 TypeId、ModuleFormat 和
source identity。

### 直接构造目标类型

完整声明：

```telora
data config: Config = import(json) "config.json";
```

在类型阶段建立 `Config` 的静态 decode obligation。Sealed MIR 必须已经确定具体 decoder、
codec/property evidence 和目标布局。初始化阶段按该封闭计划直接构造 `Config`，语言语义
中不先产生 `std::value::Value` 再做动态转换。

parse-0 的 span tree、容器索引和解码缓冲区仍可作为实现细节存在，但不能成为可观察的
语言值或长期 heap 根。若声明类型本身就是 `Value`，生成 `Value` 是用户明确选择的结果。

`check --only-types` 验证 source 语法、format、目标类型和 decode obligation，不读取数据
内容。完整 `check`、`build` 和执行初始化读取来源、执行资源限制与解析，并发布最终顶层值。

## 身份模型

三类身份必须分开：

```text
模块身份： crate::ontology
值身份：   crate::ontology::config
来源身份： @src/ontology/config.json
```

代码模块和 data declaration 都进入模块依赖图，但只有代码模块拥有可继续挂载子模块的
namespace。data declaration 发布普通、类型化的顶层 SymbolId，并有独立 DataSourceId
关联其来源、format 和 SourceId。

数据来源位置用于解析诊断；`data` 声明位置用于类型、可见性和初始化依赖诊断。一个错误
可以同时携带来源中的主位置和声明处的相关位置。

## 宏展开边界

宏定义体中的静态引用默认在 definition site 解析。宏模板保存解析后的 ModuleId、
SymbolId、TypeId 或可重定位的规范身份，而不是保存一段将在调用方重新查找的 cname 文本。

宏参数携带 invocation site 的语法上下文：

```text
宏自身静态引用 -> definition context
传入宏的语法   -> invocation context
```

展开不能偷偷挂载新模块。宏可生成 `mod` 或 `data` 声明与否需要未来宏 RFC 单独决定；若
允许，它们也必须在模块图冻结前展开成普通声明节点，不能在 resolve 中途修改模块清单。

## 编译阶段

推荐管线为：

1. 从 crate root 解析足以发现 `mod` 和 `data` 声明的 CST。
2. 按 `mod` 声明构造完整、排序稳定的 ModuleId 图。
3. 登记 data source cname，但不读取内容。
4. 解析并 lowering 所有已挂载代码模块。
5. resolve `::` 路径和 `use` 绑定。
6. 完成类型、trait、property 与 data decode obligation 闭合。
7. 形成 ExecutionClosure 和 SealedExecutable。
8. 注入并直接解码被 executable 接纳的数据来源。
9. 完成顶层值/property 初始化并创建 service。

ModuleId 分配必须依赖规范化逻辑身份的稳定排序，不能依赖文件读取顺序、目录枚举顺序或
HashMap 迭代顺序。

## 迁移策略

实现按以下阶段推进：

1. tree-sitter/CST 增加 `mod`、`use`、`data`、`import(format)` 和 `::`，保留旧语法。
2. HIR 增加静态路径和 data declaration，不把它们伪装成 Call/普通 import。
3. package/module resolve 改为从 crate root 按声明构造模块图。
4. symbol/type resolve 消费 `::`，并为旧静态 `.` 输出迁移诊断。
5. 将 std、仓库内 `.telora` tests 与 lab-ontology 迁移到新模块系统。
6. data initialization 直接按声明目标类型生成计划。
7. 删除路径式代码 import、路径式数据 import 和静态 `.` 选择。
8. 更新正式文档；不把中间兼容开关写成长期语言能力。

迁移测试优先使用外部 `.telora` fixture，Rust 单元测试只覆盖身份、确定性和阶段不变量。

## 验收条件

- 代码模块只有一种文件映射规则。
- 未经 `mod` 挂载的代码文件不能通过 `use` 或宏被隐式加载。
- `::` 在 seal 前全部解析为稳定身份；`.` 不再承担静态名字选择。
- 同一 crate 在不同绝对目录中构建产生相同 module/symbol identity 和制品字节。
- 调换目录枚举和源码读取顺序不改变 ModuleId、诊断顺序或 Wasm。
- 类型化 data 不产生可观察的中间 `Value`。
- 显式 format 覆盖扩展名；省略 format 时后缀推导确定且有诊断。
- `check --only-types` 不读取 data 内容，完整 check 能报告来源内精确位置。
- 宏定义侧跨模块引用不依赖调用方 import，也不通过名称匹配恢复身份。
- 旧 import 与静态 `.` 在完成仓库和 ontology 迁移后被删除，而非永久双轨维护。

## 后续工作

在本 RFC 建立清晰的类型域和模块域后，可另立 RFC 将 Property 查询收束为 trait evidence：
普通代码通过静态 bound 获得 property provider，动态 TypeId 查询仅保留在明确的 reflection
边界。该工作不阻塞本 RFC 的模块与语法迁移。
