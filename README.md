# WuWa Echo Sight

> 鸣潮（Wuthering Waves）声骸副词条强化追踪与概率分析桌面工具

[![Tauri v2](https://img.shields.io/badge/Tauri-v2-blue?logo=tauri)](https://v2.tauri.app/)
[![React 19](https://img.shields.io/badge/React-19-61dafb?logo=react)](https://react.dev/)
[![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

---

## 这是什么？

WuWa Echo Sight 是一款面向《鸣潮》玩家的**本地桌面应用**，用于：

- **记录**每次声骸强化出词条的结果（词条类型、档位、数值）
- **追踪**每颗声骸的完整强化历程
- **分析**副词条的实际出现概率，与理论值对比
- **管理**声骸池中所有声骸的期望配置与状态

所有数据全部存储在本地 SQLite 数据库，不上传任何信息。

---

## 功能概览

| 页面 | 功能 |
|------|------|
| **统一看板** | 新建声骸 → 录入强化 → 实时概率图，一站式工作流 |
| **声骸池管理** | 批量管理声骸资料、期望预设、补录/删除误录 |
| **数据工具** | 概率快照、CSV 导出、ZIP 数据导入、历史事件修正 |

### 核心特性

- **期望词条链**：支持优先级排序（`暴击率 > 暴击伤害 = 攻击%`），自动计算下一槽位命中概率
- **贝叶斯分析**：基于历史数据的 Wilson 置信区间与贝叶斯后验估计
- **预设系统**：保存/复用常用期望配置，快速应用到新声骸
- **实时联动**：录入即刷新概率图、分布表、命中列表

### 分析上下文基线

为避免后续讨论缺少上下文，已整理声骸系统关键规则：

- [声骸系统规则基线](docs/echo-system-baseline.md)

> 该文档用于统一讨论基线与策略评估口径，不代表官方公告。

---

## 技术栈

| 层 | 技术 |
|----|------|
| 前端 | React 19 + TypeScript 5.8 + Vite 7 + Zustand 5 |
| 后端 | Rust (Edition 2021) + Tauri v2 |
| 数据库 | SQLite (rusqlite, bundled) |
| 统计 | statrs (Rust 端概率计算) |
| 图表 | ECharts 6 |
| 打包 | Tauri bundler (NSIS / WiX / DMG) |

---

## 快速开始

### 环境要求

| 工具 | 最低版本 | 推荐版本 | 验证命令 |
|------|----------|----------|----------|
| [Node.js](https://nodejs.org/) | 18 | 20 LTS / 22 LTS | `node -v` |
| [npm](https://www.npmjs.com/) | 9 | 10+ (随 Node.js 安装) | `npm -v` |
| [Rust](https://www.rust-lang.org/tools/install) | 1.77 | latest stable | `rustc --version` |
| [Cargo](https://doc.rust-lang.org/cargo/) | 跟随 Rust | — | `cargo --version` |
| [Git](https://git-scm.com/) | 2.x | latest | `git --version` |

---

### 平台特定依赖

<details>
<summary><b>🪟 Windows（主要支持平台）</b></summary>

#### 1. Visual Studio Build Tools

Rust 编译器需要 MSVC 链接器。安装 [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) 或完整 Visual Studio，**必须勾选以下工作负载**：

- ✅ **使用 C++ 的桌面开发** (Desktop development with C++)
  - 其中包含的 MSVC 编译器和 Windows SDK 是必需项

验证安装：
```powershell
# 应输出 MSVC 编译器路径
where cl
```

#### 2. WebView2 Runtime

Tauri 在 Windows 上使用 WebView2 作为渲染引擎。

- **Windows 11**：已预装，无需操作
- **Windows 10 (1803+)**：通常已通过系统更新安装；若未安装请从 [Microsoft 官网](https://developer.microsoft.com/microsoft-edge/webview2/) 下载 Evergreen Bootstrapper
- **Windows 10 LTSC / Server**：需手动安装上述 Runtime

验证安装：
```powershell
# 若输出版本号则说明已安装
Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -Name pv -ErrorAction SilentlyContinue | Select-Object -ExpandProperty pv
```

#### 3. Rust 默认工具链

确保 Rust 工具链目标为 MSVC：
```powershell
rustup default stable-x86_64-pc-windows-msvc
```

</details>

<details>
<summary><b>🍎 macOS</b></summary>

```bash
# 安装 Xcode 命令行工具（提供 clang 编译器）
xcode-select --install

# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

macOS 11 (Big Sur) 及以上版本内置 WebView (WKWebView)，无需额外安装。

</details>

<details>
<summary><b>🐧 Linux</b></summary>

以 Ubuntu / Debian 为例：
```bash
# 系统依赖
sudo apt update
sudo apt install -y \
  build-essential \
  libwebkit2gtk-4.1-dev \
  libssl-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  wget \
  curl

# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

其他发行版请参考 [Tauri v2 Prerequisites](https://v2.tauri.app/start/prerequisites/#linux)。

</details>

---

### 推荐 IDE

| IDE | 插件 |
|-----|------|
| [VS Code](https://code.visualstudio.com/) | [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) · [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer) · [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml) |
| [RustRover](https://www.jetbrains.com/rust/) | 内置 Rust 支持 |

---

### 从源码构建

#### 1. 克隆仓库

```bash
git clone https://github.com/<your-org>/wuwa-echo-sight.git
cd wuwa-echo-sight
```

#### 2. 安装前端依赖

```bash
npm install
```

> 首次安装耗时约 1-2 分钟，依赖包含 React 19、ECharts 6、Zustand 5 等。  
> 若网络不佳可设置镜像：`npm config set registry https://registry.npmmirror.com`

#### 3. 安装 Tauri CLI（若未全局安装）

Tauri CLI 已在 `devDependencies` 中声明，`npm install` 后可直接通过 `npx tauri` 调用。若需全局安装：

```bash
npm install -g @tauri-apps/cli@^2
```

#### 4. 首次编译 Rust 后端

首次编译会自动下载并构建所有 Rust 依赖（rusqlite bundled SQLite、statrs 等），耗时约 3-8 分钟，取决于机器性能。后续增量编译通常在数秒以内。

```bash
# 仅编译后端（可选，用于验证 Rust 工具链是否正常）
cd src-tauri && cargo check && cd ..
```

---

### 开发模式

```bash
npm run tauri dev
```

此命令会同时启动：
1. **Vite 开发服务器**（`http://localhost:1420`）— 前端热重载 (HMR)
2. **Tauri 原生窗口** — 自动加载开发服务器页面

> 修改 `src/` 下的前端代码后浏览器即时刷新；修改 `src-tauri/src/` 下的 Rust 代码后自动重新编译并重启窗口。

---

### 生产构建

```bash
npm run tauri build
```

构建流程：
1. `npm run build` — TypeScript 类型检查 + Vite 生产打包 → `dist/`
2. `cargo build --release` — Rust 后端 Release 编译
3. Tauri bundler — 将前端资源嵌入二进制，生成安装包

#### 构建产物位置

```
src-tauri/target/release/
├── WuWa Echo Sight.exe          # 裸可执行文件
└── bundle/
    ├── nsis/                    # Windows NSIS 安装程序 (.exe)
    │   └── WuWa Echo Sight_0.1.0_x64-setup.exe
    └── msi/                     # Windows MSI 安装包 (可选, 需 WiX)
        └── WuWa Echo Sight_0.1.0_x64_en-US.msi
```

| 平台 | 默认安装包格式 | 备注 |
|------|--------------|------|
| Windows | `.exe` (NSIS) | 双击即装，自动安装 WebView2 |
| macOS | `.dmg` / `.app` | 需要 Apple 签名才能分发 |
| Linux | `.deb` / `.AppImage` | AppImage 无需安装即可运行 |

#### 构建优化选项

在 `src-tauri/Cargo.toml` 中可调整 Release 编译参数：

```toml
[profile.release]
strip = true       # 移除调试符号，减小二进制体积
lto = true          # 链接时优化，提升运行性能（增加编译时间）
opt-level = "s"     # 优化体积 ("s") 或性能 ("3")
codegen-units = 1   # 单线程代码生成，更好的优化
```

---

### 分发与部署

#### 方式一：直接分发安装包

将 `bundle/nsis/` 目录下的 `.exe` 文件发送给用户，双击安装即可。

- 安装程序会自动检测并安装 WebView2 Runtime（若缺失）
- 默认安装路径：`C:\Users\<用户名>\AppData\Local\WuWa Echo Sight\`
- 应用数据库路径：`%APPDATA%\com.mac.wuwa-echo-sight\wuwa_echo_sight.sqlite3`

#### 方式二：便携模式（Portable）

若不想安装，可直接运行裸二进制 `src-tauri/target/release/WuWa Echo Sight.exe`：

- 数据库仍存储在系统 AppData 目录
- 无开始菜单 / 桌面快捷方式
- 适合测试或开发者使用

#### 方式三：GitHub Releases 自动发布

推荐配合 GitHub Actions 实现 CI/CD 自动构建与发布：

```yaml
# .github/workflows/release.yml (示例框架)
name: Release
on:
  push:
    tags: ['v*']
jobs:
  build:
    strategy:
      matrix:
        os: [windows-latest, macos-latest, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - uses: dtolnay/rust-toolchain@stable
      - run: npm install
      - run: npm run tauri build
      - uses: softprops/action-gh-release@v2
        with:
          files: src-tauri/target/release/bundle/**/*
```

---

### 常见问题

<details>
<summary><code>error: linker 'link.exe' not found</code></summary>

未安装 Visual Studio Build Tools 或未选中 C++ 桌面开发工作负载。请参阅上方 [Windows 平台依赖](#平台特定依赖)。

</details>

<details>
<summary><code>Error: failed to get cargo metadata</code></summary>

Rust 工具链未正确安装。运行：
```bash
rustup update stable
rustup default stable
```

</details>

<details>
<summary>`npm run tauri dev` 启动后只有空白窗口</summary>

1. 检查 Vite 开发服务器是否正常启动（终端是否输出 `Local: http://localhost:1420/`）
2. 确认端口 1420 未被占用：
   ```powershell
   netstat -ano | findstr :1420
   ```
3. 若使用代理软件，尝试关闭或将 `localhost` 加入直连列表

</details>

<details>
<summary>构建时 `rusqlite` 编译失败</summary>

本项目使用 `rusqlite` 的 `bundled` 特性，会自行编译 SQLite C 代码，因此需要 C 编译器。  
- **Windows**：确保安装了 MSVC Build Tools  
- **Linux**：确保安装了 `build-essential`  
- **macOS**：运行 `xcode-select --install`

</details>

<details>
<summary>Windows 上构建产物体积过大</summary>

在 `src-tauri/Cargo.toml` 中添加 Release 优化参数（见上方 [构建优化选项](#构建优化选项)），Release 二进制通常可从 ~15 MB 压缩到 ~8 MB。

</details>

<details>
<summary>如何更换应用图标</summary>

替换 `src-tauri/icons/` 目录下的图标文件，保持相同文件名和尺寸。可使用 Tauri 官方工具自动生成全套尺寸：

```bash
npx @tauri-apps/cli icon path/to/your-icon.png
```

</details>

---

## 项目结构

```
wuwa-echo-sight/
├── src/                          # 前端 (React + TypeScript)
│   ├── api/tauri.ts              #   Tauri invoke 封装
│   ├── components/               #   通用组件 (BarChart 等)
│   ├── pages/                    #   页面组件
│   │   ├── RecordPage.tsx        #     统一看板
│   │   ├── EchoPoolPage.tsx      #     声骸池管理
│   │   └── AnalysisPage.tsx      #     数据工具
│   ├── store/useAppStore.ts      #   Zustand 全局状态
│   └── types/domain.ts           #   TypeScript 类型定义
├── src-tauri/                    # 后端 (Rust + Tauri)
│   ├── src/
│   │   ├── lib.rs                #   Tauri 入口 & Command 注册
│   │   ├── commands/             #   IPC 命令实现
│   │   │   ├── echo.rs           #     声骸 CRUD、预设、回填
│   │   │   ├── event.rs          #     强化事件录入/编辑/查询
│   │   │   ├── analysis.rs       #     概率分布 & 命中计算
│   │   │   └── export.rs         #     CSV 导出 & ZIP 导入
│   │   ├── db/                   #   数据库初始化 & 迁移
│   │   │   └── migrations/       #     SQL 迁移脚本
│   │   ├── domain/               #   领域类型
│   │   └── analysis/             #   统计算法模块
│   └── Cargo.toml
├── package.json
├── vite.config.ts
└── tsconfig.json
```

---

## 数据存储

应用数据存储在系统 AppData 目录下：

| 系统 | 路径 |
|------|------|
| Windows | `%APPDATA%\com.mac.wuwa-echo-sight\wuwa_echo_sight.sqlite3` |
| macOS | `~/Library/Application Support/com.mac.wuwa-echo-sight/` |
| Linux | `~/.local/share/com.mac.wuwa-echo-sight/` |

数据库结构通过 `src-tauri/src/db/migrations/` 下的 SQL 脚本自动迁移。

---

## 开发指南

### 常用命令

```bash
# TypeScript 类型检查 (不构建)
npx tsc --noEmit

# 仅构建前端
npm run build

# Vite 预览 (构建后)
npm run preview

# Rust 格式检查
cd src-tauri && cargo fmt --check

# Rust lint
cd src-tauri && cargo clippy
```

### 添加新的 Tauri 命令

1. 在 `src-tauri/src/commands/` 对应模块中添加 `#[tauri::command]` 函数
2. 在 `src-tauri/src/lib.rs` 的 `generate_handler![]` 中注册
3. 在 `src/api/tauri.ts` 中添加对应的 `invoke` 封装
4. 在 `src/types/domain.ts` 中定义相关类型

### 数据库迁移

在 `src-tauri/src/db/migrations/` 下新建 `003_xxx.sql`，脚本会在应用启动时按文件名顺序自动执行。

---

## 参与贡献

欢迎提交 Issue 和 Pull Request！请先阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。

---

## 路线图

详见 [ROADMAP.md](ROADMAP.md) 了解未来发展方向。

---

## 许可证

本项目基于 [MIT License](LICENSE) 开源。
