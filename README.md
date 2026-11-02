# filetree233

面向桌面用户和 Agent 的磁盘空间分析器。当前是开发中的早期版本，尚不能替代 WizTree。

## 已实现

- 原生桌面窗口：输入路径、扫描下一级条目、按文件逻辑大小排序，显示逻辑大小及文件分配量、筛选、进入子目录、选择文件或目录并生成清理计划；顶部色条按已显示条目的逻辑大小展示比例。
- CLI：`scan`、`plan`、`apply`；MCP stdio：`scan`、`plan`、`apply`。
- 清理必须先生成计划；执行前展示目录、大小和数量，人工输入确认码后再次检查计划与目录，再永久删除；计划展示逻辑大小和去重后的文件分配量。MCP 客户端不支持 elicitation 时拒绝执行。
- 扫描跳过符号链接及 Windows reparse point，保留其他可见文件大小并标记不完整；创建计划时遇到不可读取项或链接则拒绝。

## 构建和运行

安装 Rust stable 和平台原生编译工具后：

```sh
cargo build --release --locked
filetree233 gui                 # 不带参数也打开 GUI
filetree233 scan /path/to/dir
filetree233 scan /path/to/dir --json --threads 4
filetree233 plan /path/to/cache -o /path/to/plan.json
filetree233 apply /path/to/plan.json
filetree233 serve               # MCP stdio
```

GUI 中输入扫描路径，勾选文件或目录，填写不存在的计划文件路径，再创建计划。人工核对后输入界面显示的确认码，才能永久删除。计划文件不能放在待删目录内。

## 一键安装

六个目标：Windows、Linux、macOS 的 x64 和 ARM64。安装脚本从指定 GitHub Release 下载可执行文件和 `SHA256SUMS`，校验后安装；默认版本是 `v0.1.0-alpha.1`。

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/neko233-com/filetree233/v0.1.0-alpha.1/scripts/install.ps1 | iex
```

Linux / macOS：

```sh
curl -fsSL https://raw.githubusercontent.com/neko233-com/filetree233/v0.1.0-alpha.1/scripts/install.sh | sh
```

Windows 默认安装至 `%LOCALAPPDATA%\Programs\filetree233`；Linux/macOS 默认安装至 `~/.local/bin`，可通过 `FILETREE233_INSTALL_DIR` 修改目录，或通过 `FILETREE233_VERSION` 选择版本。Windows 脚本参数 `-InstallDir`、`-Version` 可用于本地脚本。发布工作流并行构建六种平台；标签触发发布，版本及限制记录在 [CHANGELOG.md](CHANGELOG.md)。Linux 需要系统图形和 OpenGL 运行环境；macOS 构建未签名公证。具体平台的实际可用性以发布和验收结果为准。

## MCP

```json
{"mcpServers":{"filetree233":{"command":"C:\\path\\to\\filetree233.exe","args":["serve"]}}}
```

`scan(path)` 返回子项大小、数量及扫描错误；`plan(paths, output)` 生成计划而不删除；`apply(plan)` 通过 MCP 客户端展示计划并要求用户输入确认码，确认后重新校验再执行。客户端和本地系统必须可信：恶意客户端可以伪造 elicitation 答复。

## 限制与安全边界

- **逻辑大小与文件分配量不等于可回收空间。** 扫描和计划按文件标识对硬链接去重，重复硬链接的分配量归属于扫描中首先发现的路径（并行扫描时归属可能改变，总量不变）；仅统计文件数据分配量，不含目录、NTFS 额外数据流、文件系统元数据等，也不保证删除后会释放同等空间。压缩、稀疏、网络或特殊文件系统的结果依赖操作系统提供的值。错误会使扫描标记为不完整，生成清理计划时无法读取分配量则拒绝。当前没有卷总量统计或 NTFS MFT 加速；不应与 WizTree 的速度或准确性等同。
- 文件系统遍历可设置 1～32 个工作线程（默认 4），按目录并行，单个目录中的子目录也会分配给其他工作线程；机械硬盘或网络盘上增加线程可能变慢。当前只展示一级条目和比例色条，不是完整文件树或真正的文件级 treemap；无实时扫描进度/取消、回收站、重复文件分析或规则推荐。
- `apply` 是永久删除，可能部分成功；不要把计划文件交给不可信用户修改。计划拒绝磁盘根目录、当前工作目录及其祖先、重叠目录和包含计划文件的目录。
- 计划格式为版本 2；旧版计划须重新生成。指纹基于路径、类型、长度和修改时间，无法检测所有同大小同时间内容变化；校验到删除之间存在竞态。请关闭相关程序，避免扫描与不受信任进程共同写入的目录，并备份重要数据。
- 尚未进行真实大盘性能、峰值内存、跨平台 GUI 和各平台安装端到端测试。
