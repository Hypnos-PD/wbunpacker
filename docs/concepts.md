# SVWB 数据概念

## 运营

新契约使用运营区分服务器：

| 正式值 | 输入别名 | 旧值 |
| --- | --- | --- |
| `cygames` | `cy` | `intl` |
| `netease` | `ne` | `cn` |

`intl`、`cn` 只允许出现在旧 API、旧数据库和适配层。

## 版本

| 字段 | 定义 |
| --- | --- |
| `game_build` | 客户端安装构建。主要标识是 `GameAssembly.dll` 的 SHA-256，可附带 exe 和 Unity 版本。 |
| `metadata_sha256` | IL2CPP 元数据修复后的 SHA-256。仅适用可提取该元数据的客户端。 |
| `resource_version` | 游戏资源清单版本。由 manifest 路径或 `Version/info` 返回。 |
| `app_version` | 请求头 `App-Ver` 的值。 |
| `schema_version` | JSON 或 API 结构版本。 |
| `release_id` / `data_version` | 一组发布文件的内容哈希。 |

## 协议

| 字段 | 定义 |
| --- | --- |
| `protocol.family` | 协议实现。当前为 `cygames_v1`、`netease_v1`。 |
| `codec_version` | 请求包和响应包的编解码实现版本。 |
| `routing_header` | Cygames 客户端的 `Routing-Header`。 |
| `common_header` | Cygames 客户端请求包的 52 字节常量。不是账号凭据。 |
| `runtime_profile` | 与账号无关的客户端运行配置快照。 |
| `runtime_probe` | 使用一套运行配置执行轻量请求后的结果。 |

## 账号

| 字段 | 定义 |
| --- | --- |
| `wba_user` | WBArts 邮箱账号、JWT 和角色。 |
| `game_credential` | 游戏账号材料。Cygames 使用 ClientID、UUID、AuthKey；Netease 使用 `svwb-cn-auth`。 |
| `device_session` | WBCapture、WBO 等桌面客户端通过 WBArts 设备授权取得的受限会话。 |

`wba_user`、`game_credential`、`device_session` 是三类独立身份，不能互相替代。

## CR

| 字段 | 定义 |
| --- | --- |
| `rank_snapshot` | 游戏排行榜 Top 100 快照。 |
| `match_rating` | 单局回放中的 CR 前值、后值和变化量。 |
| `cr_report` | 根据 `rank_snapshot` 生成的报告。 |

## 模式与时间

| 字段 | 定义 |
| --- | --- |
| `rotation` | 公开产品中的指定模式。旧 `limited` 通过适配器映射。 |
| `unlimited` | 无限模式。 |
| `captured_at` | 实际取得数据的时间。 |
| `snapshot_at` | 数据归属的固定采集时点。 |
| `game_date` | 按游戏服务器时区计算的日期。Cygames 使用 UTC+9，Netease 使用 UTC+8。 |
