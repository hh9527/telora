# Property 支撑的自动 trait 探索

分支 `explore/property-auto-traits` 上的语言用法实验，沿 RFC 0307 和 Issue #221 的
required/optional evidence 能力探索建模表达。使用现有的显式 blanket impl 模拟自动获得的
trait 能力，尚未定义或实现新的 `AutoTrait` 语法，也未限制语言中的开放 Property 查询。

## 使用画面

建模者声明类型及属性：

```telora
@model::dataset("devices", "device_rows")
@model::description("Managed devices")
type Device = struct { id: String, name: String };
```

库通过一处 blanket impl 将属性连接到行为能力：

```telora
impl(T: Property(DatasetSpec) + ?Property(Description)) Dataset for T {
    describe: fn() {
        let spec = props::required@[DatasetSpec, T]();
        // 消费已封闭的可选 Description evidence，并构造 DatasetInfo
        ...
    },
};
```

上面为省略函数体的结构示意，完整可执行实现在 `model/src/schema.telora`。
消费函数只依赖 Dataset，调用点通过 `T.type` 推导类型参数：

```telora
def describe: for(T: Dataset) Fn(TypeOf(T)) -> DatasetInfo = fn(target) {
    Dataset(T).describe()
};

def datasets: Array(model::DatasetInfo) = [
    model::describe(Device.type),
    model::describe(Alarm.type),
    model::describe(Manual.type),
];
```

Device 和 Alarm 自动满足 blanket impl，Manual 使用手写 exact impl。三者产生普通的统一描述，
集合消费端无需重新查询 Property，也无需给每种类型再定义 wrapper 类型。

## 运行

使用包含 RFC 0307 的 Telora 编译器。驱动复制模型到用户临时目录，为每个案例生成本地 lock，
通过真实 CLI 的 MIR 封闭及 Wasm 执行路径验收。实验不依赖 ontology，也不读取生产模型。

```sh
python3 tests/probes/property-auto-traits/run.py --telora /absolute/path/to/telora
```

临时目录遵循 `TMPDIR`，未设置时使用 `~/.cache/tmp/`。输出目录保存每个案例的 stdout、stderr
以及 `report.json`；驱动不修改源样本或已有运行环境。

## 已验证

- 必需 DatasetSpec 通过 type-site Property bound 提供能力
- Description 的 Present/Absent 均由同一 blanket impl 消费
- 给 Alarm 添加可选 Description，仅改变描述值，其他模型描述保持一致
- 手写 exact impl 无需 DatasetSpec，也能进入同一集合
- 不同类型的 DatasetInfo 可统一收集和消费
- `T.type` 保留类型推导用途，属性读取由 trait 实现中的 bound 授权

## 已复现的边界

| 案例 | 当前结果 | 含义 |
|---|---|---|
| 未标注且无手写 impl 的类型 | 静态拒绝，缺少 Dataset evidence | 依赖通过能力签名表达 |
| 擦除为普通 Type 后调用 describe | 静态拒绝，缺少具体 TypeOf witness | 擦除前需要形成描述或封装能力 |
| 字段上标注 Column，要求 owner 满足 Property(Column) | 静态拒绝 | type-site bound 不表达 field-site 属性 |
| 声明一个无消费者且必然失败的 Property provider | 执行正常入口时仍失败 | 当前 seal_export 仍保留会话中的全部 concrete Property |

最后一个案例记录的是现有全量初始化行为，驱动将它标为 `observed_retention_blocker`。
驱动成功不表示已完成属性裁剪；没有测量或声称 wasm 体积优化收益。

## 后续设计问题

类型级属性到统一描述的路径可以由现有语法表达。要将 Property 的使用完整收口到自动 trait，
还需明确以下能力：

- 字段和 variant 属性怎样形成所属类型的聚合能力及精确依赖，避免退回开放查询
- 自动 trait 的声明方式、生成哪些方法，以及与用户 exact impl 的关系
- 属性存在性 evidence 和 provider 值求值之间的依赖如何进入执行闭包
- 如何区分显式完整模块检查的根与普通执行入口的属性根

`DatasetInfo` 是已求出的普通数据。实验没有提供“对擦除后的 Type 随时恢复任意 trait”的能力，
也没有引入 World、trait object 或运行期按 TypeId 探测 Property 的协议。
