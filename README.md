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

| 工具 | 版本 |
|------|------|
| [Node.js](https://nodejs.org/) | ≥ 18 |
| [Rust](https://www.rust-lang.org/tools/install) | ≥ 1.77 (stable) |
| [Tauri CLI](https://v2.tauri.app/start/prerequisites/) | v2 |

Windows 用户还需要：
- **Visual Studio Build Tools** (含 C++ 桌面开发工作负载)
- **WebView2** (Windows 10/11 通常已预装)

### 推荐 IDE

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

### 安装与运行

```bash
# 克隆仓库
git clone https://github.com/<your-org>/wuwa-echo-sight.git
cd wuwa-echo-sight

# 安装前端依赖
npm install

# 开发模式（热重载）
npm run tauri dev

# 生产构建
npm run tauri build
```

构建产物在 `src-tauri/target/release/bundle/` 目录下。

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
