# Telora 标准库指南

本文是公开标准库的模块地图，帮助程序作者找到承载某项能力的模块。语言用法见
[`TELORA.md`](TELORA.md)，执行模式见 [`EXEC-MODE.md`](EXEC-MODE.md)，外部效果见
[`EES.md`](EES.md)。

标准库随 Telora binary 一起发布。接口以当前 binary 的查询结果为准，编写代码时应
优先使用工具发现类型和公开成员：

```bash
telora query modules -p std/
telora query exports std/array
telora query exports std/value -p Scalar
telora query at std/fmt -p display
```

模块使用规范 ID 导入：

```telora
import "std/array" as array;
import "std/value" {Value, ScalarValue};
```

## 基础与集合

- `std/prelude`：property 声明所需的基础定义。它由运行时预先导入。
- `std/array`：不可变 Array 的读取、组合、映射、过滤、折叠和查找。
- `std/dict`：不可变 Dict 的读取、键值枚举、构造、合并、映射、过滤和折叠。
- `std/option`：Option 的变换、默认值和状态判断。
- `std/result`：Result 的变换、错误映射、默认值、状态判断和解包。

集合函数不修改输入值。可能缺失的读取返回 Option；可能失败的计算返回 Result 或带
blame 的 failure，具体契约可通过 `telora query exports` 查看。

## 文本与格式

- `std/string`：String 的拆分、连接、查找、替换、缩进和解析 property。
- `std/regex`：正则编译、匹配，以及用于类型字符串解析的 `ParseBy` property。
- `std/fmt`：`Display` trait、`Fmt` 值、基础格式项和 `@fmt.display_by` 模板。
- `std/path`：纯字符串的路径连接、规范化、父路径和文件名操作，不访问文件系统。
- `std/hash`：SHA-256 一次性摘要和增量摘要状态。

名义 struct 可以用 Display 模板获得统一的格式与插值能力：

```telora
import "std/fmt" as fmt;

@fmt.display_by("{host}:{port}")
type Endpoint = struct {host: String, port: Int};
```

## 数据边界

- `std/value`：定义递归的 `Value`，以及数据库绑定等边界使用的 `ScalarValue`。
- `std/codec`：在名义类型与 Value 之间编码、解码，并统一消费 codec property。
- `std/json`：JSON 解析、类型化解码、编码、schema 与 JSON codec decorator。
- `std/yaml`：把 YAML 文本解析为 Value。
- `std/toml`：把 TOML 文本解析为 Value。

`Value` 是 source、Entry、EES 和 JSON 共享的数据边界。`ScalarValue` 的 untagged codec
把 `'None`、`'Bool(...)`、`'Int(...)`、`'Float(...)`、`'String(...)` 分别编码为普通
JSON null、boolean、number 和 string。

通常先在格式模块中得到 Value，再用 `codec.decode(Target, value)` 进入业务名义类型；
输出时用 `codec.encode(codec.Value, value)` 回到数据边界。

## 反射

- `std/type-desc`：查询 Type 的 kind、children、field、variant、opaque name 和引用解析。
- `std/type-property`：按 type、field index 或 variant index 查询 property，并取得静态
  `Property(P)` 约束的 evidence。
- `std/dyn`：携带类型身份的动态值、安全投射和基于反射 index 的结构访问。
- `std/eq`：运行时结构相等；静态类型明确的代码使用语言运算符 `==`。

反射中的 member index 来自 `std/type-desc` 的 `FieldDesc` 或 `VariantDesc`。程序应传递
这些已验证的 index，而不是根据布局自行猜测。

## 执行与效果

- `std/entry`：构造 Host 可选择的 `Eval`、`Run(State)` 和 `Serve(State)` 值。
- `std/actor`：定义 reducer 的 `Event`、`Effect`、`Transition` 和 `Service`。
- `std/ees`：声明 Native Effect Service model 并构造请求。

`std/ees` 的公共入口包括通用 model/request 构造器、空配置，以及 SQLite Query 和 IMOS
组件的便捷构造器。应用通过 `std/actor` 发出 EES 请求，不直接执行物理 I/O。具体写法
见 [`EES.md`](EES.md)。

## 工具协议

- `std/argv`：检查、过滤和组合命令参数 Array。
- `std/rt-types/exec`：描述平台、下载解包、环境和可执行入口的纯数据计划。

`std/rt-types/exec` 只定义计划类型，不执行下载、解包或子进程。执行这些计划属于 Host
或 native component 的职责。

## 按任务发现接口

```bash
# 查看模块的全部公开定义
telora query exports std/codec

# 搜索公开名称
telora query exports std/string -p parse

# 查看模块内相关定义
telora query at std/type-property -p variant
```

查询输出是结构化 JSONL，可供人、编辑器和 Agent 使用。模块清单和精确签名无需在应用
文档中复制维护。
