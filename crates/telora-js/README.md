# 探索性 JavaScript 后端

前端的 `SealedExecutable` 直接生成单一 ES module，默认导出所选入口，Service 模型提供 `createService()`。模型制品包含 JS 运行时和封闭类型元数据，不包含 Wasm，也不依赖额外的模型运行时文件。

## 构建

```sh
cargo run --release -p telora -- -C <workspace> build --backend js <module> -o model.mjs
cargo run --release -p telora -- -C <workspace> build --backend js <module>:<export> -o value.mjs
```

单文件实验也可以使用 `cargo run -p telora-js --example compile -- input.telora answer output.mjs`。
当前验收环境为 Node.js 26.12.1。

## Host 边界

应用 host 使用通用的 `host/process.mjs`，模型只在独立 Node 子进程中加载，通过 JSONL IPC 通信。
`createService()` 是该子进程中的执行接口，不作为应用进程直接加载模型的建议方式。

```js
import { ModelProcess } from './crates/telora-js/host/process.mjs';

const model = new ModelProcess({
  model: '/path/to/model.mjs',
  memoryMiB: 512,
  initializationTimeoutMs: 15000,
  requestTimeoutMs: 5000,
});
try {
  const response = await model.request('ic/transform', { intents: [intent] });
} finally {
  model.close();
}
```

默认通过 Linux cgroup v2 与用户 systemd scope 限制总内存，关闭交换，并限制进程/线程数量。
Node 的 V8 堆参数辅助控制 GC；硬上限由 `MemoryMax` 提供，包含堆外分配。
缺少可用的 systemd/cgroup 环境时启动失败，生产路径不静默降级。
`unconfined-test` 仅供没有 systemd 的基础测试显式选择。

初始化与请求分别由 host 的墙钟计时器约束。超时、OOM、协议错误或响应超额后终止该代进程；下一次请求重新初始化，失败请求不自动重放。
同一模型进程按顺序处理请求，默认请求不超过 1 MiB、响应不超过 16 MiB。
JS 的时间预算不等价于 Wasm 的确定性 fuel。

## 语义与模型覆盖

- Int 使用 BigInt，保留有符号 64 位范围检查
- 函数、闭包、递归、封闭泛型实例、条件与模式匹配
- 结构、元组、newtype、枚举、Option/Result 与不可变集合
- 类型反射、property 链、TypeMemorizer、pipeline 的私有 builder/memo 状态
- JSON/typed codec、regex 命名捕获、Display、URI 与纯 JS SHA-256
- 构造/解码校验、诊断捕获、Service transform 路由
- IC 与 NS 真实模型编译、受限 IPC 调用及 SQLite 查询验收

后端复用解析、符号解析、类型检查和封闭入口证据，不重新推断类型。
暂未实现的可执行节点/native 在生成时明确报错。

## 当前边界

构建只完成前端与 JS 代码生成。模型的 A 阶段准备和信息生成仍在子进程初始化时执行；尚未将构建期计算结果序列化为 B 阶段静态数据，`--snapshot/--staged/--source` 选项不适用于该后端。
因此 JS 构建时间和已执行 A 阶段的 Wasm staged 构建时间不代表相同工作量。

诊断具有静态源码位置，输入值的来源位置与完整 blame 传播尚未对齐。
回归对照验证接受/拒绝、SQL、bindings、列描述、警告消息和 severity；位置列表的差异单独记录。
标准库尚未全量覆盖，尾递归受 JS 调用栈限制。
通用进程 host 为本分支原型，尚未接入 `telora-run` 或 dsh-oks 的正式运行路径。

## 验证

```sh
cargo test -p telora-js
cargo test -p telora --test cli backend_surface
node --test crates/telora-js/host/process.test.mjs
TELORA_JS_HARD_LIMIT_TESTS=1 node --test crates/telora-js/host/process.test.mjs
```

硬限制测试实际验证 cgroup 上限、堆外分配触发终止、死循环/初始化超时、响应配额与进程重建。

`scripts/js-model-smoke.mjs` 接受 `{domain,wasm,db,cases}` JSON；每个 case 为 `{id,input,expectedAccepted?}`。
它逐项对照 Wasm，并在只读 SQLite 中执行成功查询。`--golden` 保存 Wasm 响应，`--reference` 可回放已保存的基线。
`scripts/js-model-benchmark.mjs` 使用同一请求重复测量两边 IPC 往返、JS 执行、冷启动和进程 RSS。
测量记录见 [EVALUATION.md](EVALUATION.md)。
