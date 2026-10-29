# 不可变 Fmt 与内部文本 writer 的探索

沿用 explore/collection-aliasing，跟踪 #255。不合入、不部署。

第一轮原型（340a42fe）：

- format::finish_text 统一发布 RT Rust String，JSON 完成时不再创建第二个 Rust String。
- 默认关闭的 TELORA_EXPERIMENTAL_TEXT_WRITER 消除标准标量 Display 插值、
  render(from_string/from_int/from_float(value)) 的临时 Fmt。
- 仅接受 sealed 的标准 native 叶子；未知/自定义 Display 保留原路径，混合插值整段回退。
- 共享 Fmt、相等性操作图、既有接口不变；没有公开 StringMut。

8192 次整数插值约 1.29 s→0.157 s，立即整数 render 约 1.30 s→0.124 s。
但此压力基准关闭自动 GC，RT 资源表从头扫描空槽造成 O(N²) 放大，不能将倍数归于纯文本缓冲。
完整 Fmt 图的 render 负载仍基本不变。JSON 少一次复制暂未测出稳定收益。
IC/NS 真实准备 fuel 仅降约 0.5% / 0.7%，不能据此宣称业务明显加速。

下一层抽象是用户提出的 fmt(self, f: Fmt) -> Fmt：Fmt 作为不可变输出累加器，
只在可证明线性消费、最终 render 的 region 内降低为内部 StringMut。
方法签名本身不证明线性，保留旧版本、分叉、比较、未知调用必须保留不可变实现或复制。
应直接追加标量 token/文本，不再先造 Fmt 片段。旧 concat 的嵌套会触发 128 层上限，
不能用简单 wrapper 代替新的追加表示与迭代遍历。本轮未改变 Display 接口。

完整实验方法、原始报告、真实应用验证及边界：
https://github.com/hh9527/lab-ontology/blob/explore/lazy-knowledge-materialization/benchmarks/text-writer/README.md


## 不可变 Fmt 累加器原型

Display 唯一接口改为 `fmt: Fn(Self, Fmt) -> Fmt`，DisplayBy 的闭包同步改为
`Fn(Dyn, Fmt) -> Fmt`，插值与 codec 的 Display 桥接都已迁移。

```telora
impl fmt::Display for Endpoint {
    fmt: fn(value, f) {
        let f = fmt::write_str(f, value.host);
        let f = fmt::write_str(f, ":");
        fmt::write_int(f, value.port)
    },
};
```

`empty()`、`write_str/write_int/write_float`、`write_fmt`、`encode_json_string` 和
`encode_json_float` 提供不可变输出计划；`to_string(value)` 是从空 Fmt 开始的便利入口。
JSON 字符串包含两端引号与完整转义，和 json::stringify 共用 quoted primitive。
encode_json_float 对非有限数额外防御；语言算术自身已拒绝产生非有限结果。
这是接口迁移，不支持旧的 Display::display/member contract；from_*/concat 保留为片段构造器。

RT 使用追加节点保留旧版本，迭代渲染连续追加；比较前缀使用尾循环。
concat 和 write_fmt 的嵌套仍受 128 层限制；追加次数本身不消耗嵌套层数。
GC 和构建快照的统一 trace 认识所有新节点布局，空 Fmt 没有 operand。
修复原生 JSON Float 检查的逻辑地址读取：先经现有堆地址转换读取整数位，再 reinterpret。
没有将逻辑指针直接传给 f64.load。

实验开关还支持：

- render 直接消费、以 empty 为根的简单 write_str/int/float/encode_json_string 链。
- 包括泛型 to_string 在内，经 sealed evidence 与标准包装器精确转发验证的标量 Display。

这些路径只收集参数并在末端向一个内部 Rust String 写入，保留实参求值顺序、callee
初始化检查和首次失败。共享、比较、未知 Display、任意绑定/fold 的整段 region 尚不融合。
没有公开 StringMut，也没有将签名视为独占证明；codec→Value→JSON 仍未融合。

2048 项、三次新 Session 的中位数（自动 GC 关闭）：

| 路径 | 默认 ms | 融合 ms | fuel 降幅 |
| --- | --- | --- | --- |
| write_int 后 render | 143.87 | 47.27 | 76.92% |
| 三步 write 链后 render | 720.72 | 62.27 | 95.04% |
| 标准整数 to_string | 160.49 | 47.58 | 77.30% |
| 整数插值 | 160.87 | 55.89 | 74.89% |

仍含绕过资源表 O(N²) 搜索的收益，不能解释为纯 StringMut 效果；累计托管分配不覆盖
内部 Rust String 分配，也不等于峰值 RSS。IC/NS 的准备仅小幅改善，API 本身还增加了
一些代码与调用成本，不能宣称业务已明显加速。原始报告与应用对照见 lab 的
benchmarks/text-writer/accumulator-*.json 与 README.md。

验收：core 211 项；默认/融合 Wasm 各 206 项，5 ignored，过滤既有 main 内存增长用例；
默认/融合语言验收各 441 cases。新增覆盖版本复用/分叉、2048 次追加及比较、JSON 转义、
负零、GC、构建快照、原生非有限浮点防御和失败/调试顺序。

## 自定义 formatter 的整段融合（Issue #256）

分支 `perf/display-writer-regions` 默认尝试融合自定义 Display、跨函数 helper、
`display_by` property、插值及 codec 的 Display 桥接。业务接口仍是不可变 Fmt，
不需要启用实验开关或手写可变 builder。

编译器对 sealed world 中同签名的全部可能目标做保守线性证明：累加器必须唯一、
只能转发和追加，不能分叉、比较、捕获或传给未知消费者。通过证明后生成内部 writer
专用函数，共享隐藏 ABI 参数中的 Rust String；普通函数与 callable 身份保持不变。
`array::fold` 的已证明 callback 可以转发同一个 writer。最终一次性生成 String。
同签名中存在不安全目标时会保守回退，因此证明覆盖率仍有扩展空间。

`write_fmt`/concat、观察旧版本的路径仍使用普通 Fmt。codec 桥接本身仍创建一个空
Fmt；这不是 codec→Value→JSON 全链融合。新路径关闭 writer 版本的尾调用降级，
递归 formatter 的进一步优化尚需独立验证。

调用失败时，外层先销毁 writer，再传播 failure；调试和实参求值顺序不变。
强制逐次 GC 验证了插值临时字符串的根，构建期快照也覆盖了 property closure 恢复。
隐藏 writer 不会成为语言值或快照结果。Rust String 分配不包含在托管分配计数中。

`TELORA_DISABLE_FORMAT_REGIONS=1` 仅用于编译对照；默认不设置即可自动受益。
旧 `TELORA_EXPERIMENTAL_TEXT_WRITER` 原型仍保留，两者需要分别归因。
应用对照、制品增长及原始计量见 lab-ontology 的 `benchmarks/display-regions/`。
