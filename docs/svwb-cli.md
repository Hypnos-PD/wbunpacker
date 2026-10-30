# svwb CLI

当前命令：

```text
svwb doctor [--config path] [--json]
svwb profile validate <file> [--json]
svwb profile inspect <file>
svwb profile id <file>
svwb profile probe <file> --token-file <path> [--api-base URL] [--output FILE] [--json]
svwb profile import-dumper <runtime.json> --output <profile.json>
svwb profile detect --output <profile.json> [--engine python|native] [--memory-dump DIR] [--dumper PATH] [--game-root PATH] [--pid PID]
svwb profile publish <file> --token-file <path>
svwb profile list --operator <cygames|cy|netease|ne> --token-file <path>
svwb profile active --operator <cygames|cy|netease|ne> --token-file <path>
svwb profile activate <profile-id> --token-file <path>
svwb credential inspect --operator <cygames|cy|netease|ne> <path> [--json]
svwb probe validate <file> [--json]
```

## doctor

检查：

- `WBU_CONFIG` 或 `config/Config.local.toml`。
- `python3`。
- `ffmpeg`。
- `vgmstream-cli`。
- `AssetStudioModCLI`。
- WBU 配置中的资源目录、manifest 地址和解密字段。

配置字段缺失时退出码为 `1`。外部命令缺失记为警告。

## profile

`profile validate` 读取并校验 RuntimeProfile。`--json` 输出稳定 JSON。

`profile inspect` 校验成功后输出规范化 JSON。

`profile id` 输出 RuntimeProfile 的稳定内容指纹。

`profile probe` 将 profile 提交给 WBArts 执行 Cygames 轻量探测。JWT 文件权限必须排除 group/other。接口不修改 active profile。

`profile import-dumper` 读取 `svwb_dumper` 的 `runtime.json`，转换为 RuntimeProfile。

`profile detect` 当前仅支持 `operator=cygames`。

- 默认引擎为 `native`：Rust 进程发现、内存 dump、元数据提取和 CommonHeader 定位，调用 `dotnet` 运行本地 `Il2CppDumper.dll`。
- `--engine python` 保留为显式兼容选项，并强制 `--dry-run`。

原生模式需要 Linux、`GameAssembly.dll` 和 `Il2CppDumper.dll`。默认 `Il2CppDumper` 目录为 `~/.local/share/svwb_dumper/Il2CppDumper`。

`--memory-dump` 复用已有内存 dump，跳过 `/proc` 进程读取。原生模式的元数据和 Il2CppDumper 产物按 resource version 缓存；`--refresh-dump` 强制重建。

内存中没有 manifest URL 时，传 `--token-file` 让 WBArts 执行 `Version/info`。取得 `resource_version` 后用已知模板拼接 manifest URL。

`profile publish` 保存候选 Profile。`profile activate` 单独切换 active Profile。`profile list` 和 `profile active` 查询 WBArts 中的记录。

## credential

`credential inspect` 读取已有账号材料并输出脱敏摘要。Cygames 的 `<path>` 是 credential 目录；Netease 的 `<path>` 是 `svwb-cn-auth` 文件。

## probe

`probe validate` 校验 RuntimeProbe JSON。当前命令不执行线上探测。
