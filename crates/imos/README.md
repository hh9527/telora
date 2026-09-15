# imos 文件系统边界

`plan.name` 是请求文件名，长度为 1–64 字节，仅允许小写 ASCII 字母、数字和
分隔符 `-`、`_`、`.`；分隔符不能出现在首尾或连续出现。保留 `.json` 后缀能力，
拒绝 Windows 设备名（con/prn/aux/nul/com1–9/lpt1–9，包括带扩展名的形式）。
不对非法名称自动转换大小写或替换字符。item.name 仍是允许 UTF-8 的描述名称。

`src/fsx.rs` 集中 store 所需的文件系统契约，后端为 `fsx/unix.rs` 与
`fsx/windows.rs`。普通读写、遍历与文件内容同步仍使用标准库；不引入虚拟
文件系统或动态派发。

- `FileSnapshot` 提供文件身份、链接数、长度、修改标记与文件种类。身份包含卷与
  文件 ID，完整身份用于比较；请求键只在已确认同卷的 store 中使用。
  Unix 请求键仍是十进制 inode，已有 store 无需迁移。身份可能在最后一个链接
  删除后被系统复用；长度和修改时间相同也不证明内容绝对未变。
- `hard_link` 必须建立同一文件的引用，失败不能降级为复制。注册、数据库更新、
  stale 判断与 GC 的顺序属于 store 协议，不放入 fsx。
- `try_lock` 将共享/独占锁竞争统一为 `Ok(false)`，其他错误继续报告；重试和进度
  观察由上层负责。底层复用 fs2 的平台实现及竞争错误码。
- `replace_request` 发布完整请求文件，`publish_directory` 发布完整临时制品目录。
  既有目标处理由持有对象锁的 store 负责；接口不承诺替换任意非空目录。
- 文件内容 `sync_all` 与 `sync_directory` 分开。可见性与断电持久性是不同保证，
  目录发布接口本身不承诺持久化。
- `AccessPolicy` 表达私有目录、普通目录、普通文件、可执行文件、防误写保护。
  防误写不是安全边界，也不能代替私有目录的访问限制。

Unix 权限和身份 API 仅存在于 Unix 后端。归档的执行位仍作为制品输入信息，
由 artifact 转换成访问策略；不把整套 POSIX 权限位作为跨平台契约。
可能阻塞的文件系统调用在既有异步流程中通过 blocking 任务执行。

Windows 后端必须处理：句柄文件身份和链接计数、目录句柄、ACL 与只读属性
的区别、打开文件的删除/共享权限、请求替换及制品发布语义、目录项持久化
能力。缺失的访问控制能力必须落实或显式报错，不能静默视为成功；目录项
持久化缺口允许作为显式平台取舍接受，但必须写入后端说明，不能只靠
no-op 静默带过。

`request_lock_key` 也属于平台边界。Unix 保留规范化父目录加文件名的原有哈希，
Windows 需要针对大小写和路径别名确定一致的锁范围；不能仅将路径转为字节就
声称解决了别名问题。plan.name 的字符集限制与调用方先规范化父目录，共同把
别名面收窄到前缀与大小写两类。未来修改锁键需要考虑与旧版本进程同时使用
同一 store。

运行契约测试：`cargo test -p imos -p telora-ees`。测试覆盖硬链接身份与引用回收、
只读请求替换后旧链接保持有效、锁竞争、完整目录发布，以及 store 的安装和回收。

## Windows 后端

Windows 后端为 `fsx/windows.rs`，处理方式如下。

- 文件身份来自 `GetFileInformationByHandle`：volume serial 对应 `st_dev`、
  file index 对应 `st_ino`、`nNumberOfLinks` 对应 `st_nlink`。
- FAT/exFAT 的 file index 恒为 0，后端对此显式报错：store 要求本地 NTFS。
- `PrivateDirectory` 写入受保护、可继承的属主 DACL：属主、SYSTEM 与
  Administrators 全权，对应 Unix 0700。store 根目录接受任意路径，
  `TELORA_EES_STORE` 还能覆盖位置，不能依赖 profile 默认 ACL；普通
  目录、文件与可执行策略沿用从私有根继承的默认 ACL。
- `WriteProtected` 与 `protect_file` 为显式平台取舍的 no-op：只读属性会
  挡住请求文件替换（tempfile persist）与陈旧对象回收；防误写本身不是
  安全边界，由 store 锁与身份检查承担。
- `sync_directory` 为显式平台取舍的 no-op：接受 Windows 上的崩溃持久性
  缺口（目录 fsync 需要 `FILE_FLAG_BACKUP_SEMANTICS` 句柄加
  `FlushFileBuffers`，未在此实现），依据上方契约段允许显式取舍。
- `request_lock_key` 的输入前提是规范化父目录加受限 plan.name：在此
  前提下剥掉 `\\?\`/`\\?\UNC\` verbatim 前缀并折叠大小写，覆盖残余的
  前缀与大小写别名，不宣称归一化任意 Windows 路径。
