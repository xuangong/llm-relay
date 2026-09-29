# LLM Relay v0.5.1

此补丁修复 Windows 上 WSL 卡死导致 Use / setup 一直等待或失败的问题。本次发布 Windows x64 安装包。

## 修复

- WSL 枚举、文件读写和探测命令统一使用 5 秒超时，覆盖 stdin 写入和 stdout/stderr 读取，避免卡住 endpoint 切换锁。
- 超时后冷却 30 秒，避免每个配置文件重复等待；后台继续重试，WSL 恢复后补同步。
- 首次 setup 遇到不可用的 WSL 时，Windows 仍可完成配置，失败的 WSL 保留为待同步。
- WSL 探测失败时保留此前的客户端安装记录，避免误判为客户端已卸载。
- WSL 探测异常不再中断 Windows 的官方登录检测；未完成首次配置的 WSL 可安全取消 setup。

## 下载

- [Windows x64 EXE 安装包](https://github.com/xuangong/llm-relay/releases/download/v0.5.1/LLM.Relay_0.5.1_x64-setup.exe)
- [Windows x64 MSI 安装包](https://github.com/xuangong/llm-relay/releases/download/v0.5.1/LLM.Relay_0.5.1_x64_en-US.msi)
- SHA256SUMS.txt 提供安装包校验值。

macOS 和 Linux 用户可继续使用此前发布的安装包；此补丁未重新打包这些平台。

## 验证

自动化测试覆盖 WSL 进程挂起、stdin 阻塞、大量输出、Windows 与 WSL 配置隔离，以及恢复后重试。未在真实卡死的 WSL 环境中执行安装验证。

Windows 本地验证：workspace 测试 195 项通过、8 项跳过；版本一致性及前端类型检查通过。EXE/MSI 安装包已生成并提供 SHA-256；MSI ProductVersion 已核对为 0.5.1。前端生产资源与 Rust 后端均已重新构建。
