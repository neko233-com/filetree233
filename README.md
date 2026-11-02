# filetree233

面向桌面用户和 Agent 的磁盘空间分析器。当前是开发中的早期版本，尚不能替代 WizTree。

## 已实现

- 原生桌面窗口：普通遍历支持并行构建完整目录快照、展开树、导航和分层 treemap；显示逻辑大小及文件分配量，查找内容相同的文件、查看有风险说明的清理建议，选择文件或目录并生成清理计划。Windows NTFS MFT 实验引擎也可生成完整树，结果标记为不完整。
- CLI：`scan`、`tree`、`duplicates`、`recommend`、`plan`、`apply`；MCP stdio 也提供这些操作。
- 清理必须先生成计划；执行前展示目录、大小和数量，人工输入确认码后再次检查计划与目录，再按计划移入回收站或永久删除；默认回收站；计划展示逻辑大小和去重后的文件分配量。MCP 客户端不支持 elicitation 时拒绝执行。
- 扫描跳过符号链接及 Windows reparse point，保留其他可见文件大小并标记不完整；创建计划时遇到不可读取项或链接则拒绝。

## 构建和运行

安装 Rust stable 和平台原生编译工具后：

```sh
cargo build --release --locked
filetree233 gui                 # 不带参数也打开 GUI
filetree233 scan /path/to/dir
filetree233 scan /path/to/dir --json --threads 4
filetree233 tree /path/to/dir --threads 4 > tree.jsonl
filetree233 tree C:\Data --engine mft > mft-tree.jsonl  # Windows NTFS, experimental
filetree233 duplicates /path/to/dir --threads 4
filetree233 recommend /path/to/dir --threads 4
filetree233 bench /path/to/dir --threads 4 --iterations 3  # 独立进程测量时间与峰值内存
filetree233 scan C:\Data --engine mft --json  # Windows NTFS, experimental
filetree233 scan C:\Data --engine auto --json # MFT or traversal fallback
filetree233 plan /path/to/cache -o /path/to/plan.json  # 默认回收站
filetree233 plan /path/to/cache -o /path/to/plan.json --mode permanent
filetree233 apply /path/to/plan.json
filetree233 mft-inspect /path/to/exported.MFT --record-size 1024 --used-only
filetree233 mft-volume /path/to/ntfs/mount --name demo
filetree233 mft-volume /path/to/ntfs/mount --name demo --fast
filetree233 serve               # MCP stdio
```

GUI 中输入扫描路径，`Walk` 的 Full tree 模式支持展开目录、点击 treemap 和取消扫描；取消 Full tree 后使用一级汇总。勾选文件或目录，填写不存在的计划文件路径，再创建计划。选择回收站或永久删除，人工核对后输入界面显示的确认码，才能执行。计划文件不能放在待删目录内。

离线 `mft-inspect` 按 JSON Lines 流式输出已导出的逻辑 MFT 记录、无效槽及最终统计；记录大小必须由来源确认，不能自行猜测。该接口会输出文件名等元数据，包含用于保留原始名称的 UTF-16 码元。`mft-volume` 可只读访问 NTFS 挂载卷的 MFT，Windows 通常要求管理员权限；这是原始记录检查接口，不提供目录汇总或清理计划。`--fast` 使用批量读取，可能跳过损坏记录或遗漏扩展属性，无法统计跳过的数量，结果始终标记不完整；默认模式提供逐条错误信息。离线解析不能证明记录完整，也不能替代实时卷扫描或作为删除依据。

性能测量方法和一次小目录实测见 [docs/performance.md](docs/performance.md)。

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

`tree(path, output, threads?, engine?)` 将完整树写入扫描目录之外的新 JSONL 文件，禁止覆盖。`recommend(path, threads?, engine?)` 输出需要人工审核的缓存/构建目录建议、大小与风险，不自动选择或删除；`benchmark(path, threads?, iterations?, engine?)` 测量独立扫描进程的时间和峰值内存；`duplicates(path, threads?)` 返回重复文件及硬链接别名，不执行删除；`scan(path, threads?, engine?)` 返回子项大小、数量及扫描错误；`engine` 可选 `walk`、`mft`、`auto`，默认 `walk`；`plan(paths, output, mode?)` 生成计划而不删除，`mode` 为 `recycle`（默认）或 `permanent`；`apply(plan)` 通过 MCP 客户端展示计划并要求用户输入确认码，确认后重新校验再执行。客户端和本地系统必须可信：恶意客户端可以伪造 elicitation 答复。

## 限制与安全边界

- **逻辑大小与文件分配量不等于可回收空间。** 扫描和计划按文件标识对硬链接去重，重复硬链接的分配量归属于扫描中首先发现的路径（并行扫描时归属可能改变，总量不变）；仅统计文件数据分配量，不含目录、NTFS 额外数据流、文件系统元数据等，也不保证删除后会释放同等空间。压缩、稀疏、网络或特殊文件系统的结果依赖操作系统提供的值。错误会使扫描标记为不完整，生成清理计划时无法读取分配量则拒绝。实验性的 Windows NTFS MFT 目录汇总采用两次全卷遍历，可能遗漏损坏记录、扩展属性或无法解码的名称，结果始终标记不完整；`auto` 对非卷根目录直接使用普通遍历，卷根目录在无法访问 MFT 时回退普通遍历。当前没有卷总量统计，不应与 WizTree 的速度或准确性等同。
- 文件系统遍历可设置 1～32 个工作线程（默认 4），按目录并行，单个目录中的子目录也会分配给其他工作线程；机械硬盘或网络盘上增加线程可能变慢。完整树模式将节点保存在内存中，内存随文件数量增长；treemap 最多绘制 2048 个区域，多余项目会汇总为 Other。完整树支持进度和取消；MFT 完整树模式显示已读取的记录数并支持取消；MFT 一级汇总模式尚不支持实时进度或取消。清理建议使用目录名和项目标记进行启发式匹配，可能遗漏或误报；最多返回 10,000 条，并抑制已建议目录内的嵌套建议。
- 重复文件查找按大小、分段哈希、完整 SHA-256 和逐字节比较筛选，硬链接作为同一文件的别名；读取失败或扫描不完整会标记结果不完整。搜索结果不会自动选择或删除文件。文件在检查后仍可能发生变化，共享存储或稀疏文件的实际可回收空间也可能不同。
- 清理建议是待检查的线索，不代表目录可安全删除。建议包括 Rust `target`、Node `node_modules` / `.next`、Python 缓存、Gradle 缓存以及 Unity 项目的 `Library` / `Temp`；清理可能触发重新下载、重新构建或丢失自定义文件。大小是扫描快照，不等于可回收空间；生成计划时会重新统计并检查。
- `apply` 按计划中的模式移动到系统回收站或永久删除，可能部分成功；回收站通常不会立即释放空间，恢复受系统及回收站设置影响；Windows 网络路径不支持回收站模式。失败不会由程序自动改为永久删除。不要把计划文件交给不可信用户修改。计划拒绝磁盘根目录、当前工作目录及其祖先、重叠目录和包含计划文件的目录。
- 新计划格式为版本 4，显式记录处置模式，并对原始文件名、文件身份、状态变更时间、类型、长度和修改时间建立指纹；版本 2、3 的计划必须重新生成。计划路径本身必须能表示为 JSON 字符串。该指纹不读取文件内容，不能保证发现所有内容变化；校验到删除之间存在竞态。请关闭相关程序，避免扫描与不受信任进程共同写入的目录，并备份重要数据。
- `bench` 不清理系统文件缓存，峰值内存包含整个进程及树；请分别测试冷缓存、热缓存及真实大盘，不能仅凭小目录结果推断全盘表现。尚未进行真实大盘性能、跨平台 GUI 和各平台安装端到端测试。
