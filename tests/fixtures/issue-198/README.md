# #198 的小型复现

在 main `fe007a27` 对应的 release 实现上复现，保留为修复后的回归用例和对照。

从仓库根目录运行：

```sh
cargo build --release
target/release/telora -C tests/fixtures/issue-198 lock
target/release/telora -C tests/fixtures/issue-198 check --only-types @src/boundary
target/release/telora -C tests/fixtures/issue-198 check @src/boundary
```

`boundary.telora` 只有三行。修复前纯类型检查成功，普通 check 在 Wasm codegen 阶段报告
`Wasm: sealed boundary adaptation missing: TypeId(...) -> TypeId(...) at HirId(...)`。
TypeId 数字依赖图内分配，不作为复现条件。`accept` 不会调用传入的 `stop`，因此预期
初始化成功并产生整数 1；原错误不是执行 `fail!` 的结果。

实际边界是 `Fn(Int) -> Never` 到 `Fn(Int) -> Int` 的函数参数。
原 `adapt()` 只处理实际值本身为 Never 的情况，未处理此例中函数返回类型的 Never。
发射调用实参时由 `functions.rs::call()` 调用 `adapt()`，不能仅根据报错字符串
断定失败来自 `enums.rs::constructor()` 或 newtype 隐式转换。

修复前将上述两个 check 命令的模块换为 `@src/enum-payload`，也能重现相同阶段的报错。
该例把同一个函数作为枚举载荷传入，仍不需要 newtype、泛型、外部语料或数据源。

修复后两个模块的纯类型检查和普通 check 均应成功。MIR 在函数值使用处记录目标签名
与适配证据，seal 验证证据完整；Wasm 消费既有适配，不重新推断，也不修改 `stop`
声明的返回类型。语言测试另覆盖泛型实例、局部绑定、数组，以及调用后仍执行 `fail!`。

`capture-control.telora` 是未触发第二个错误的正向对照：导入枚举成员构造器，闭包中引用
并调用它，再对结果匹配。以下命令成功输出 `1`：

```sh
target/release/telora -C tests/fixtures/issue-198 eval @src/capture-control:answer
```

此外尝试了顶层/局部构造器值别名、嵌套闭包、泛型工厂、map、高阶参数、泛型枚举、
重排类型实参和成员导入模式等组合，尚未复现 #198 的 `unavailable capture`。
这些结果不能证明第二个报告已修复，也不能证明本复现就是原始语料中的同一根因。
