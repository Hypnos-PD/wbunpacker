# Profile Detector

`svwb profile detect` 执行：

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

检测器仅支持 Linux。

## 对照基线

同一份 `762956809150` 内存 dump 曾用于对照 Python 适配器和 Rust 原生实现。移除 Python 适配器前，两边输出以下字段一致：

- operator
- resource version
- App-Ver
- RoutingHeader
- CommonHeader
- metadata SHA-256
- GameAssembly SHA-256
- manifest SHA-256
- profile ID

Profile ID：`sha256:2a34e15aac7e150f07c7f3922b9c5d07c307f134ea958ede2ee5dd8f69d5b00a`
