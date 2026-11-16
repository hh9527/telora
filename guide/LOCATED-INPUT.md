# 带 offset 来源的结构化输入

宿主已经完成自身语言的解析时，可以直接提供业务值及其来源范围。Telora 不再解析业务 JSON 文本，导入后使用现有 Value、codec、检查及诊断机制。

## Rust 接口

`telora_wasm::located_input` 和 `telora_run::located_input` 导出相同的结构定义。

```rust
use telora_wasm::located_input::{InputValue, LocatedValue, Span};
let input = LocatedValue {
    span: Some(Span { start: 0, end: 7 }),
    value: InputValue::String("cpu_usage".into()),
};
let value = session.import_located_value(&input)?;
let packet = input.encode()?;
let reply = runner.request_located(&packet)?;
```

`Session::import_located_value` 返回与解析接口相同的 transport::Value，使用相同的根保持规则。`TransformSession::transform_located` 接收编码后的 packet，和 transform 一样由调用方按既有生命周期 reset；Runner::request_located 自动执行请求重置。

对象通过 `InputValue::Object(Vec<Field>)` 表达，Field 包含 name、键的可选 span，以及 LocatedValue。对象键顺序任意，导入时按现有 Dict 规则排序；重复键被拒绝。数据类型包含 Null、Bool、Int64、Float64、String、Array、Object。

## 来源

span 是宿主原始输入中的 UTF-8 字节半开区间，不是结构包中的位置。所有宿主来源均为临时 source=0，显示名 `<input>`。不传入源码、文件名、行索引或 CRLF 信息；宿主负责验证自己的字符边界，并在收到诊断后恢复文件和编辑器坐标。

值与对象键的范围分别提供，生成值可以共享来源。None 不附加来源；`[0, 0)` 按现有约定等同无来源。`[0, n)`（n > 0）可用。start <= end，end 必须小于 2^25。来源不成为业务字段，也不改变普通值相等性。

诊断保留模型规则位置和数据 subject 位置。`<input>` 的 start/end.line 为 0，offset 是原始字节偏移；模型源码位置继续使用原有行列。来源不自动把无 subject 的 fail! 或内部 trap 变成用户代码错误。

## 命令与服务

```sh
telora-run app.wasm --located-input < request.tli
telora-run app.wasm --serve stdio+located://
telora run @src/main --serve stdio+located://
```

单次 --located-input 的 stdin 是一个 TLI1 packet；成功 stdout 和其他单次命令一样输出业务 JSON，诊断写 stderr。该开关与 --serve 互斥。

`stdio+located://` 是常驻二进制帧协议：每个请求为 u32 little-endian 长度 + TLI1 packet；每个响应为 u32 little-endian 长度 + UTF-8 的 telora.service/v1 响应 JSON。长度不含前缀本身。完整帧中的非法结构包返回错误响应，之后仍可处理新请求；截断帧、超长帧等传输错误终止连接。普通 JSONL 和 HTTP 输入不隐式识别结构包。

Guest 显式入口为 `run-service-located(input, length, output, capacity, result)`，缓冲区所有权和 run-service 一致。语言 handler 仍接收 Value。`telora_import_located` 是供物化器调用的内部入口；公开集成不应依赖它返回的内部节点包布局。

## TLI1 字节编码

所有整数 little-endian，包头为四字节 ASCII `TLI1`，后接一个递归值。包尾不得有多余字节。

每个值先编码 u8 kind，然后编码来源 start:u32、end:u32。0/0 表示无来源。kind 与后续 payload 如下：

| kind | 值 | payload |
| --- | --- | --- |
| 0 | Null | 无 |
| 1 | False | 无 |
| 2 | True | 无 |
| 3 | Int | i64 |
| 4 | Float | IEEE 754 binary64，必须有限 |
| 5 | String | u32 UTF-8 字节数，再接字符串字节 |
| 6 | Array | u32 元素数，再接各递归值 |
| 7 | Object | u32 字段数，再接各字段 |

每个对象字段编码：name 的 u32 UTF-8 字节数 + name 字节 + 键来源 start:u32、end:u32 + 递归字段值。Int64 保持精确，不经 JS Number 中转。空对象、空数组和空字符串均允许。

输入上限为 64 MiB、100000 个值节点、最大深度 128（根为 0）。传输层另有自身长度上限；Guest 内存和 fuel 继续限制实际解码与物化。非法版本、UTF-8、范围、长度、重复键、非有限浮点数或超限输入均在发布值前拒绝。

## 验证

```sh
cargo test -p telora-wasm-shared located_input
cargo test -p telora-wasm located_input
cargo test -p telora-run --lib
cargo test -p telora --test cli standalone_runner_accepts_located
```
