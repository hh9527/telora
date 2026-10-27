# 探索性 JavaScript 后端

从前端的 `SealedExecutable` 直接生成单一 ES module，默认导出所选入口的值。生成代码使用原生 JS 函数和 BigInt，不需要 Wasm、Telora VM 或额外运行时文件。

## 运行

```sh
cargo run -p telora-js --example compile -- input.telora answer output.mjs
node --input-type=module -e 'import answer from "./output.mjs"; console.log(answer(5n));'
cargo test -p telora-js
```

示例输入：

```telora
def offset: Int = 7;
pub def answer: Fn(Int) -> Int = fn(x: Int) {
    let y = x + offset;
    if y > 10 { y * 2 } else { y - 1 }
};
```

测试依赖 PATH 中可用的 Node.js。示例编译器接受单文件及标准库模块。正式 workspace 可通过 `telora -C <workspace> build --backend js <module>:<export> -o output.mjs` 构建指定导出；省略 `:<export>` 时选择 Service 入口。

## 已验证范围

- Int、Float、String 字面量与标量运算、条件分支
- 函数调用、闭包捕获、递归与封闭泛型实例
- Array/Dict/Tuple/结构、枚举构造和模式匹配
- Array/Dict/String 标准库基本操作，集合结果被冻结
- 严格求值的局部绑定、按需记忆化的全局引用
- BigInt 表达 Int，保留有符号 64 位范围检查
- 不支持的可执行节点在生成时返回错误

产物的 Int 参数和结果使用 BigInt。当前错误为 JS 异常，未对齐 Telora 的诊断位置、错误恢复及 quota。未完成语义覆盖前，不作为现有后端的替代品。

## 扩展路线

后端独立于 `telora-wasm`，复用前端的解析、符号解析、类型检查和封闭入口。语义证据来自 MIR；JS 后端不重新进行类型推断。

1. 对齐标量边界、错误与求值顺序，并建立与 Wasm 后端的差分测试
2. 实现 newtype、enum、Option、结构、模式匹配及集合的不可变语义；JS 内部可利用可变 builder，但对外发布后不能共享可变别名
3. 为 native 标准库实现 JS 适配，覆盖类型元数据、property、TypeMemorizer、pipeline 和 JSON 编解码
4. 接入现有 build-time staging，分别表达构建期执行与运行期残余程序；类型身份来自同次编译，信息产物作为同文件内的静态数据导出
5. 接入 Service 与正式构建 CLI，用生成样本、IC、NS 比较构建耗时、文件大小、加载初始化与首次/稳态 transform

运行期 fuel 和内存 quota 不能直接映射到 JS 引擎；需单独设计，不以浏览器或 Node.js 堆限额冒充等价实现。尾递归目前也受 JS 调用栈约束。

当前原型尚不支持构造校验、反射、完整 Service 和 build-time staging，因此没有 IC/NS 的可比性能数据。独立的 property 初始化尚未实现；本原型仅验证所选入口的基础表达式子集。
