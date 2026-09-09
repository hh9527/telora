# Telora 测试最佳实践

本文面向 Telora 程序作者和库作者，说明如何把语言契约写成可独立执行、可定位失败的
测试。语言语义见 [语言设计](../docs/design/LANGUAGE.md)，命令和 JSONL 协议见
[CLI 指南](TELORA-CLI.md)，测试模块的路径与可见性见 [Workspace 指南](WORKSPACE.md#test-root)。

## 先区分检查与行为测试

`telora check MODULE` 解析、检查并初始化模块，适合发现语法、类型、导入和初始化
问题。`telora test NAME` 在这些步骤成功后，执行 `tests/NAME.telora` 直接导出的
`std/test.Test`。`check` 成功不代表行为断言通过。

| 要验证的契约 | 合适的验证方式 |
| --- | --- |
| 普通计算的结果、转换、边界条件 | `test.should_ok` 内显式断言 |
| 函数返回 `Err` 或 `None` | 在 thunk 内匹配并检查返回值 |
| 可恢复的执行失败及其消息 | `test.should_fail` / `test.should_fail_with` |
| 语法、类型、导入或模块初始化失败 | 独立运行 `check`，检查退出码与诊断 |
| 导出签名和语义查询结果 | 运行 `query`，检查对应输出 |
| Entry、reducer、外部效果和 Host 协议 | 对应的 `eval-with`、`run`、`serve` 集成验证 |

`test` 不驱动应用 reducer 或 EES。资源耗尽、取消等终止错误不能当作预期失败通过；
测试初始化错误也不能由尚未执行的 `should_fail` 捕获。

## 把计算放进 thunk，显式断言结果

下面是完整的 `tests/arithmetic.telora`：

```telora
import "std/test" as test;

def twice: Fn(Int) -> Int = fn(value) { value * 2 };

export def doubles_positive = test.should_ok(fn() {
    let actual = twice(3);
    if actual == 6 { True } else { fail!("unexpected doubled value", actual) }
});

export def doubles_zero = test.should_ok(fn() {
    let actual = twice(0);
    if actual == 0 { True } else { fail!("zero must stay zero", actual) }
});
```

`should_ok` 的含义是“正常返回”，不是“返回 True”，也不是“返回 Ok”。正常返回
`False`、`Err(...)` 或 `None` 都会通过。需要验证结果时，必须匹配结果或让不满足的
条件执行 `fail!`。

不要写顶层 `def actual = twice(3);`，再在 Test 中读取它来代替测试计算。
顶层值在模块初始化时计算；失败会阻止整个入口执行。可复用类型、decorator、纯输入
常量和函数可以放在顶层，被测调用与断言放进零参数 thunk。计算复杂的输入准备也宜
封装成函数，在需要它的用例中调用。

每个可独立诊断的契约导出一个 Test，使用表达行为的名称。同一契约内可以有多个
相关断言，但不要把整个套件串成一个大 thunk。可恢复失败后，其他用例仍会执行。
不要让用例依赖执行顺序或另一用例的成功结果。

## 区分返回错误与抛出错误

下面是完整的 `tests/results.telora`：

```telora
import "std/test" as test;

def positive: Fn(Int) -> Result(Int, String) = fn(value) {
    if value > 0 { Ok(value) } else { Err("expected positive") }
};

export def accepts_positive = test.should_ok(fn() {
    let actual = positive(3).unwrap!();
    if actual == 3 { True } else { fail!("wrong payload", actual) }
});

export def returns_rejection = test.should_ok(fn() {
    match positive(0) {
        Err(message) => if message == "expected positive" { True }
            else { fail!("wrong rejection", message) },
        Ok(value) => fail!("zero was accepted", value),
    }
});

export def raises_rejection = test.should_fail_with(fn() {
    positive(0).unwrap!()
}, "expected positive");
```

返回 `Err` 不产生失败诊断；`unwrap!` 在 Ok 时取出 payload，在 Err 时调用
`raise!`。`should_fail_with` 检查可恢复失败的主消息是否包含给定的非空、区分大小写
子串，不检查完整文本相等，也不自动验证来源位置。只关心是否拒绝时可用
`should_fail`，关心拒绝原因时选择稳定且有区分度的消息片段。

预期失败应尽量只包含被测操作，避免前面的输入准备意外失败也让测试通过。为拒绝
用例保留相邻的成功对照。若库同时公开 Boolean 探测器与真正的转换入口，分别测试：
探测器返回 False 不能证明转换入口一定拒绝，也不能证明拒绝理由正确。

不要为了方便测试而把所有领域函数改成 Result。只有调用者确实需要恢复或分支时才
选择 Result；直接承诺成功类型并在非法输入上失败，同样是可测试的契约。

## 保留诊断来源，不把 warning 当作断言

`raise!` / `warn!` 接受的错误有明确边界：

| 错误类型 | message | 数据引用 | rule |
| --- | --- | --- | --- |
| String | 字符串内容 | 空，不自动附加字符串来源 | 报告处的实际宏调用位置 |
| BlameError | 错误提供 | 错误携带的显式 subjects | 报告处的实际宏调用位置 |

`blame!` 构造错误数据，不报告；`raise!` 归一化错误、添加 rule 并产生失败，返回
Never。`fail!(message, subjects...)` 仍是受支持的写法，等价于在该位置执行
`raise!(blame!(message, subjects...))`。断言失败时，把能解释问题的实际值作为 subject
传入；不要把已有 BlameError 改成字符串而丢掉数据引用。

`.unwrap!()` 的报告位置属于用户的宏调用处，调用参数和 Result 容器不会被隐式
加入数据引用。若 `.unwrap!()` 写在 helper 内，rule 就在 helper 内，不能假设它指向
最外层调用者。`ParseError` 等其他错误类型需显式适配，不能仅因有 message 就直接解包。

`.ok_or_warn!()` 在 Ok 时返回 Some，在 Err 时交给 `warn!` 并返回 None。warning
不会使 `should_ok` 失败，不应用它吞掉成功用例中的错误。需要断言 warning 的数量、
级别或 rule/数据引用时，应检查带诊断的公开观测接口或 Host 的结构化输出，单靠
`should_fail_with` 不足以验证这些内容。

旧的函数专用 `should_ok!` / `must_ok!`、`try_unwrap!` 和 `std/result.unwrap` 已删除。
普通测试构造器 `test.should_ok` 仍然存在，与函数结果解包不是同一职责。

## 类型约束与 codec 边界分别测试

类型声明 `T` 与元数据 `T.type` 不可混用。codec 接收明确的类型见证，例如
`codec.decode(T.type, input)`。对于带 `@check` 的类型，要分别验证合法构造、非法
构造和解码失败：普通构造拒绝产生执行失败，codec 拒绝返回 `Err(BlameError)`。

下面是完整的 `tests/checked.telora`：

```telora
import "std/test" as test;
import "std/blame" { BlameError };
import "std/codec" as codec;
import "std/value" { Value };

def check_positive: Fn(Int) -> Result((), BlameError) = fn(value) {
    if value > 0 { Ok(()) } else { Err(blame!("expected positive", value)) }
};

@check(check_positive)
type Positive = struct(Int);

export def constructs = test.should_ok(fn() {
    let value = Positive(2);
    if value.0 == 2 { True } else { fail!("wrong positive value", value) }
});

export def rejects_construction = test.should_fail_with(fn() {
    Positive(0)
}, "expected positive");

export def rejects_decode = test.should_ok(fn() {
    match codec.decode(Positive.type, Value.Int(0)) {
        Err(_) => True,
        Ok(value) => fail!("decoder accepted zero", value),
    }
});
```

具名字段 struct 的 `@check` 参数是 `Unchecked(T)`，newtype 和带载荷 variant 的
参数是载荷类型；返回 `Ok(())` 接受，`Err(BlameError)` 拒绝。根据真实契约覆盖边界值、
嵌套构造和更新，不要只重复测试校验 helper 而绕过类型构造入口。

编码解码测试除 roundtrip 外，还应检查外部表示和非法输入。两个方向同时出错也可能
通过 roundtrip；预期输出必须独立于被测转换计算。跨模块名义类型和 property 的
契约应通过真实 import 验证，而不是在测试里复制同形声明。

## 用 fixtures 承载重复输入和来源

同一契约需要一组外部数据时使用 `with_fixtures`。例如在 `tests/fixtures/one.json`
和 `tests/fixtures/two.json` 中分别保存 `1` 与 `2`，然后创建 `tests/fixtures.telora`：

```telora
import "std/test" as test;
import "std/codec" as codec;
import "std/value" { Value };

export def positive_integers = test.with_fixtures([
    "fixtures/one.json", "fixtures/two.json",
], fn(input) {
    test.should_ok(fn() {
        let value = codec.decode(Int.type, input).unwrap!();
        if value > 0 { True } else { fail!("expected positive fixture", input) }
    })
});
```

fixture 由 Host 准备，factory 接收保留来源的 Value 并返回子 Test。解码、转换和
断言放在子 thunk 中；factory 自身失败不能由子 `should_fail` 捕获。Host 在执行本组
第一个 factory 前准备全部直接输入，缺失或格式错误的文件属于 fixture 准备失败。

路径相对实际构造 Test 的模块，不能越过声明 crate；导入或重导出不改变该基准。
支持 JSON、YAML、TOML，不支持网络、stdin 或通配符。嵌套组可以表达多维输入组合，
但应控制规模，避免无意义的组合膨胀。小型内联输入无需一律移到文件。

fixture 不产生 import 边。验证静态数据模块的加载、可见性或模块身份时，仍应使用
真实 import，而不是用 fixture 替代它。

## 组织、执行与验收

在已配置并锁定的 workspace 中运行：

```bash
telora check @test/arithmetic
telora test arithmetic
telora test results
telora test checked
telora test fixtures
```

`NAME` 是 `tests/` 下不带后缀的入口路径，支持子目录；当前每次显式选择一个入口。
Test 必须由入口直接公开导出，放在 Array、Dict 或 Dyn 中不会被发现。普通 import
不会执行依赖的 Test；显式重导出会按公开别名执行，同一 Test 的两个别名计为两次。

查看退出码与 `telora.test/v2` summary，确认有用例执行、没有失败且没有中止。
`total == passed + failed`，成功的 fixture 组本身不额外计数。不要把准备阶段报错、
零用例执行或只有一部分结果输出误判为通过。失败时先区分 discovery、fixture、
factory、execution 阶段，再定位对应 export 和 fixture 索引。

测量性能时固定 binary、输入和命令，分别记录 `check` 与 `test`。端到端时间包含
加载、类型检查和初始化，不能用总时间除以用例数推断单次断言成本，也不能把提前
失败的耗时与成功执行完整套件比较。
