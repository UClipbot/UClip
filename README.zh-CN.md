# UClip

[English](README.md) | 简体中文

UClip 是一款本地优先的剪贴板管理器，帮你保存复制过的内容，在需要时快速搜索、整理和再次粘贴。历史数据保存在本机，可选择开启加密同步。

## 功能

- **剪贴板历史**：保存复制过的文字、图片及文件或文件夹引用，重启后仍可找回。
- **同步**：通过局域网、WebDAV 或 SFTP 双向合并设备上的历史。
- **快速搜索**：按内容、类型搜索历史，支持正则搜索。
- **复制与回粘**：将历史内容重新复制到剪贴板，Linux X11 支持粘贴回原应用。
- **采集控制**：随时暂停或恢复采集，设置排除规则。各分类与未分类历史分别计数，每个标签页默认最多保留 30,000 条本地记录。
- **手工保存**：直接新建文本，保存常用片段。
- **置顶与去重**：保留常用内容，在置顶页整理重复记录。
- **分类整理**：创建、重命名分类，将记录移动或复制到不同分类。
- **回收站**：恢复已删除的记录、永久删除选中项或清空回收站。
- **悬停预览**：在历史条目上悬停 1.5 秒，即可在独立、可调整大小的浮窗中查看文本、图片或文件引用，以及最近采集或保存时间。浮窗打开时保留当前焦点，点击浮窗可继续阅读，按 Esc 关闭。
- **自定义快捷键**：调整搜索、复制、预览、删除与导航的按键组合。
- **桌面常驻**：通过托盘或快捷键唤起窗口，窗口始终置顶，固定后失焦不隐藏，并支持登录启动。
- **界面语言**：支持简体中文和 English，默认跟随系统；可在“设置 → 外观 → 语言”切换，重启后保留。
- **窗口外观**：支持深浅主题，桌面平台共用圆角窗口风格。macOS 和 Linux 的主窗口可切换为系统标题栏。

## 使用

1. 启动应用后会自动监听剪贴板；可在“设置 → 采集”中暂停或恢复，重启后保留所选状态。
2. 在其他应用正常复制，之后复制的内容会出现在历史列表中。
3. 在目标应用中使用主窗口快捷键唤起 UClip。Linux X11 下默认是 Ctrl+Shift+V。
4. 选中一条记录后按 Enter，粘贴到原应用。自动回粘不可用时，会复制到剪贴板供手动粘贴。

Windows 和 Linux 用 Ctrl+C 复制所选记录，macOS 用 ⌘C。右键点击记录可查看完整内容、整理或删除记录。

桌面端采用单实例运行。再次启动时会唤回已有窗口。`npm run tauri:dev` 启动的开发版可与已安装的版本同时运行。Linux 的单实例检测需要会话 D-Bus。

文件记录保存的是引用，使用时需要原文件仍然存在。

搜索和筛选默认显示，可在「设置 → 外观」中关闭。

在 **设置 → 快捷键** 的「全局快捷键」分组里配置主窗口和快捷面板的快捷键，在同页的「窗口内快捷键」分组里调整搜索、回粘和导航等按键。打开快捷面板后，按 **1–5** 选择近期条目。使用主窗口快捷键可展开面板。面板快捷键以字母等非数字键结尾时，也可松开后再按一次末键展开。在「设置 → 桌面」可开启「在快捷面板中显示置顶条目」。

## 同步历史

1. 在一台设备的 **设置 → 同步** 中创建同步组，显示配对码。在其他设备上填写同一个配对码并加入。同步组默认共享已有和之后新增的全部历史，包括分类、置顶和回收站。
2. 设备在同一个 IPv4 局域网内自动发现彼此。先建立的主机协调合并，主机退出后由仍在线的设备接任。若系统防火墙询问，允许 UClip 访问专用网络。
3. 若设备经常错峰在线，在两台设备的「服务器存储」中选择 WebDAV 或 SFTP，使用同一个配对码和远端目录。WebDAV 使用 HTTPS，支持 HTTP Basic 认证。SFTP 使用密码认证，并按填写的 SHA-256 指纹校验 SSH 主机密钥。服务器保存加密后的历史。保持局域网同步开启即可同时使用两条链路。
4. 粘贴另一台设备上的文件时，在历史中选中对应条目，按平常方式复制或粘贴。UClip 会从局域网内在线的来源设备拉取原文件。支持普通文件及文件夹，每次最多传输 256 MiB。接收端会暂存文件，并在下次启动或传输前清理超过 24 小时的副本。

同步后的条目在置顶与未置顶组内按采集或保存时间排序。同一条目的编辑、移入回收站和恢复发生冲突时，保留最新操作。永久删除优先于同一条目的其他操作，包括设备离线时执行的恢复。局域网同步和服务器存储使用 v4 协议。共享历史的内容上限为 2 GiB，可在「设置 → 同步」查看此设备上的共享历史用量。已同步记录的保留独立于本机的采集条数设置。

启用批量上传的 Nextcloud WebDAV 会分批接收加密历史对象。其他 WebDAV 服务器采用有限并发上传。同步会复用服务器上已验证的对象，各客户端继续使用同一种归档格式。

在「服务器存储」中点击「立即同步服务器存储」，可请求立即与已配置的服务器同步。

在「设置 → 同步」中点击「打开配置和数据库目录」，可找到当前桌面端使用的库。

## 常用快捷键

| 操作 | 默认快捷键 |
| --- | --- |
| 显示或隐藏搜索和筛选 | Ctrl/Cmd+F |
| 设置 | Ctrl/Cmd+, |
| 回粘单条所选记录，不可用时复制 | Enter |
| 复制所选记录 | Ctrl/Cmd+C |
| 查看完整内容 | Space |
| 删除所选记录 | Delete |
| 返回、清空搜索或隐藏窗口 | Esc |

## 平台

| 平台 | 当前状态 |
| --- | --- |
| Linux X11 | 已在 Linux Mint 22 完成原生验证，支持采集与自动回粘 |
| Linux Wayland | 本地界面和历史查询已验证；采集、自动回粘暂不支持 |
| Windows / macOS | 已有适配代码，尚未完成原生验收 |

UClip 仍在开发中。归档导入和 Cliporax 迁移请参阅[备份或迁移历史](#备份或迁移历史)。

## 命令行与无桌面同步

桌面安装包内含 `uclip` 命令。Linux deb 会将它装到系统命令路径。Windows 可从 UClip 安装目录运行 `uclip.exe`。macOS 可使用 `/Applications/UClip.app/Contents/MacOS/uclip`。也可以用下方命令单独从源码安装。历史与无桌面同步命令可在没有显示服务的环境运行。

```bash
cargo install --locked --path crates/uclip-cli
printf 'A note\n' | uclip save
uclip list --json
uclip search note --full
uclip get latest --raw
uclip copy --file ./notes.txt
```

`get` 支持 UUID、`latest`、从 1 开始的 `--position` 和从 0 开始的 `--index`。在终端中显示文本时会转义换行和制表符以外的控制字符。`--raw` 输出原文本字节且不额外追加换行。图片以 PNG 输出，文件引用以 JSON 路径数组输出。`list` 和 `search` 可按分类名或 UUID 筛选，并用 `--limit` 限制条数。`--full` 从同一历史快照逐行输出包含完整内容的 JSON。`--image` 读取图片，`--file` 读取文本，重复使用 `--reference` 可保存多个路径。`completion bash` 和 `completion zsh` 生成补全脚本。诊断走 stderr。退出码 `0` 表示成功，`1` 表示其他错误，`2` 表示输入无效，`3` 表示数据不存在，`4` 表示冲突或库忙。搜索无匹配时成功并返回零条。

默认使用相同构建配置的桌面库。`--dev` 指向 `tauri:dev` 的库，`--portable` 使用可执行文件旁 `uclip-data` 下的对应配置，`--data-dir` 或 `UCLIP_DATA_DIR` 可指定绝对目录。Unix 上请使用仅当前账户可访问的目录。无桌面也可读写历史。写入系统剪贴板需要受支持的桌面会话，远端文件引用的实际复制继续走桌面端的局域网取回流程。X11 上，超过服务器单次请求大小的内容会返回 `limit_exceeded`。

命令行帮助和诊断优先跟随终端语言（依次读取 `LC_ALL`、`LC_MESSAGES`、`LANG`），Windows 和 macOS 也可使用系统显示语言。中文地区使用简体中文，其他地区使用英文。设置 `UCLIP_LANG` 为 `zh-CN` 或 `en` 可指定语言，设为 `system` 可自动选择。历史输出和退出码与语言无关。

要让一个加密同步节点常驻，可以用独立目录初始化并启动：

```bash
export UCLIP_DATA_DIR="$HOME/.local/share/uclip-server"
uclip sync create
uclip sync code
uclip serve
```

请像保护密码一样保管配对码，并在其他设备上填写。已有同步组可把配对码通过标准输入交给 `uclip sync join`。`uclip sync remote < profile.json` 从标准输入读取 WebDAV 或 SFTP 配置，格式见 `uclip sync remote --help`。`uclip sync status` 显示已保存的设置，不含凭据。局域网使用 IPv4 组播发现，无桌面节点参与与桌面端相同的主机选举。同一个库只运行一个同步进程，更改同步设置前先停止它。`serve` 前台运行，支持 Ctrl+C 和 Unix SIGTERM，可直接交由服务管理器监管。

## 备份或迁移历史

使用当前版本的 UClip CLI。对已有的独立库，`import` 默认预演。退出 UClip 后加 `--apply` 正式导入，并在库目录下生成经校验的 SQLite 备份。可用 `--data-dir`、`--dev` 或 `--portable` 选择 UClip 库。

```bash
uclip export --output history.jsonl
uclip import --input history.jsonl
uclip import --input history.jsonl --apply
```

明文归档含文本、图片、本机文件引用、分类、置顶、时间戳和回收站，请妥善保管。其他设备的文件引用依赖原设备，导出时会跳过。导入保留记录 ID，跳过目标库中已有或已永久删除的记录。超出容量或分类名称冲突时，整批回滚。要恢复导入前的备份，先退出 UClip 和无桌面同步服务，将当前数据库和 SQLite 附属文件移出原目录，再把备份复制为 `foundation.sqlite3`。

在 Windows、macOS 或 Linux 上从 Cliporax 迁移时，先打开一次 UClip 初始化目标库，再退出两个应用。脚本使用 Python 3.11 或更新版本。

```bash
python scripts/migrate-cliporax.py --source "/path/to/cliporax.db"
python scripts/migrate-cliporax.py --source "/path/to/cliporax.db" --apply
```

省略 `--source` 时读取 Cliporax 标准安装路径。从其他系统带来的备份可用 `--source-platform windows`、`macos` 或 `linux` 指明来源。如果旧库没有设备 ID，请通过 `--source-id` 指定一个固定值，后续重跑沿用该值。脚本报告标记敏感、损坏、超限或路径不兼容等跳过项。文件引用仍需要原路径上的文件。应用设置、同步凭据和插件数据保留在 Cliporax。当前 CLI 不在 PATH 时，用 `--uclip` 指定可执行文件。

## 从源码运行

安装 Node.js、Rust 和对应平台的 [Tauri 构建依赖](https://v2.tauri.app/start/prerequisites/)后运行：

```bash
npm ci
npm run tauri:dev
```

运行 `npm run dev` 可在浏览器中体验内存示例界面。浏览器仅记住语言与外观偏好，示例记录刷新后还原。

## 安装到本机 Linux

准备好上述构建依赖后，在 Linux 桌面会话的终端中进入源码目录并运行 `./scripts/install.sh`。脚本会构建 UClip、为当前用户安装、添加应用菜单入口并启动应用，无需 `sudo`。更新源码后再次运行，构建完成后脚本会关闭由此脚本安装的运行实例，再启动新安装的版本。

## 构建安装包

```bash
npm run package
```

在对应操作系统构建：Linux 生成 deb 和 AppImage，Windows 生成 NSIS 安装程序，macOS 生成 app 和 dmg。

Windows 安装程序会复用系统已有的 WebView2 Evergreen 运行时。缺少时，安装程序会联网下载并安装它。UClip 的 MSVC C 运行库采用静态链接。运行 `bundle/nsis` 下的安装程序即可安装 UClip。

Linux 的 deb 包声明 GTK、WebKitGTK 和托盘库依赖，由包管理器安装。AppImage 随带这些库，仍使用宿主系统的 glibc。请在计划支持的最旧 Linux 发行版上构建分发包，以打包工作流的 Ubuntu 22.04 环境为基线。macOS 应用包使用系统自带的 WKWebView。

## 在 GitHub Actions 中打包

从干净且已同步的 `main` 运行 `npm run release -- <版本号> --package-only`，脚本会更新版本并将版本提交推到 `main`。一次 **UClip CI** 运行完成 Linux、Windows、macOS Apple Silicon 和 Intel 的检查与打包，可在该次运行的产物中下载五个安装包。

用 `npm run release:watch -- <版本号>` 跟踪打包进度，加 `--once` 可只查询当前状态。运行 `npm run release -- <版本号>` 请求发布 GitHub 预览版。发布请求推送后，即使本地命令退出，GitHub Actions 也会在验证通过后为同一提交打标签并发布。这条命令也可以发布已用 `--package-only` 打包的版本。详见[发布流程](AGENTS.md)。
