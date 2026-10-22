# RFC 0313 对照实验

`prepare-ic-model.py` 将 lab-ontology 复制到外部目录，生成三个版本及差异补丁：

- baseline：原始源码。
- seq：仅将 `length(filter(...))` 和 `map(filter(...))` 改为显式 Seq。
- application：在 seq 基础上，存在性判断读取轻量类型身份/映射，summaries 批量构建。

application 变体只用于合法固定模型的对照；尚未作为应用迁移交付或承诺所有非法
模型的诊断等价。原项目不会被修改。脚本拒绝源和输出树包含彼此，已有变体目录
也不会覆盖。

```sh
python3 rfc/0313/prepare-ic-model.py --source /path/to/lab-ontology --output /tmp/seq-ic
CARGO_TARGET_DIR=/tmp/seq-probe-target cargo build --release --manifest-path rfc/0313/initialize-probe/Cargo.toml
# 对 baseline、seq、application 各自执行，使用同一个新编译器：
telora -C /tmp/seq-ic/baseline/icloud_model build icloud_model -o /tmp/seq-ic/baseline.wasm
/tmp/seq-probe-target/release/telora-seq-initialize-probe /tmp/seq-ic/baseline.wasm 64 32 64 250000
```

probe 参数依次是 artifact、Guest MiB、words MiB、回收间隔 KiB、fuel 百万单位。
记录 create-service 的时间、fuel、线性内存大小、GC 次数与累计 managed 分配。
线性内存只增长，初始化结束时大小可作此轮 Guest 峰值；它不等于 Host RSS。
probe 的初始化 trap 是终止结果，随后重设 fuel 仅为读取诊断指标，不恢复用户计算。

完成初始化的制品可另用 `build --snapshot --initialization-fuel 250000` 发布，
再通过 telora-run 验证实际请求。不要在测量初始化耗时期间并行运行重型构建/测试。

三个制品的输出对照可用：

```sh
python3 rfc/0313/check-ic-model.py --runner /path/to/telora-run /tmp/seq-ic/baseline.wasm /tmp/seq-ic/seq.wasm /tmp/seq-ic/application.wasm
```

## 2026-10-22 测量

源项目 commit `ad2465bb`，实现 commit `475718c3`；同一 release 编译器、
64 MiB Guest、32 MiB words、64 KiB GC 下限、2500 亿初始化 fuel。
三个初始化顺序运行，未与重型构建/测试并行；各一次，时间只作观察。
原始输出在 `/tmp/rfc0313-ic/`，机器可读记录及 artifact SHA 见 results.json。

| 版本 | 初始化秒 | fuel（十亿） | 累计分配 MiB | Guest 峰值 MiB | GC 次数 |
| --- | ---: | ---: | ---: | ---: | ---: |
| baseline | 68.66 | 144.36 | 145.38 | 32.25 | 214 |
| seq | 66.68 | 137.89 | 130.90 | 24.44 | 193 |
| application | 67.31 | 137.90 | 130.90 | 24.19 | 193 |

迁移合计替换 43 处 length/filter 和 32 处 map/filter，含请求路径，不能将全部
替换数量当作初始化调用次数。三个版本的 8 个实际请求输出完全一致。
Seq 版本累计分配下降约 9.96%，fuel 下降约 4.49%；单次耗时差异不足以作为
稳定加速承诺。额外轻量身份/批量 summaries 实验没有可见增益。

这证明该能力可减少实际模型的一部分中间分配，同时证明这一批迁移不足以
大幅降低初始化成本。大部分分配仍存在；重复扫描、profile 验证中构造临时
查询对象、分配器/根发布/地址转换成本仍需分别调查和设计。
标量微测的固定 888 B 分配不能外推为整个模型零中间对象。

微测（16/256/1024 项）中 Seq map/filter + both(count,fold) 执行分配均为
888 B，eager 对照为 768/8448/33024 B；输入装载不计入。小输入的固定构造
开销可大于 eager，首版未承诺消除 plan 构造和所有间接回调。

完整微测覆盖单消费者、组合消费者与分别求值；输入大小均为 16/256/1024：

| 路径 | managed 分配 B | 保留 scratch 容量 B |
| --- | --- | --- |
| Seq count | 480 / 480 / 480 | 336 / 336 / 336 |
| Seq both(count, fold) | 888 / 888 / 888 | 304 / 304 / 304 |
| Seq 分别 count、fold | 696 / 696 / 696 | 672 / 672 / 672 |
| eager length、fold | 768 / 8448 / 33024 | 336 / 336 / 336 |

scratch 指 `telora_frame_bytes` 在调用后的保留 backing 容量，包含转移缓冲和
frame 元数据，属于 scratch 高水位的容量上界，不是瞬时 live 字节。
微测回调仅作标量运算；表内包含 plan 和结果构造，未将各构造节点独立计费。
组合消费者的固定 plan 开销可能高于分别消费，价值在于共享遍历和公共回调，
不保证对所有小输入降低总分配。
