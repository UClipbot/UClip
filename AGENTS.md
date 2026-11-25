# UClip 发布规则

## 开始发布

- 只有用户明确要求发布时才推送发布分支和标签。先审查并提交本次要发布的改动，再将 `main` 与 `origin/main` 同步。确认工作区干净，并确认要发布的提交包含在当前 `main` 中。
- 未指定版本时，根据已有版本和本次改动选择新的 SemVer 版本。已发布的版本和标签不得覆盖或移动。当前 GitHub 工作流发布预览版安装包。
- 本机需要仓库要求的 Node.js、npm、Rust 和 GitHub CLI。完成 `npm ci`，确认 `gh auth status` 正常，并确认 Git 能推送到 `origin`。从 `main` 运行 `npm run release -- <版本号>`，传入不含 `v` 的版本号。

## 版本一致性

- 发布脚本同步更新 `Cargo.toml` 的 `[workspace.package].version`、`package.json`、`package-lock.json` 的顶层及 `packages[""]`、`src-tauri/tauri.conf.json`。
- 用 `cargo update --workspace` 同步 `Cargo.lock` 中所有 workspace crate 的版本。目前是 `uclip-cli`、`uclip-core`、`uclip-desktop` 和 `uclip-runtime`。各 crate 通过 `version.workspace = true` 继承应用版本，CLI 和桌面界面也读取 Cargo 包版本。
- 检查 npm 两处锁定版本、Tauri 版本、Cargo workspace 和所有 crate 的锁定版本完全一致。开发配置继承生产版本。数据库 schema、IPC 契约、同步协议、最低系统版本及第三方依赖版本各自独立，不因应用发布号变动而递增。

## 打包与发布完成条件

1. 发布脚本运行 `npm run check`，提交版本改动，将同一提交推到 `main` 和 `package/v<版本号>`。
2. 等待该提交的 `Foundation checks` 和 `Build installers` 全部成功。打包矩阵覆盖 Linux、Windows、macOS Apple Silicon 和 Intel，并检查安装包内 `uclip --version`。
3. 验证完成后才推送指向同一提交的注释标签 `v<版本号>`。`package.yml` 的标签任务按提交 SHA 取打包产物，发布包含五个安装包和 `SHA256SUMS.txt` 的 GitHub 预览版。
4. 发布脚本确认 Release 已公开，下载五个安装包及清单并重新计算 SHA-256。向用户报告 Release 链接、版本和提交，以及仍未通过或未执行的验证。

若提交或推送后的步骤失败，保留同一发布提交。本地或 CI 故障解决后，重新运行相同命令即可继续。如果 CI 任务本身失败，先查看日志；可恢复的失败可重跑原任务。已经推送的标签不得重写，修复需要改动源码时使用新版本号。
