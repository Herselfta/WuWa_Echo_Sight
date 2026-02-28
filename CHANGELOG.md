# Changelog

本文件基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 标准，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

---

## [Unreleased]

### Added
- **统一看板（RecordPage）**：新建声骸 + 强化录入 + 实时概率分析一站式工作流
- **声骸池管理（EchoPoolPage）**：声骸 CRUD、期望配置、补录/删除、拖拽排序
- **数据工具（AnalysisPage）**：概率快照、CSV 导出、ZIP 导入
- **期望词条链**：支持优先级排序（`>` 严格优先、`=` 同级），预设保存/加载
- **贝叶斯概率引擎**：Wilson 置信区间 + Beta 后验估计
- **全局分布图**：基于 ECharts 6 的交互式概率可视化
- **紧凑词条格式**：`1: B1=21%` 压缩显示，hover 查看完整信息
- **表单状态持久化**：新建声骸表单内容在页面切换后保留
- **SQLite 本地存储**：零服务端依赖，数据完全离线

### Infrastructure
- Tauri v2 + React 19 + TypeScript 5.8 + Vite 7 + Zustand 5
- Rust 后端：rusqlite (bundled)、statrs、chrono、uuid
- 自动数据库迁移系统（按文件名序号递增）

---

## [0.1.0] - TBD

首个公开版本。
