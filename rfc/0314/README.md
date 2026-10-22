# 表示原型与阶段验收

布局原型使用未发布的 tooling Wasm，复现命令：

```sh
CARGO_TARGET_DIR=/tmp/telora-rfc0314-target CARGO_BUILD_JOBS=4 \
  cargo run -p telora-wasm --example scalar --release -- \
  /tmp/rfc0314-layout.wasm rfc/0314/layout-probe.telora
python3 rfc/0314/layout-probe.py /tmp/rfc0314-layout.wasm
```

`layout-probe.json` 记录基线布局结果。样例含 Int、String、tuple、嵌套数组；
完整元素宽度为 16 或 24 字节。间接 Single 各省去 16 字节 RawArray 头；
直接内联完整元素需要将 Array 从 24 扩为 32 或 40 字节，每复制一个描述符
分别增加 8 或 16 字节。多个别名会重复支付直接内联成本，间接 Single 共享元素槽。
有限 payload 内联还需为来源另留空间，不能把完整来源头当成免费空间。

因此按 RFC 首版选择保持 24 字节的间接 Single，递归类型继续通过间接边终止。
这个原型测量的是封闭类型宽度，不是动态数组频率，也不包含 allocator 元数据。
实际时间与分配结果见 [results.md](results.md)，动态发布频率另以隔离插桩统计；
不能用静态宽度原型代替应用性能证据。

Raw 内容阶段：长 String/Bytes 在受管堆中有独立 len/cap 头，普通执行回收保持
raw 引用和切片偏移不变；显式边界压缩仍需重定位 Raw 对象。内容统计现在包含
8 字节头且已计入 managed/words，不能再与 words 相加作为独立占用。
阶段检查采用 shared、content、root_lifetimes 和 transform_service 相关用例。
完整验收与配额口径见 [validation.md](validation.md)，三轮原始样本见
[measurements.json](measurements.json)，制品 SHA-256 见 [provenance.json](provenance.json)。
空／单／多命中、字符串密集与 ic-model 均完成三轮对照；已明确记录高命中过滤回退。
