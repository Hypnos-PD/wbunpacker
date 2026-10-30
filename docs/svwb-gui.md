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
| Profile | 选择 Python adapter 或 Rust native 检测运行中 Cygames 客户端，生成并校验 RuntimeProfile |
| 账号材料 | 调用 `credential_inspect`，显示脱敏材料和指纹 |
| Runtime Probe | 调用 `probe_profile`，通过 WBArts 执行 Cygames 轻量探测 |

## 命令

```bash
cd apps/svwb-gui
npm ci
npm run build
npm run tauri dev
```

GUI 和 CLI 调用同一套 `svwb-core` 校验逻辑。
