# AudioDown Core

AudioDown Core 第一阶段提供可独立运行的 Rust Core、受限 Supervisor、SQLite
状态、结构化日志、Vue 空状态界面，以及仅用于契约测试的虚拟 Node.js 插件。

## 启动

```bash
docker compose up -d --build
curl http://localhost:18080/healthz
```

默认仅 Core 暴露 `18080`。SQLite 和日志写入 `./data`，Docker Socket 只挂载给
Supervisor。

虚拟插件安装接口只在显式开发模式下启用：

```bash
export AUDIODOWN_DEV_MODE=1
export AUDIODOWN_DEV_TOKEN=local-fixture-token
docker compose up -d --build
./scripts/install-virtual-plugin.sh
```

运行完整验证：

```bash
./scripts/verify.sh
```

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

## 第一阶段边界

- 不包含任何真实平台插件或真实下载能力。
- 不需要激活码，也不存在授权心跳或设备绑定。
- 虚拟插件只用于 manifest、RPC、生命周期、日志和容器隔离的契约测试。
- GitHub 插件安装、凭据、搜索数据、下载和自动更新属于后续阶段。

设计规格见
[`docs/superpowers/specs/2026-07-11-audiodown-1-plugin-platform-design.md`](docs/superpowers/specs/2026-07-11-audiodown-1-plugin-platform-design.md)。
