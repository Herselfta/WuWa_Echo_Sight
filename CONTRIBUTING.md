# 贡献指南

感谢你对 WuWa Echo Sight 的关注！以下是参与贡献的基本流程和规范。

---

## 开始之前

1. 确保已阅读 [README.md](README.md) 并成功运行项目
2. 搜索现有 [Issues](../../issues) 确认问题未被报告或功能未被提议
3. 对于较大的变更，建议先开 Issue 讨论方案

---

## 开发流程

### 1. Fork & Clone

```bash
git clone https://github.com/<your-fork>/wuwa-echo-sight.git
cd wuwa-echo-sight
npm install
```

### 2. 创建功能分支

```bash
git checkout -b feat/your-feature
# 或
git checkout -b fix/your-bugfix
```

### 分支命名规范

| 前缀 | 用途 |
|------|------|
| `feat/` | 新功能 |
| `fix/` | 修复 Bug |
| `refactor/` | 重构（不改变功能） |
| `docs/` | 文档变更 |
| `chore/` | 构建工具/依赖/CI 变更 |

### 3. 开发 & 验证

```bash
# 启动开发服务器
npm run tauri dev

# 提交前务必通过以下检查
npx tsc --noEmit          # TypeScript 类型检查
npm run build              # 前端构建
cd src-tauri && cargo clippy  # Rust lint
cd src-tauri && cargo fmt --check  # Rust 格式
```

### 4. 提交

遵循 [Conventional Commits](https://www.conventionalcommits.org/) 规范：

```
<type>(<scope>): <subject>

[optional body]
```

**Type 类型：**

| Type | 说明 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `docs` | 文档 |
| `style` | 格式（不影响代码逻辑） |
| `refactor` | 重构 |
| `perf` | 性能优化 |
| `test` | 测试 |
| `chore` | 构建/工具/依赖 |

**Scope 范围（可选）：**
`frontend` · `backend` · `db` · `analysis` · `ui` · `store`

**示例：**
```
feat(frontend): add expectation preset naming in RecordPage
fix(backend): handle duplicate slot insertion gracefully
docs: update README with project structure
```

### 5. 提交 Pull Request

- 确保 PR 标题遵循 Conventional Commits 格式
- 在描述中说明变更内容和动机
- 如有 UI 变更，附上截图
- 关联相关 Issue（`Closes #123`）

---

## 代码规范

### TypeScript / React

- 使用函数式组件 + Hooks
- 状态管理统一使用 Zustand store（跨页持久化状态）或 `useState`（页面内临时状态）
- 类型定义集中在 `src/types/domain.ts`
- API 调用封装在 `src/api/tauri.ts`
- 一个页面对应一个 `src/pages/XxxPage.tsx` 文件

### Rust

- 遵循 `cargo fmt` 格式化标准
- 所有 Tauri command 返回 `Result<T, String>`
- 数据库操作位于 `src-tauri/src/commands/` 或 `src-tauri/src/db/`
- 新增表或字段通过迁移脚本（`src-tauri/src/db/migrations/`）实现

### CSS

- 全局样式统一在 `src/App.css`
- 采用 BEM-like 命名：`.record-page`、`.echo-table-card`、`.slot-pill-locked`
- 利用 CSS Custom Properties (已定义 `--muted` 等变量)

---

## 数据库迁移

如果你的变更涉及数据库结构：

1. 在 `src-tauri/src/db/migrations/` 目录新建迁移文件
2. 文件名格式：`003_description.sql`（序号递增）
3. 脚本应幂等——使用 `CREATE TABLE IF NOT EXISTS` 或 `ALTER TABLE ... ADD COLUMN` 等
4. 在 PR 中说明迁移内容

---

## 报告 Bug

提交 Bug Issue 时请包含：

1. **环境信息**：操作系统版本、应用版本
2. **复现步骤**：尽可能详细
3. **预期行为** vs **实际行为**
4. **截图/日志**（如有）

---

## 功能建议

提交 Feature Request 时请说明：

1. 你想解决什么问题
2. 你设想的解决方案
3. 是否有替代方案
4. 该功能对其他用户是否也有价值

---

## 许可

提交贡献即表示你同意你的代码以 [MIT License](LICENSE) 发布。
