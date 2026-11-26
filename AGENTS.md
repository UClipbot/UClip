# UClip 发布规则

## 开始发布

- 只有用户明确要求发布时才推送发布标签。先审查并提交本次要打包的改动，再将 `main` 与 `origin/main` 同步。确认工作区干净，并确认要打包的改动包含在当前 `main` 中。
- 未指定版本时，根据已有版本和本次改动选择新的 SemVer 版本。已发布的版本和标签不得覆盖或移动。当前 GitHub 工作流发布预览版安装包。
- 本机需要仓库要求的 Node.js、npm、Rust 和 GitHub CLI。完成 `npm ci`，确认 `gh auth status` 正常，并确认 Git 能推送到 `origin`。从 `main` 运行 `npm run release -- <版本号>`，传入不含 `v` 的版本号。只更新版本并打包时，加上 `--package-only`。

## 版本一致性

- 发布脚本同步更新 `Cargo.toml` 的 `[workspace.package].version`、`package.json`、`package-lock.json` 的顶层及 `packages[""]`、`src-tauri/tauri.conf.json`。
- 用 `cargo update --workspace` 同步 `Cargo.lock` 中所有 workspace crate 的版本。目前是 `uclip-cli`、`uclip-core`、`uclip-desktop` 和 `uclip-runtime`。各 crate 通过 `version.workspace = true` 继承应用版本，CLI 和桌面界面也读取 Cargo 包版本。
- 检查 npm 两处锁定版本、Tauri 版本、Cargo workspace 和所有 crate 的锁定版本完全一致。开发配置继承生产版本。数据库 schema、IPC 契约、同步协议、最低系统版本及第三方依赖版本各自独立，不因应用发布号变动而递增。

## 打包与发布完成条件

1. 发布脚本运行 `npm run check`，以 `Package UClip <版本号>` 为提交标题提交版本改动，并推送到 `main`。明确发起发布时，同时推送指向同一提交的 `publish/v<版本号>` 分支作为发布请求。提升已打包版本时，只推送该发布请求。
2. `UClip CI` 对该提交运行一次检查和打包。打包矩阵覆盖 Linux、Windows、macOS Apple Silicon 和 Intel，并检查安装包内 `uclip --version`。使用 `--package-only` 时，打包验证通过即完成。
3. `package.yml` 的发布任务按提交 SHA 等待打包通过并取回产物，再创建指向同一提交的注释标签 `v<版本号>`，发布包含五个安装包和 `SHA256SUMS.txt` 的 GitHub 预览版。发布请求推送后，本地进程退出也不影响 GitHub Actions 继续执行。
4. 发布任务和本地发布脚本均确认 Release 已公开，下载五个安装包及清单并重新计算 SHA-256。向用户报告 Release 链接、版本和提交，以及仍未通过或未执行的验证。

日常 `main` 推送和 PR 运行四平台检查及 Debug 编译。版本提交和手动触发的打包在同一个矩阵中运行检查、构建及验证安装包。

单独检测打包结果时，运行 `npm run release:watch -- <版本号>`。脚本按远端 `main` 中的版本提交及 SHA 跟踪，也支持已有的 `package/v<版本号>` 打包记录。状态有变化才输出，打包失败时返回非零。加 `--once` 可只查询一次。

若提交或推送后的步骤失败，保留同一发布提交。重新运行相同命令可继续跟踪原发布请求。若打包或发布任务失败，查看对应的 Actions 日志，按需先重跑打包任务，再重跑发布任务。保留已推送的发布请求和标签，修复需要改动源码时使用新版本号。
