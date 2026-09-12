# RFC 0285：SealedMir 到 Cranelift 的机械 codegen

- 状态：实施中；已打通闭合表达式/无捕获函数的首批机器码执行，尚未整图总装
- 日期：2026-09-12
- 上级：[RFC 0282](0282-native-cranelift-roadmap.md)
- 分支：`feat/native-cranelift`
- 跟踪：[#182](https://github.com/hh9527/telora/issues/182)

## 动机与范围

直接生成机器码，不先建设新字节码或 Wasm 后端。

## 用户可见语义与内部契约

本 RFC 不改变语言语法和静态求解规则。新增执行能力仅经隐藏 native 路线选择；默认旧实现不变。

- 首先确定 Cranelift 版本、Cargo feature 和主机 target 支持，依赖应留在独立 native 模块/crate 边界。
- 直接消费 SealedMir、最终 TypeId、符号身份和泛型实例化结果；不得重做 resolve/类型推断或按名字识别内置能力。
- 先跑通常量、标量运算、局部值、分支、函数调用和返回，再补循环/递归、聚合操作、闭包及间接调用等实际 MIR 节点。
- Rust helper 承担对象和原生资源操作。优先固定槽位和统一 ABI，不先做对象访问内联、寄存器分配策略或复杂优化。
- 生成代码和函数地址归 native session 所有，其生命周期覆盖所有闭包和调用；返回状态接入统一诊断。
- 缺失 codegen 规则在执行前明确报 unsupported，并附带位置/类型；不能回退旧 VM。

## 实施计划

### 首批实现与边界

`telora-native` 的可选 `jit` feature 使用稳定版 Cranelift 0.135.0。当前验证平台为 x86_64 Linux（64-bit little-endian）。默认 CLI 未依赖该 crate，也未增加隐藏开关；不存在选择 native 后跳过初始化的临时捷径。

`jit::compile` 消费 SealedMir 中指定的表达式或无捕获、单态 Closure：支持 Int/Float/Unit 常量、参数引用、Bool 条件分支和纯结果 block。读取已闭合 TypeId/符号身份，不使用名字推断；其他表达式、局部绑定和捕获/导出引用明确返回带位置的 unsupported。此接口不等同于模块求值，不跳过顶层副作用声称完成 eval。

真实入口为 C ABI `(context, args_ptr, result_ptr, closure_ptr) -> u32`。第四个指针借用闭包描述符，生成函数通过独立运行时 helper 读取捕获；无捕获的 host 根调用传空。调用前核对参数数量/类型/宽度；仅 Success 解码结果并检查 stamp；Failed 不读结果。返回分支按 word 生成 SSA 合流，保留被选值的来源。所有代码地址留在 Compiled 内，借用期间调用，编译失败或 owner 析构时释放 JIT 内存，地址不对外发布。

验证：`cargo test -p telora-native --features jit` 通过 6 项测试，包含真实机器码的常量/Unit/浮点和三参数条件选择、错误参数拒绝、unsupported 不丢语句，以及失败返回不读取未写结果。当前没有 runtime helper 对象访问、程序内部函数调用、整图初始化或 CLI；后续按本 RFC 继续补齐，不关闭 #182。

先落实本模块契约并保证可独立编译，再用简单单测或少量语言用例验证，然后进入后继模块。允许 native 路线阶段性缺失能力，不要求每次提交完成整个语言。实现前将本草案中的待定项补成明确决议，不引入兼容兜底。

### 对象 helper 接入

后续已接入独立 runtime 的对象 helper：机器码可以构造 String、Array、Tuple/Record、有序 Dict，并按已求解布局读取字段/数组元素。对象描述经固定栈缓冲区传递，引用对象不经过旧 Val 或深复制桥。CallContext 持有 native Runtime，参数验证所属 session，结果继承该 session 身份；helper 的失败携带来源并返回 Failed，生成代码直接传播，不能读取失败结果。

`cargo test -p telora-native --features jit` 当前通过 11 项测试，包括真实机器码构造四类对象、读取已发布 main 数组及有来源的越界失败。尚未实现的 construction check/字段 property、捕获/导出引用和普通语句仍明确拒绝；程序内部函数调用、整图初始化和 CLI 后续推进。

### 函数与标量运算进展

后续已支持普通 let/def、直接函数调用和递归。按稳定 HIR 身份注册函数，声明后排队生成函数体，递归引用不会重复编译；调用使用独立参数/结果栈区域及同一四指针 ABI。无用绑定仍执行初始化并传播失败，不通过删除语句制造成功。

词法闭包开始支持按稳定 SymbolId 排序的捕获计划；参数和函数内部声明不计入外部捕获。运行时环境独立存入 Vec 槽位，其缓冲区包含完整捕获值；环境 0 表示无捕获，其余编码为环境表 HeapRef + 1。初始化发布会复制嵌套闭包与捕获对象，并保持别名；FunctionId 保留为代码计划身份，不保存机器码地址。

间接调用消费 callee 的封闭 Function TypeId，生成该签名下的 FunctionId 分派；所有可达函数完成注册后再生成分派体，未知或签名不符的 ID 产生一次 Failed。已验证函数参数、返回带捕获闭包、条件选择函数，以及显式实例化泛型函数作为高阶参数。Runtime 首次执行时绑定 Compiled 代码计划身份，拒绝混用其他编译结果；代码内存仍由 Compiled 独占。整图多入口计划与初始化调度尚待接入，这里不是完整 CLI 生命周期的完成声明。

标量 Int/Float 运算与比较直接生成 Cranelift 指令；整数溢出、除零以及非有限浮点结果走有来源的失败路径，逻辑运算短路。当前 `--features jit` 共 13 项测试通过，包含 .telora 递归阶乘资产。Float remainder、非标量操作、泛型实例、捕获/间接调用等仍继续推进；未完成项不走旧 VM。

### 闭合泛型实例进展

函数注册键扩展为 `(HirId, GenericInstanceId?)`，类型直接读取对应实例的归一化节点表；表项缺失即报错，不退回模板类型或执行替换求解。实例内调用直接读取 MIR 的 reference 边，显式 TypeApply 沿 callee 边消费已存在的实例记录，不重新匹配签名。隐式、显式和嵌套泛型调用资产已通过，当前总计 14 项测试。运行时没有模板参数推导。

## 验收条件

Native 声明现在生成同一调用 ABI 的适配函数，使用已登记 native 模块 ID 与声明 ABI 导出键链接，并检查封闭签名；导入别名和一等泛型实例不改变身份。首批为 array.length / string.length，测试包含真实标准库声明、Unicode 字符数、间接泛型参数和拒绝用户模块同名 native 声明。函数值初始化与适配器调用使用不同编译键，初始化仅生成描述符，不能以空参数调用 native 本体。其余 native 操作和 property 查询仍待补齐。

类型元数据使用封闭 TypeId 作为单 word 数据，`TypeOf(T)` 的见证在构造/发布时核对；比较直接比较 represented TypeId。函数参数和返回边界的 `TypeOf(T) -> Type` 适配只更新外层类型标记，保留 represented TypeId 与来源，不在运行时求解类型。该能力已在普通参数、隐式返回、显式 return 和发布路径验证，property 执行仍需单独接入。

以独立入口运行小型已 seal MIR/源码用例，检查标量、分支、函数、递归、聚合访问及错误路径。建立表达式支持清单；后续以 .telora 用例补齐规则。首个原型无需完整标准库可运行。

## 延后与备选方案

不采用 Wasm/Wasmtime、多层编译链或新字节码解释器作为本阶段前置。不提前替换默认运行时。性能优化、AOT 分发、跨平台覆盖与生产切换按证据另立后续 RFC；本子项完成不等于新路线全量验收。
