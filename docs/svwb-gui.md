# SVWB Toolkit GUI

路径：`apps/svwb-gui`

## 技术

| 项目 | 实现 |
| --- | --- |
| 桌面壳 | Tauri 2 |
| 前端 | React、TypeScript、Vite |
| 共用核心 | `svwb-core` |

## 当前页面

| 页面 | 功能 |
| --- | --- |
| 环境检查 | 调用 `doctor_report`，显示配置和外部命令状态 |
| Profile | 检测运行中的 Cygames 客户端，或从 Netease 安装目录生成 RuntimeProfile；Netease 目录留空时自动查找 Sevo |
| 账号材料 | 调用 `credential_inspect`，显示脱敏材料和指纹 |
| Runtime Probe | 调用 `probe_profile`，通过 WBArts 执行 Cygames 或 Netease 轻量探测 |
| 数据管线 | 读取 `pipeline.toml`、校验清单、执行所选步骤的 Dry Run 或正式任务，并实时显示 stdout/stderr |

## 命令

```bash
cd apps/svwb-gui
npm ci
npm run build
npm run tauri dev
```

GUI 和 CLI 调用同一套 `svwb-core` 校验逻辑。

数据管线页面不会复制步骤定义。它调用 `svwb pipeline list/check/run`，
`pipeline.toml` 继续作为唯一执行清单和依赖图。GUI 中的 `svwb` 字段可填写可执行文件路径，
留空时读取 `SVWB_EXE`，再回退到 PATH 中的 `svwb`。
