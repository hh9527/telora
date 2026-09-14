# #191：跨模块声明尚未确定时过早统一使用点

独立的复现 crate，取自 GitCode 提交
`1fa629b815d4894e633177af73f15d25b87d0f30` 的测试，源码独立存储，不嵌入 Rust。
所有用例在语言语义上均应通过；下表记录当前 bug，不作为期望成功的诊断断言。

在仓库根目录运行（已提供本地 workspace lock，无需下载依赖）：

```sh
target/release/telora -C crates/telora/tests/fixtures/issues/191 check --only-types @src/main
target/release/telora -C crates/telora/tests/fixtures/issues/191 check --only-types @src/decorated
target/release/telora -C crates/telora/tests/fixtures/issues/191 check --only-types @src/local
target/release/telora -C crates/telora/tests/fixtures/issues/191 check --only-types @src/empty
```

2026-09-14，main `96232665` 的 release 实测：

| 入口 | 内容 | 实际结果 |
| --- | --- | --- |
| `main` | 跨模块 nominal 的两个字面量，scalar 字段分别为 String.type 和 Int.type，声明均为 Type | 退出 1：`cannot unify String with Int` |
| `decorated` | 跨模块 alias-to-Dict 的 decorator 参数分别为非空记录和空记录 | 退出 1：`cannot unify {en: String, zh: String} with {}` |
| `local` | nominal 声明移到同模块且位于使用之前 | 退出 0 |
| `empty` | 单独使用跨模块 Dict(String) 别名注解空记录 | 退出 0 |

两个失败入口均为一个 type conflict、零 unknown，无解析或 resolve 错误。
`--only-types` 不执行 property 或运行时代码，因此失败发生在静态类型求解阶段。

## 修复后的回归

CLI 测试 `declaration_shapes_are_not_inferred_from_the_first_use` 直接读取这里的源码，
要求 `main`、`decorated`、`local`、`local-late`、`reversed`、`empty`、`batch`
在纯类型检查和包含初始化的检查中均通过。`local-late` 将声明放在使用之后，
`reversed` 调换两个使用点，`batch` 先加载声明模块再加载多个使用模块。
`bad-field` 与 `bad-alias` 是负例，必须仍然拒绝错误字段和无法解析的类型。

根因是 type 声明经过普通值 Fit 队列才链接身份，以及类型构造调用尚未得到结果时，
其结果槽被误当成可从实参推断的自由变量。修复后声明直接链接身份，类型调用显式
登记其结果槽；Fit 等待类型构造结果，不通过丢弃约束或停机放行来掩盖问题。
