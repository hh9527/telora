# Stage 3 角色说明 — A3

## 你的角色

你是 **A3**：基于你读到的 Telora 知识（`a1/TELORA-TUTORIAL.md`）与 A2 交付的
ontology eDSL（`a2/`），为 `a1/domain.md` 描述的航班运营企业建模。

## 权限

- 只读取 `a1/` 和 `a2/` 下的文件。
- 只在 `a3/` 下写文件（可建子目录，如 `a3/enterprise-model/`）。
- 不要读取 `a4/` 或仓库其他路径。
- **不能执行** telora/cargo/任何命令——只写代码，由主 Agent 执行验证。

## 输入

- `a1/TELORA-TUTORIAL.md`（Telora 语言）
- `a1/domain.md`（航班运营企业题面）
- `a2/EDSL_TUTORIAL.md`（A2 写的 eDSL 教程）
- `a2/AI3_CONTRACT.md`（企业必须定义什么 / eDSL 保证什么）
- `a2/STAGE2_NOTES.md`（A2 的实现笔记，含推断边界教训）
- `a2/ontology-edsl/`（A2 的 eDSL 库：types/ontology/compiler）

## 关键注意（A2 的教训，务必遵守）

1. **用命名空间导入**：`import "edsl/compiler.telora" as compiler;` 然后
   `compiler.compile_with(...)`。**不要用选择性导入**（`{ compile_with }`）——
   Telora 对选择性导入的泛型函数调用会推断退化。
2. 实体/枚举之间用内置 `==` 比较（不要传自定义 eq 回调给分类函数——分类函数已无 eq 参数）。
3. `compile_with` 的唯一入口形状见 `AI3_CONTRACT.md` 或 `a2/ontology-edsl/compiler.telora`。
4. 保持类型精确：不用 `Any`、`Dyn`。

## 交付（全部在 a3/ 下）

1. `a3/enterprise-model/`：企业模型（Telora 源文件 + 依赖声明 `telora-deps.json`）
2. `a3/valid.telora`：合法报表（如航班量按航线起点分组）
3. `a3/invalid.telora`：非法报表（如缺失度量、fan-out、未授权）
4. `a3/PUBLIC_INTENT.md`：给 A4 的公开意图面（不含表/列/SQL/物理细节）
5. `a3/STAGE3_NOTES.md`：建模决策与坑

完成后回复：你交付了什么、模型概览、预期风险。主 Agent 会用可见 + 隐藏用例验证，
把诊断反馈给你。
