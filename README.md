# filetree233

面向命令行用户和 Agent 的磁盘空间分析与清理工具。当前是开发中的早期版本，尚不能替代 WizTree。

## 已实现

- 磁盘总览：列出已发现的卷及总容量、可用空间，也可查询任意指定路径的文件系统容量。
- 普通遍历支持并行构建完整目录快照，显示逻辑大小和文件分配量；支持全树搜索、CSV / JSONL 导出、重复文件检查、清理建议与清理计划。Windows NTFS MFT 实验引擎的结果标记为不完整。
- CLI：`scan`、`tree`、`duplicates`、`search`、`recommend`、`plan`、`apply`；MCP stdio 也提供这些操作。
- 清理必须先生成计划；执行前展示目录、大小和数量，人工输入确认码后再次检查计划与目录，再按计划移入回收站或永久删除；默认回收站；计划展示逻辑大小和去重后的文件分配量。MCP 客户端不支持 elicitation 时拒绝执行。
- 扫描跳过符号链接及 Windows reparse point，保留其他可见文件大小并标记不完整；创建计划时遇到不可读取项或链接则拒绝。

## 构建和运行

安装 Rust stable 和平台原生编译工具后：

```sh
cargo build --release --locked
filetree233                     # 显示命令帮助
filetree233 volumes             # 已发现的卷
filetree233 capacity /path      # 指定路径的文件系统容量
filetree233 scan /path/to/dir
filetree233 scan /path/to/dir --json --threads 4
filetree233 tree /path/to/dir --threads 4 > tree.jsonl
filetree233 tree /path/to/dir --format csv --output /path/outside/tree.csv
filetree233 tree C:\Data --engine mft > mft-tree.jsonl  # Windows NTFS, experimental
filetree233 duplicates /path/to/dir --threads 4
filetree233 recommend /path/to/dir --threads 4
filetree233 search /path/to/dir "*.log" --glob --kind file --limit 100
filetree233 search /path/to/dir "**/cache/*.tmp" --glob --path-match --min-bytes 1024
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

使用 `tree` 导出 CSV 或 JSONL，导出路径须位于扫描目录外且不能覆盖已有文件。使用 `plan` 生成计划，核对路径、大小和处置模式后运行 `apply`，人工输入显示的确认码才能执行。计划文件不能放在待删目录内。

离线 `mft-inspect` 按 JSON Lines 流式输出已导出的逻辑 MFT 记录、无效槽及最终统计；记录大小必须由来源确认，不能自行猜测。该接口会输出文件名等元数据，包含用于保留原始名称的 UTF-16 码元。`mft-volume` 可只读访问 NTFS 挂载卷的 MFT，Windows 通常要求管理员权限；这是原始记录检查接口，不提供目录汇总或清理计划。`--fast` 使用批量读取，可能跳过损坏记录或遗漏扩展属性，无法统计跳过的数量，结果始终标记不完整；默认模式提供逐条错误信息。离线解析不能证明记录完整，也不能替代实时卷扫描或作为删除依据。

性能测量方法和一次小目录实测见 [docs/performance.md](docs/performance.md)。

## 一键安装

六个目标：Windows、Linux、macOS 的 x64 和 ARM64。安装脚本从指定 GitHub Release 下载可执行文件和 `SHA256SUMS`，校验后安装；默认版本是 `v0.1.0-alpha.1`。该已发布的旧预览版仍包含 GUI，尚不包含本页面描述的全部新功能；当前纯命令行版本需要从源码构建，直到新版本发布。

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/neko233-com/filetree233/v0.1.0-alpha.1/scripts/install.ps1 | iex
```

Linux / macOS：

```sh
curl -fsSL https://raw.githubusercontent.com/neko233-com/filetree233/v0.1.0-alpha.1/scripts/install.sh | sh
```

安装器也支持通过 `FILETREE233_RELEASE_BASE` 指定 HTTPS Release 下载镜像（路径应对应 GitHub 的 `releases/download`）。校验文件与程序来自同一下载源，使用镜像时应确认该源可信。安装前校验 SHA-256，并在目标目录暂存后替换已有程序。

发布构建会在六个平台对当前产物执行本地 HTTP 安装、重复安装及错误校验测试；标签发布后还会自动运行六平台的 GitHub Release 实际下载与安装验证。

Windows 默认安装至 `%LOCALAPPDATA%\Programs\filetree233`；Linux/macOS 默认安装至 `~/.local/bin`，可通过 `FILETREE233_INSTALL_DIR` 修改目录，或通过 `FILETREE233_VERSION` 选择版本。Windows 脚本参数 `-InstallDir`、`-Version` 可用于本地脚本。发布工作流并行构建六种平台；标签触发发布，版本及限制记录在 [CHANGELOG.md](CHANGELOG.md)。macOS 构建未签名公证。具体平台的实际可用性以发布和验收结果为准。

## MCP

```json
{"mcpServers":{"filetree233":{"command":"C:\\path\\to\\filetree233.exe","args":["serve"]}}}
```

`volumes()` 返回已发现的卷，`capacity(path)` 查询指定路径的文件系统容量；`tree(path, output, threads?, engine?, format?)` 将完整树写入扫描目录之外的新文件，`format` 为 `jsonl`（默认）或 `csv`，禁止覆盖。`search(path, query, glob?, path_match?, kind?, min_bytes?, limit?, threads?, engine?)` 搜索整棵树并返回最大的匹配项；`recommend(path, threads?, engine?)` 输出需要人工审核的缓存/构建目录建议、大小与风险，不自动选择或删除；`benchmark(path, threads?, iterations?, engine?)` 测量独立扫描进程的时间和峰值内存；`duplicates(path, threads?)` 返回重复文件及硬链接别名，不执行删除；`scan(path, threads?, engine?)` 返回子项大小、数量及扫描错误；`engine` 可选 `walk`、`mft`、`auto`，默认 `walk`；`plan(paths, output, mode?)` 生成计划而不删除，`mode` 为 `recycle`（默认）或 `permanent`；`apply(plan)` 通过 MCP 客户端展示计划并要求用户输入确认码，确认后重新校验再执行。客户端和本地系统必须可信：恶意客户端可以伪造 elicitation 答复。`scan`、`search`、`recommend` 和 `duplicates` 作为后台只读任务运行，支持 MCP `notifications/cancelled`；每次仅允许一个后台任务，运行期间其他长操作返回忙碌错误，取消后需等待该任务退出才能重试。`tree`、`benchmark`、`plan` 和 `apply` 仍同步执行；系统调用或网络文件系统阻塞时取消并非即时。MCP 的路径参数可以是普通字符串，也可以是 `{"encoding":"utf16","units":[...]}`（Windows）或 `{"encoding":"bytes","units":[...]}`（Unix）；可使用扫描返回的 `path_units`，不要用有损显示文本替代原始路径。

## 限制与安全边界

- **逻辑大小与文件分配量不等于可回收空间。** 扫描和计划按文件标识对硬链接去重，重复硬链接的分配量归属于扫描中首先发现的路径（并行扫描时归属可能改变，总量不变）；仅统计文件数据分配量，不含目录、NTFS 额外数据流、文件系统元数据等，也不保证删除后会释放同等空间。压缩、稀疏、网络或特殊文件系统的结果依赖操作系统提供的值。错误会使扫描标记为不完整，生成清理计划时无法读取分配量则拒绝。MFT 查询特殊文件分配量失败时，汇总返回 `allocation_unknown` 和各项 `allocation_errors`，树中对应文件节点返回 `complete: false` 和错误原因；零值不代表确定的分配量。实验性的 Windows NTFS MFT 目录汇总使用两次全卷遍历，支持 `--threads` 并行读取，按记录顺序合并并将每轮缓存限制为每个线程一个最多 1,024 条记录的分块（`bench` 的 `scan_threads` 显示扫描线程数），保留文件名原始 UTF-16 码元，但仍可能遗漏损坏记录或扩展属性，结果始终标记不完整；底层 MFT 库暂不支持名称无法转成 Unicode 的卷挂载点，此时应使用普通遍历；`auto` 对非卷根目录直接使用普通遍历，卷根目录在无法访问 MFT 时回退普通遍历。卷容量通过系统接口单独查询，与扫描发现的文件分配量不是同一指标；不应与 WizTree 的速度或准确性等同。
- 卷列表是查询时的系统快照。Linux 自动列表不包含网络和 tmpfs 挂载，可通过 `capacity PATH` 显式查询；网络文件系统查询可能受网络状态影响。Windows 的总量和可用量可能受用户配额影响。
- 文件系统遍历可设置 1～32 个工作线程（默认 4），按目录并行，单个目录中的子目录也会分配给其他工作线程；机械硬盘或网络盘上增加线程可能变慢。完整树模式将节点保存在内存中，内存随文件数量增长。清理建议使用目录名和项目标记进行启发式匹配，可能遗漏或误报；最多返回逻辑大小最大的 10,000 条（大小相同时按路径排序），并抑制已建议目录内的嵌套建议。
- 重复文件查找按大小、分段哈希、完整 SHA-256 和逐字节比较筛选，硬链接作为同一文件的别名；各阶段核对文件身份和系统状态变更时间，最终逐字节比较时再次核对完整哈希；读取失败、身份变化或扫描不完整会标记结果不完整。文件系统时间戳精度有限，扫描结束后文件仍可能变化。搜索结果不会自动选择或删除文件。文件在检查后仍可能发生变化，共享存储或稀疏文件的实际可回收空间也可能不同。
- CSV 按节点流式输出父子编号、路径、大小和完整性；`path_units` / `name_units` 保留原始路径码元。以 `=`, `+`, `-`, `@` 等可能触发电子表格公式的文本字段会加单引号，`spreadsheet_escaped` 标记该行；需要精确原始名称时请使用码元字段。导出失败可能留下部分文件。
- 全树搜索默认按名称做不区分大小写的子串匹配；`--glob` 使用通配符，`--path-match` 匹配完整路径，最多返回 10,000 条且按逻辑大小排序。非 Unicode 名称的子串匹配可能有损，结果保留原始路径码元；搜索不会自动选择或删除。
- 清理建议是待检查的线索，不代表目录可安全删除。建议包括 Rust `target`、Node `node_modules` / `.next`、Python 缓存、Gradle 缓存以及 Unity 项目的 `Library` / `Temp`；清理可能触发重新下载、重新构建或丢失自定义文件。大小是扫描快照，不等于可回收空间；生成计划时会重新统计并检查。
- `apply` 按计划中的模式移动到系统回收站或永久删除，可能部分成功；回收站通常不会立即释放空间，恢复受系统及回收站设置影响；Windows 网络路径不支持回收站模式。失败不会由程序自动改为永久删除。不要把计划文件交给不可信用户修改。计划拒绝磁盘根目录、当前工作目录及其祖先、重叠目录和包含计划文件的目录。
- 新计划格式为版本 5，显式记录处置模式，并对原始文件名、文件身份、状态变更时间、类型、长度和修改时间建立指纹；版本 4 仍可使用，版本 2、3 须重新生成。无法转换为 Unicode 的路径在计划中以原始字节或 UTF-16 码元保存，并在确认时显示完整编码；系统回收站可能拒绝某些此类路径。扫描结果提供 `path_units` / `name_units`，显示文本可能包含替代字符。该指纹不读取文件内容，不能保证发现所有内容变化；校验到删除之间存在竞态。请关闭相关程序，避免扫描与不受信任进程共同写入的目录，并备份重要数据。
- `bench` 不清理系统文件缓存，峰值内存包含整个进程及树；每次运行的 `diagnostics` 汇总不完整节点、跳过的链接、分配量错误及其他直接错误节点（不包含路径）；请分别测试冷缓存、热缓存及真实大盘，不能仅凭小目录结果推断全盘表现。尚未进行真实大盘性能和新版本发布后的各平台安装端到端测试。
