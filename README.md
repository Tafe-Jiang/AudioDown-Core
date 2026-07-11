# AudioDown Core

AudioDown Core 提供可独立运行的 Rust Core、受限 Supervisor、SQLite 状态、
结构化日志、Vue 管理界面，以及安全隔离的 Node.js 插件运行时。核心仓库保持
空核心，不包含任何真实平台实现。

## 启动

```bash
docker compose up -d --build
curl http://localhost:18080/healthz
```

默认仅 Core 暴露 `18080`。SQLite 和日志写入 `./data`，Docker Socket 只挂载给
Supervisor。

完整验证：

```bash
./scripts/verify.sh
```

## 插件仓库

用户可以在插件页输入 GitHub 公共仓库地址。Core 先解析默认分支并锁定一个
不可变 Commit SHA，再校验仓库索引、插件 manifest、`package.json` 和
`package-lock.json`。插件构建使用 AudioDown 固定的 Node.js 22 构建与运行时
镜像，仓库不能提供 Dockerfile、命令、挂载或网络策略。

默认安装命令为 `npm ci --omit=dev --ignore-scripts`。只有在开发者模式下，
用户针对当前 Commit 明确勾选风险授权并提供开发者令牌，才允许 npm 生命周期
脚本执行。授权不会跟随分支或后续 Commit。

## 界面

- 五个操作路由：Discover（发现）、Search（搜索）、Plugins（插件）、
  Logs（日志）、System（系统）。
- 桌面端使用可折叠导航，移动端使用导航 Sheet。
- MCP 选定的模式以 Vue/Reka UI 实现，不复制 React registry 代码。
- 单元测试、类型检查和构建：

  ```bash
  docker run --rm -v "$(pwd)/web:/app" -w /app node:22-bookworm-slim \
    sh -lc 'npm ci && npm test -- --run && npm run typecheck && npm run build'
  ```

- 固定 Playwright 验证：

  ```bash
  docker run --rm --ipc=host -v "$(pwd)/web:/app" -w /app \
    mcr.microsoft.com/playwright:v1.61.1-noble \
    sh -lc 'npm ci && npx playwright test'
  ```

## 当前边界

- 不支持私有仓库或 GitHub Token。
- 不自动检查或安装插件更新。
- 不包含任何真实平台插件或真实下载能力。
- 不包含凭据、Cookie 或登录处理。
- 不提供搜索、发现、专辑或曲目数据。
- 不提供下载、整理归档、格式转换或后处理。
- 虚拟插件和虚拟仓库只用于 manifest、构建、RPC、生命周期、日志和容器隔离
  的契约测试。

设计规格见
[`docs/superpowers/specs/2026-07-11-audiodown-1-plugin-platform-design.md`](docs/superpowers/specs/2026-07-11-audiodown-1-plugin-platform-design.md)。
