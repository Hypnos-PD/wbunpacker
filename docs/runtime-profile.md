# RuntimeProfile

Schema：`contracts/runtime-profile.schema.json`

## 顶层字段

| 字段 | 必需 | 说明 |
| --- | --- | --- |
| `schema_version` | 是 | 当前为 `1`。 |
| `artifact_type` | 是 | 固定为 `runtime_profile`。 |
| `operator` | 是 | `cygames` 或 `netease`。 |
| `game_build` | 是 | 客户端构建信息。 |
| `runtime` | 是 | 服务器地址、资源版本、App-Ver。 |
| `protocol` | 是 | 协议族和协议常量。 |
| `client_headers` | 否 | 协议请求使用的客户端头字段。 |
| `manifest_source` | 否 | 资源 manifest 来源和校验信息。 |
| `provenance` | 是 | 生产者、工具版本和生成时间。 |

## Cygames

`operator` 为 `cygames` 时：

- `protocol.family` 必须是 `cygames_v1`。
- `protocol.routing_header` 必需。
- `protocol.common_header` 必需，值为 104 位十六进制文本。

## Netease

`operator` 为 `netease` 时：

- `protocol.family` 必须是 `netease_v1`。
- `protocol.routing_header` 和 `protocol.common_header` 不允许出现。
- 国服账号字段不进入 RuntimeProfile。

## 账号材料

RuntimeProfile 不包含以下字段：

- ClientID
- UUID
- AuthKey
- `http_udid`
- `short_udid`
- `local_uuid`
- `anti_cheat_value`
- `secret_key`
- `sdk_request`

账号材料由独立 credential provider 保存。RuntimeProbe 返回结果时只引用 profile 和账号指纹，不输出账号材料。

## 校验

`svwb profile validate <file>` 检查：

- JSON 字段和类型。
- `operator` 与 `protocol.family` 一致。
- URL 使用 HTTPS。
- SHA-256 为 64 位小写十六进制。
- `resource_version` 只含数字且不全为零。
- `app_version` 为一至四段数字。
- `generated_at` 为 RFC3339。

## 探测流程

Cygames：

```text
读取 credential
  -> 读取 RuntimeProfile
  -> 执行 Title/index
  -> 执行 Version/info
  -> 输出 RuntimeProbe
  -> 解除账号锁
```

`request_scope` 为 `title_version`。`RuntimeProbe` 与 `rank collect` 使用同一账号锁。

Netease：

```text
读取 RuntimeProfile
  -> GET serverlist_wizard2
  -> 取正式服、all/all、isDebug=0 条目的 resVer
  -> GET pc_serverlist_game
  -> 输出 RuntimeProbe
```

`request_scope` 为 `server_list`。该流程不使用游戏账号材料，也不获取账号锁。

## 发布流程

```text
创建候选
  -> svwb_runtime_profiles

执行 Probe
  -> RuntimeProbe

激活候选
  -> svwb_active_runtime_profiles
  -> svwb_runtime_profile_activations
```

发布和激活是两个管理接口。Probe 成功后不会自动激活。

## profile_id

`profile_id` 是以下字段紧凑 JSON 数组的 SHA-256，输出格式为 `sha256:<hex>`：

```text
[
  "svwb-runtime-profile-v1",
  schema_version,
  operator,
  gameassembly_sha256,
  exe_version,
  unity_version,
  metadata_sha256,
  base_url,
  resource_version,
  app_version,
  protocol.family,
  protocol.codec_version,
  routing_header,
  common_header,
  client_headers sorted by key
]
```

`manifest_source`、`provenance.generated_at` 和生产者版本不参与指纹。
