# JoyManga

<p align="center">
  <img src="src/assets/icon.png" width="96" height="96" alt="JoyManga Logo" />
</p>

<p align="center">
  <strong>基于 Tauri v2 + Rust + Vue 3 的 Android 漫画下载、CBZ 书库管理与离线阅读客户端</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Version-v0.3.20-ff69b4.svg" alt="Version 0.3.20" />
  <img src="https://img.shields.io/badge/Platform-Android%20(arm64--v8a)-0969da.svg" alt="Platform Android" />
  <img src="https://img.shields.io/badge/Core-Rust%20%2B%20Tauri%20v2-ea580c.svg" alt="Tauri v2" />
  <img src="https://img.shields.io/badge/Frontend-Vue%203%20%2B%20Naive%20UI-10b981.svg" alt="Vue 3" />
</p>

---

## 简介

**JoyManga** 是一款面向 64 位 Android 设备（`arm64-v8a`，Android 7.0+）的开源漫画下载与离线阅读应用。

- **后端核心**：基于 Rust 与 Tokio 异步运行时，负责章节并发抓取、切片图像反混淆还原、整本 CBZ 归档打包与本地书库索引维护。
- **前端界面**：基于 Vue 3、Pinia、Naive UI 与原生 CSS（Design Tokens），围绕手机与平板触控交互设计。
- **原生集成**：通过 Kotlin 原生桥接处理 Android 前台下载服务、公共存储（`Download/JoyManga`）读写、SAF 导入导出、系统网络/VPN 监测及应用内更新安装。

---

## 主要功能

### 浏览、搜索与账号

- **简繁双变体搜索与筛选**：内置 OpenCC 简繁词典，搜索时自动补全繁简体变体并合并去重结果，支持按语言/标签二次过滤。
- **每周必看与详情页**：按期数与分类浏览每周推荐作品，支持查看详情与按话选择下载。
- **账号、收藏与签到**：支持账号登录、云端收藏同步（含多文件夹查看与批量下载收藏）及每日签到日历统计。

### 下载与网络调度

- **并行反混淆与双层并发**：使用 Rayon + Rust `image` 并行重组混淆切片；支持设置章节并发数与单话图片并发数（所有章节共用图片总并发上限）。
- **任务队列落盘与断点续传**：下载进度、阶段状态与失败明细实时写入本地日志/状态文件，应用重启或异常中断后可恢复未完成任务，支持按任务暂停、继续、重试失败与批量取消。
- **多线路测速与自动切换**：内置多组 API 与图片 CDN 线路，可并发下载样本测试实际吞吐量（MB/s）并一键切换最快线路，也支持自定义域名。
- **代理管理与网络状态感知**：支持跟随系统网络、直接连接与手动 HTTP 代理。检测到系统 VPN 开启时会自动旁路手动代理并清空旧连接池；Wi-Fi 与移动数据切换时自动重置网络状态，开启「仅 Wi-Fi 下载」后在计费网络下自动暂停。

### 本地书库与整本 CBZ 归档

- **自动整本合并**：漫画下载完成后，通过临时文件与原子替换链自动将已下载章节合并为单体 `.cbz` 文件；历史遗留的分话目录可在书库卡片菜单中一键整理为整本 CBZ。
- **第三方阅读器兼容**：CBZ 采用标准 ZIP 容器以 `STORE`（无压缩）写入图片，并在根目录嵌入包含标题、作者、标签与话数信息的 `ComicInfo.xml`，兼容 Tachiyomi、Mihon、Kavita、Komga 等客户端与服务器。
- **外部导入与系统打开**：支持从本地文件或目录批量导入 `.cbz` 与图片漫画，也支持在系统文件管理器中通过「用 JoyManga 打开」直接导入并进入阅读。
- **备份还原与诊断包导出**：支持导出或恢复配置、任务队列与本地阅读进度/书签，以及一键生成脱敏后的诊断日志包以便排查问题。

### 离线阅读器

- **单页与滚动双模式**：直接读取本地 CBZ 或目录；单页模式提供「适合宽度 / 适合高度 / 适合屏幕 / 原始尺寸」四种适配，支持双击 2 倍放大、双指捏合缩放（最高 5 倍）及放大状态下单指拖拽平移；上下滚动模式对屏幕窗口外的页面按比例占位并卸载图片，控制长章节内存占用。
- **操控与快捷翻页**：支持点击翻页热区、左右滑动（随日漫右往左 / 从左往右设置联动）、Android 音量键翻页与外接键盘方向键、`A`/`D`、空格及 `Esc`。
- **阅读进度与书签**：自动防抖保存每部漫画的当前章节与页码，章末支持衔接上一话/下一话，可随时添加、查看并跳转到指定页书签。

### Android 专项支持

- **默认公共存储目录**：优先使用系统公共目录 `Download/JoyManga`（在权限允许的系统版本下），卸载重装不会删除已下载的漫画；也支持随时切换回应用专用目录。
- **16 KB 页对齐与固化签名**：构建针对 `arm64-v8a` 启用 NDK `29.0.14206865` 16 KB 页对齐；仓库与 CI 使用同一套发布签名证书，同一来源的版本可直接覆盖升级。
- **应用内更新**：定期或手动检查 GitHub Releases 新版本，通过原生前台服务下载 APK 后直接拉起系统安装器完成安装。

---

## 下载与安装

请前往 [GitHub Releases](https://github.com/Beyondoooars/JoyManga/releases) 下载最新版本：

| 平台 | 安装包 | 系统要求与说明 |
| :--- | :--- | :--- |
| **Android** | `joymanga-arm64-v8a.apk` | Android 7.0+ 64 位设备（`arm64-v8a`），同源发布版支持直接覆盖更新 |

---

## 常见问题

<details>
<summary><strong>安装新版时提示「软件包冲突 / 签名不一致」怎么办？</strong></summary>

- 本仓库从 `v0.2.1` 起已固化发布签名密钥；若手机上原本安装的是旧临时签名测试包或第三方重打包版本，请先卸载旧版再安装新版，后续版本即可直接覆盖升级。
- 下载在系统公共目录 `Download/JoyManga` 中的漫画文件不会随卸载被删除。

</details>

<details>
<summary><strong>封面或章节图片加载慢、频繁失败怎么排查？</strong></summary>

1. 进入应用【设置】→【网络】→【手动设置线路】，点击「测速全部图片线路」查看各线路实际吞吐量与可用性，选择最快线路或更换接口线路。
2. 可使用「检查连接」逐项排查接口、图片与域名状态；若需在社区反馈，可到【设置】→【数据】导出脱敏诊断包。
3. 切换或开启系统 VPN 后建议重新测速；若已连接系统 VPN 却无法联网，应用会自动旁路手动 HTTP 代理以避免端口冲突。

</details>

<details>
<summary><strong>锁屏或切到后台后下载任务被系统杀掉怎么处理？</strong></summary>

- 国产定制 Android 系统通常会在锁屏或后台一段时间后限制网络与后台服务，请在系统【应用管理 → JoyManga → 电池优化 / 省电策略】中设为「无限制」或允许后台高耗电运行，并保持通知权限开启以便显示下载前台服务通知。

</details>

<details>
<summary><strong>导出的 CBZ 在第三方阅读器中打不开或缺少封面？</strong></summary>

- 本应用归档的 `.cbz` 为标准 ZIP 文件（`ComicInfo.xml` 位于根目录，图片以无压缩格式存储），兼容 Tachiyomi、Mihon、Komga、Kavita 等工具。如遇到异常，请确认文件管理器/阅读器具备存储访问权限，或检查文件大小是否因强制中断而不完整。

</details>

---

## 本地开发与构建

### 环境要求

- **Node.js**：`>= 26.13.0`（对应 `package.json` `engines.node`）与 **pnpm** `11.16.0`
- **Rust**：稳定版工具链（`rustc` / `cargo`）
- **Android 构建依赖**：Java 17、Android SDK 35、Android NDK `29.0.14206865`（需 16 KB 页对齐支持）

### 常用命令

```bash
# 1. 克隆仓库并安装前端依赖
git clone https://github.com/Beyondoooars/JoyManga.git
cd JoyManga
pnpm install

# 2. 启动前端开发服务器（支持在浏览器访问 /tests/ui.html 查看离线模拟界面）
pnpm dev

# 3. 运行与 GitHub Actions 一致的全量检查
#    (eslint + node --test + vue-tsc && vite build + cargo fmt + cargo clippy + cargo test)
pnpm check

# 4. 启用本地 pre-push 钩子（推送 main 前自动运行上面检查）
pnpm setup:hooks

# 5. 生成前后端 TypeScript 绑定（修改 Rust IPC 命令/事件后执行）
pnpm gen:bindings

# 6. 构建 Android arm64-v8a APK（Release / 轻量体积 Debug 测试包）
pnpm build:android
pnpm build:android:test
```

### 自动打包与发布流程

每次向 `main` 分支推送代码时，GitHub Actions（[android-build.yml](.github/workflows/android-build.yml)）会执行以下流程：

1. **前置检查**：先运行前端（`eslint`、`pnpm test`、`pnpm build`）与 Rust（`fmt`、`clippy`、`cargo test`）全部门禁，任意失败立即中止。
2. **版本递增**：默认按 patch 自动递增版本并同步写入 `package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 与 `README.md` 版本徽章。若需升级 minor 或 major，可在提交说明末尾加一行 `Release-Bump: minor`（或 `major`），或在 Actions 面板手动运行 **Android Build** 并指定版本号。
3. **签名构建与发布**：编译并验证 `arm64-v8a` APK 签名证书及版本元数据，通过后回写版本提交并创建对应 Release。

> 版本号提交会自动推回 `origin/main`，因此本地下一次推送前需要先执行 `git pull --rebase`；也可使用 `pnpm bump <patch|minor|major|X.Y.Z>` 在本地同步版本号。

---

## 许可与声明

1. **开源协议**：本项目基于 [MIT License](LICENSE) 开源。内嵌第三方组件（OpenCC 简繁词典等）的授权声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
2. **免责声明**：本工具仅用于个人离线阅读体验改进与技术研究备份，请勿用于任何商业用途、二次打包分发或侵犯版权的行为；因使用本项目产生的法律责任由使用者自行承担。
