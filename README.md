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

## 第一阶段边界

- 不包含任何真实平台插件或真实下载能力。
- 不需要激活码，也不存在授权心跳或设备绑定。
- 虚拟插件只用于 manifest、RPC、生命周期、日志和容器隔离的契约测试。
- GitHub 插件安装、凭据、搜索数据、下载和自动更新属于后续阶段。

设计规格见
[`docs/superpowers/specs/2026-07-11-audiodown-1-plugin-platform-design.md`](docs/superpowers/specs/2026-07-11-audiodown-1-plugin-platform-design.md)。
