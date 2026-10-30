# JoyManga

<p align="center">
  <img src="src/assets/icon.png" width="96" height="96" alt="JoyManga Logo" />
</p>

<p align="center">
  <strong>基于 Tauri v2 + Rust + Vue 3 的 Android 漫画下载与离线阅读客户端</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Version-v0.3.6-ff69b4.svg" alt="Version 0.3.6" />
  <img src="https://img.shields.io/badge/Platform-Android-0969da.svg" alt="Platform" />
  <img src="https://img.shields.io/badge/Core-Rust%20%2B%20Tauri%20v2-ea580c.svg" alt="Tauri v2" />
  <img src="https://img.shields.io/badge/Frontend-Vue%203%20%2B%20Naive%20UI-10b981.svg" alt="Vue 3" />
</p>

---

## 📌 项目概述

**JoyManga** 是一套面向 Android 的全功能漫画工具。它不仅具备工业级的批量高速下载引擎，还深度集成了现代化离线漫画阅读器与书库管理系统。

底层基于 **Rust** 编写核心异步运行时与反混淆切片图像还原算法，兼顾极致性能与极低内存占用；前端采用 **Vue 3 + Naive UI + UnoCSS**，提供适配手机与平板的操作界面，在 Android 设备上获得流畅的原生级体验。

---

## ⚡ 核心功能一览

### 1. 🚀 高性能并发下载与网络调度
- **反混淆极速解码**：基于 Rust 高性能并行算法，毫秒级完成切片混淆图片的重组与无损保存。
- **双层并发调度**：章节级与图片级独立并发控制，支持自定义线程数与请求间隔，兼顾下载速度与防风控。
- **断点续传与持久化队列**：下载进度实时落盘，异常退出或重启后自动恢复未完成任务。
- **多线路测速与 CDN 分流**：内置多组 API 与图片 CDN 线路，支持实时延迟测速、一键智能优选或手动切换。
- **网络代理支持**：原生支持系统代理、HTTP 代理、SOCKS5 代理及直连模式。

### 2. 📖 内置轻量漫画阅读器
- **单页与滚动阅读**：直接打开已归档的 CBZ 或下载目录，支持单页翻页与上下滚动两种模式；单页模式可切换"适合宽度 / 适合屏幕"。
- **手势与按键翻页**：跟手滑动、边界回弹；外接键盘的 `←/→`、`A/D`、空格均可用，`Esc` 关闭。
- **进度控制与微调**：底部悬浮进度条支持拖动定位、页码显示，以及前后逐页按钮；章末自动衔接下一章。
- **阅读记忆与书签**：自动记忆每部漫画的已读章节与页码，支持在任意页添加书签并跳转。

### 3. 🗄️ 本地书库与归档导出
- **多话合并单体 CBZ 导出（v0.2.3）**：支持将一部漫画的所有已下载章节合并打包为一个独立的 `.cbz` 文件，告别零散文件夹。
- **分卷 CBZ 导出**：可按话分卷导出为 CBZ 压缩包。
- **元数据内嵌（ComicInfo.xml）**：导出的 CBZ 自动嵌入标题、作者、分类标签、话数等标准元数据，与 Tachiyomi、Mihon、Komga、Kavita 等第三方服务器及阅读器无缝衔接。
- **外部漫画导入**：支持将本地已有的 CBZ 漫画或图片文件夹直接扫描加入书库管理。

### 4. 📱 Android 移动端专属优化
- **固化统一证书签名**：项目固化了标准化发布证书，本地构建与 GitHub CI 云端打包证书完全同源，**版本更新直接无缝覆盖安装，绝无签名冲突报错**。
- **单一 arm64-v8a 架构**：剔除冗余 ABI，包体轻巧精炼，大幅降低手机存储开销。
- **公共存储卸载保护**：漫画默认下载至系统公共目录 `Download/JoyManga`，卸载或更新应用时**漫画文件绝不丢失**，支持第三方应用直接访问。
- **移动交互细节**：适配沉浸式全面屏、多级返回手势拦截、后台下载防系统休眠与通知中心进度推送。

---

## 📥 下载与安装

请前往 [GitHub Releases](https://github.com/Beyondoooars/JoyManga/releases) 获取经过验证的稳定版本：

| 平台 | 文件包格式 | 适用场景与说明 |
| :--- | :--- | :--- |
| **Android** | `joymanga-arm64-v8a.apk` | 主流 64 位安卓手机/平板设备，支持无缝覆盖更新 |

---

## 🔧 常见问题 (FAQ)

<details>
<summary><strong>Q: 手机安装新版本时提示“软件包冲突 / 签名不一致”？</strong></summary>

> **原因与解决**：
> - 从 `v0.2.1` 起，项目已固化统一签名密钥。如果您之前安装过非官方构建或历史临时签名的测试包，首次升级时请**先卸载手机上的旧版本**，然后安装新版。
> - 放心卸载：下载的漫画存放在系统公共目录 `Download/JoyManga`，卸载应用**不会删除已下载的漫画**。
> - 首次重装后，后续的所有发布版本均可直接覆盖安装升级，不再冲突。
</details>

<details>
<summary><strong>Q: 遇到漫画封面或章节图片加载缓慢、解析失败？</strong></summary>

> **解决建议**：
> 1. 打开应用【设置】➔【线路分流与测速】。
> 2. 点击“一键测速”，选择当前网络环境下延迟最低、可用性最好的 API 线路与图片 CDN 线路。
> 3. 若处于特殊网络环境，可在设置中开启 HTTP/SOCKS5 代理。
</details>

<details>
<summary><strong>Q: Android 端后台下载被系统中断或暂停？</strong></summary>

> **解决建议**：
> - 国产安卓系统（MIUI/HyperOS/ColorOS/OriginOS/HarmonyOS 等）默认策略会在锁屏后休眠后台应用。
> - 请前往手机【系统设置 ➔ 应用管理 ➔ JoyManga ➔ 省电策略/电池优化】，将其调整为**“无限制” / “允许后台运行”**。
</details>

<details>
<summary><strong>Q: 导出的 CBZ 文件在其他阅读器中打不开或看不到封面？</strong></summary>

> **说明**：
> - 导出的 CBZ 属于标准化 ZIP 容器规范，并在根目录内嵌了 `ComicInfo.xml`。
> - 兼容市面上绝大多数第三方工具，如 Tachiyomi、Mihon、Kavita、Komga、EhViewer、CDisplayEx 等。若个别老旧阅读器报错，可在导出时选择分卷或检查存储读取权限。
</details>

---

## 🛠️ 本地开发与构建

如果您希望自行从源码编译本应用，请参考以下步骤：

### 前置依赖
- **Node.js** (>= 18.0) 及 **pnpm** (>= 9.0)
- **Rust** 稳定版工具链 (`rustc`, `cargo` >= 1.78)
- **Android SDK 34**, **NDK 26+**, **Java 17**

### 常用命令

```bash
# 1. 克隆代码并安装前端依赖
git clone https://github.com/Beyondoooars/JoyManga.git
cd JoyManga
pnpm install

# 2. 启动前端调试环境 (热重载)
pnpm dev

# 3. 运行代码检查与测试套件
pnpm exec eslint .
pnpm test
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings

# 4. 编译 Android arm64-v8a APK
pnpm build:android
```

### 版本发布

推送到 `main` 即触发云端打包，无需本地改版本号：

1. GitHub Actions 自动递增 patch 版本（`package.json`、`tauri.conf.json`、`Cargo.toml`、`Cargo.lock`、README 徽章同步）。
2. 编译并校验 arm64-v8a 签名 APK。
3. 校验通过后把版本号提交回 `main`，创建 `vX.Y.Z` Release 并上传 APK，应用内「检查更新」即可发现新版本。

需要升 minor/major 时，在 Actions 面板手动运行 **Android Build**，`version` 输入 `minor`、`major` 或明确的 `X.Y.Z`。
本地仍可用 `pnpm bump <patch|minor|major|X.Y.Z>` 手动同步版本号。

日常修复直接提交到 `main`，每次推送都会自动打包并发布 patch 版本。需要升 minor/major 时，在提交信息末尾加一行 `Release-Bump: minor`（或 `major`），也可在 Actions 面板手动指定版本；`main` 始终是可日常使用的版本。

---

## 📜 声明

1. **许可协议**：本项目以 [MIT License](LICENSE) 开源，版权归属与许可条款以 LICENSE 文件为准；内嵌第三方组件（OpenCC 词典）的许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
2. **免责声明**：本项目所有代码与编译产物仅供个人离线学习、技术研究及归档备份使用。禁止用于任何商业用途、付费分发或侵犯版权的行为。因使用本工具产生的一切法律纠纷与责任均由使用者自行承担。
