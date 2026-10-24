# RFC 0317：Service 初始化元数据发布

- 状态：Draft，按用户授权实施并在工作分支验收
- 跟踪：[Service 构建 #248](https://github.com/hh9527/telora/issues/248)

## 动机与语义

Service 初始化可计算一份供 Host 读取的静态元数据。其生成应与准备计算共用同次初始化，结果不必被 handler 捕获，也不应因此成为运行期 snapshot 根。

标准库提供 publish_metadata(name, Value)，在 Service 初始化期间接收已序列化的 JSON。名称非空且唯一；初始化之外调用、重复名称都失败。普通运行模式也允许初始化收集元数据，Host 不读取时不改变业务响应。元数据不包含语言闭包、TypeId 或任意 Guest 地址。

## 实现

RT 在 Service 准备开始时打开收集窗口，create 完成后关闭。初始化原有全局求值及 Service init 均位于窗口内。收集器拥有 UTF-8 字节，不含 GC 管理的语言值引用；一次导出后释放缓冲。Host 在 seal 前取得结果，以 ServiceBuild 生命周期与同次初始化绑定。snapshot 不序列化收集器，恢复后的实例保持发布窗口关闭。

最终 Wasm 可有唯一的 telora.service.metadata 自定义段，UTF-8 JSON 为 {version:1,entries:{name:value}}。名称只处于 entries 内，不能覆盖标准 Wasm 段。仅 snapshot 构建发布元数据；无条目则不附加段。初始化、序列化、元数据解析、B 验证或资源配额失败均不覆盖旧输出。

语言及 RT 只认识 Service、元数据和 JSON，不认识任何领域知识图。首版没有跨构建缓存、可执行元数据或独立用户回灌通道。

## 备选与验收

避免借 debug／诊断承载制品协议，避免为导出元数据持久保留业务路由或 handler 根。元数据读取不要求启动实例。

验收覆盖同次初始化结果与 transform 一致、多个命名条目、重复及迟到发布拒绝、失败保留旧制品、full／runtime 一致、自定义段可直接读取，且元数据生成代码不必保留在 B 中。通用编译器回归及实际使用方验收通过后提交推送，保持未合入。
