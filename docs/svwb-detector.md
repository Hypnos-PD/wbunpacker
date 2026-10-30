# Profile Detector

## 当前实现

`svwb profile detect` 调用：

```text
python3 <dumper>/scripts/sync_runtime.py --dry-run --json-out <temp>
```

适配器输入：

- 游戏安装目录。
- 数据缓存目录。
- 游戏进程 PID。
- manifest 语言变体。

适配器输出：

- 原始 `runtime.json`。
- 转换后的 RuntimeProfile。

命令不传递 `--write-wbu`、`--apply-server` 或 `--apply-server-ssh`。

## 替换边界

CLI 和 GUI 只依赖 `detect_with_python_dumper` 的输入和 RuntimeProfile 输出。

Rust 原生实现接入时保持以下命令不变：

```text
svwb profile detect --output <profile.json>
```

原生实现替换 `PythonDumperOptions` 调用，不改变 RuntimeProfile 契约。

## 原生实现

```text
svwb profile detect --engine native --output <profile.json>
```

原生模式执行：

1. `/proc` 进程发现和内存区域读取。
2. manifest URL 与 `App-Ver` 搜索。
3. IL2CPP 元数据定位、头部修复和 SHA-256 计算。
4. CommonHeader 定位。
5. 调用 `dotnet Il2CppDumper.dll` 生成 `stringliteral.json`。
6. RoutingHeader 筛选。
7. manifest 下载和 RuntimeProfile 生成。

未下载游戏内容时，内存中可能没有 manifest URL。此时检测继续执行，并生成不含 `resource_version` 的内部 Draft。传入 `--token-file` 后：

```text
Draft
  -> WBArts Title/index + Version/info
  -> resource_version
  -> manifest URL（已知模板拼接）
  -> manifest 下载
  -> RuntimeProfile
```

Cygames manifest URL：

```text
https://shadowverse-wb.akamaized.net/dl/manifests/Windows/{resource_version}/assetbundle.{variant}.manifest
```

Jpn 变体使用 `assetbundle.manifest`，不带语言段。`resource_version=00000000` 不会拼接 URL。

原生模式当前只支持 Linux，并已作为 `profile detect` 默认引擎。Python 模式保留为显式兼容选项。

## 对照结果

使用同一份 `762956809150` 内存 dump 时，Python 与 Rust native 输出一致：

- operator
- resource version
- App-Ver
- RoutingHeader
- CommonHeader
- metadata SHA-256
- GameAssembly SHA-256
- manifest SHA-256
- profile ID

Python profile ID：`sha256:2a34e15aac7e150f07c7f3922b9c5d07c307f134ea958ede2ee5dd8f69d5b00a`

Rust native profile ID：`sha256:2a34e15aac7e150f07c7f3922b9c5d07c307f134ea958ede2ee5dd8f69d5b00a`
