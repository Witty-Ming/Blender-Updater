# Blender 更新器

一个独立的 Windows 桌面程序，用来更新本机的 Blender 便携版安装，代替手动下载 zip 再解压覆盖。

作者：**WittyMing** · <https://space.bilibili.com/263206115>

## 直接使用

双击 `Blender更新器.exe` 即可，无需安装、无需 Python 或其他运行环境（单个 exe，约 5 MB）。

程序会记住扫描到的安装目录和各项设置，配置文件在 `%APPDATA%\BlenderUpdater\config.json`。

## 功能

### 双更新通道，按版本系列更新

- **稳定版**：从 `download.blender.org/release` 及国内镜像取当前系列（如 5.2）的最新正式版
- **每日构建**：从 `builder.blender.org` 的每日构建**归档**接口（`/download/daily/archive/`）取**当前系列对应分支**（如 v52）的最新构建，构建日期按本地时间显示，与官网 archive 页面一致；不会把主线 main 的 alpha 包塞给你

频道只会更新「同系列的小版本」。当官方发布了更高的系列（例如当前是 5.2，官方已经有 5.3），主界面会单独显示一张黄色提示卡，说明跨系列升级不会自动覆盖，并给出官方下载目录链接 —— 是否升级、怎么迁移插件，由你自己决定。

每日构建里版本号相同时，按构建类型（内测 / 测试 / 候选 / 正式）和构建日期判断是否需要更新，卡片会同时列出当前构建与最新构建的日期。如果当前装的是正式版、归档里只剩同号候选版，就不会提示更新，并在来源里注明「正式版 x.y.z 已发布」。

### 覆盖式更新，目录路径不变

- 下载 → 解压到同盘 staging → 原子改名交换 → 后台清理
- 原来的 `portable/` 会被完整搬进新版本，配置、插件、扩展全部保留
- 交换是同一磁盘上的元数据操作，通常不到 2 秒；原目录不可用的窗口极短
- 中途失败不会破坏正在用的安装

### 显示实际版本

界面显示的版本号与构建日期来自 `blender.exe --version` 的实测结果（Build Hash / Build Date / Build Type），而不是目录名。目录叫 `blender-5.2.0-windows-x64` 但实际是 5.2.2 LTS 时，大字显示 5.2.2，副行标注「目录名 5.2.0」。探测结果按 exe 的修改时间和大小缓存，换了版本会自动重新探测。

### 检查结果缓存

每个「安装 + 通道」组合的检查结果会缓存 3 分钟。切换安装目录或切换通道再切回来，结果直接复用，不用重新等网络。

### 配置备份（快照）

- 随时给 `portable/config` 拍快照，带日期、时间、原因标记（手动备份 / 更新前 / 恢复前）
- 快照列表显示 Blender 版本、文件数、体积，可一键恢复或删除
- 手动备份永久保留，程序不会自动清理
- 可设置每个安装自动保留的份数（1–50）
- 可设置更新前是否自动备份
- **备份位置可自定义**：改位置时已有快照会自动迁移过去，旧位置不留残余；点「恢复默认」也会迁回来

### 其他

- 自动发现安装：扫描各磁盘根目录下名字含 `blender` 的文件夹，也支持手动添加
- 无边框窗口 + 自绘标题栏，深色界面
- 优先使用国内镜像（清华 / 阿里），失败自动回退官方源
- 实时进度：检查、备份、下载、解压各阶段都有进度和速度显示

## 更新过程做什么

1. 检查当前系列的最新版本（对比实测版本号）
2. 按设置给 `portable/config` 拍一份 `before_install` 快照
3. 下载安装包到 `%APPDATA%\BlenderUpdater\downloads`（校验 SHA-256）
4. 解压到同盘 `.blender-updater\staging`（此时原 Blender 仍可正常使用）
5. 验证新 exe 可执行
6. 原目录挪进同盘的隐藏 staging → staging 的新版本改名为原目录 → `portable/` 搬回新目录
7. 后台删除移走的旧目录，安装目录旁边不会留下 `xxx.old`

## 残留清理

旧版本目录会在交换完成后立刻移进隐藏的 `.blender-updater` staging，再后台删除，所以安装目录旁边**不会长期留下** `xxx.old`。

如果程序在删除过程中被关掉（1.4 GB / 七千多个文件在机械硬盘上要几十秒），残留的 `xxx.old` 和 staging 会在**下次启动时自动清理**，扫描安装列表时也不会把 `xxx.old` 当成一个安装显示出来。

如果清理失败是因为某文件被别的程序占用（常见于文件管理器的缩略图、杀毒软件实时扫描），关掉那个程序后重启本程序即可删掉。

## 开发模式（改完直接看效果，不用打包 exe）

双击项目根目录的 **`开发模式运行.bat`**，它会用 `cargo run` 启动 **debug 构建**：

- 编译比 release 快很多（release 每次约 1 分半，debug 增量通常几秒）
- 界面、功能、行为与正式版完全一致
- 关掉窗口 → 改代码 → 再双击一次，就能看到改动
- 窗口里按 **F12** 可以打开开发者工具，看控制台日志

改完满意了，再按下面的方式编译 release：

## 从源码构建

需要 Rust 工具链和 Windows 的 WebView2（Windows 10/11 自带）。

```bash
cd blender-updater-rs
cargo build --release
```

产物在 `target/release/blender-updater.exe`。

改动 `ui/` 下的静态资源后，需要先 `touch src/main.rs` 再编译，否则新资源不会被嵌入。

## 目录结构

```
blender-updater-rs/
├── src/            Rust 后端
│   ├── main.rs     Tauri 入口
│   ├── commands.rs 前端接口 + 更新流程编排
│   ├── release.rs  稳定版源（三镜像并发 + 5 秒预算）
│   ├── daily.rs    每日构建源（按分支过滤）
│   ├── downloader.rs 带进度的 HTTP 下载
│   ├── installer.rs  解压、校验、原子替换
│   ├── snapshots.rs  配置快照（含位置迁移）
│   ├── scanner.rs    安装目录发现
│   ├── buildinfo.rs  实测版本探测 + 缓存
│   ├── config.rs     配置文件
│   └── model.rs      数据结构
├── ui/             前端（原生 HTML/CSS/JS，无框架）
├── capabilities/   Tauri 2 权限声明
└── tauri.conf.json 窗口与打包配置
```

## 注意

- 只更新「便携版」布局的 Blender（顶层有 `blender.exe` 和 `portable/` 的那种）
- 更新前请关闭正在运行的 Blender，否则 exe 会被占用导致替换失败

## 许可证

MIT License，详见 [LICENSE](LICENSE)。
