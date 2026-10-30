# 账号材料

## 类型

| 运营 | 来源 | 必需材料 |
| --- | --- | --- |
| `cygames` | systemd credentials 目录或本地安全存储 | `svwb-client-id`、`svwb-uuid`、`svwb-auth-key` |
| `netease` | `svwb-cn-auth` JSON | 当前 `ChinaAuth` schema v1 的 18 个字段 |

## 显示规则

`svwb credential inspect` 只输出：

- 运营。
- 材料类型。
- 整包 SHA-256 指纹。
- 文件是否存在、可读、权限是否仅限当前用户。
- 结构校验问题。

不输出 ClientID、UUID、AuthKey、UDID、session ID、SDK 返回值和密钥。

## 使用规则

- Cygames 三个文件按 systemd credential 文件读取。
- Netease 文件按 JSON 读取。
- RuntimeProfile 不复制账号材料。
- RuntimeProbe 和排行榜采集使用同一账号锁。
- 当前版本只检查已有材料，不写入系统钥匙串，也不自动生成 Netease 材料。
