# WBU Catalog Release

CatalogRelease 是 WBArts `data/` 目录的不可变内容快照。

## 范围

纳入：

- `data/**` 中的 JSON、图片、音频、Spine、纹理和其他站点资源。

排除：

- `data/wbcapture/**`。
- HTML、JavaScript、CSS。
- 排行榜、CR、Meta 等数据库动态数据。
- 账号材料、RuntimeProfile 和构建缓存。
- `.DS_Store`、`Thumbs.db` 和符号链接。

## release_id

文件按路径进行 Unicode 字节序排序。`release_id` 输入为：

```text
wbu-catalog-release-v1
<path>\t<size>\t<sha256>
...
```

每行以 LF 分隔。对 UTF-8 字节流计算 SHA-256，输出：

```text
sha256:<64 lowercase hex>
```

`generated_at`、`producer` 和 `sources` 不参与 `release_id`。

## 命令

只计算并输出摘要：

```bash
svwb catalog build --root ../WBArts --dry-run
```

生成 manifest：

```bash
svwb catalog build \
  --root ../WBArts \
  --output artifacts/catalog-release/manifest.json
```

manifest 必须写到 `data/` 目录之外。

校验现有 manifest：

```bash
svwb catalog verify \
  --root ../WBArts \
  --manifest artifacts/catalog-release/manifest.json
```

比较两个 manifest：

```bash
svwb catalog diff \
  --old old-manifest.json \
  --new new-manifest.json
```

`--json` 输出稳定 JSON。

## sources

`sources` 通过 JSON 文件提供：

```json
[
  {
    "kind": "runtime_profile",
    "name": "cygames",
    "reference": "sha256:..."
  }
]
```

允许的 `kind`：

- `game_resources`
- `runtime_profile`
- `official_api`
- `manual`
- `generated`

```bash
svwb catalog build \
  --root ../WBArts \
  --source-file sources.json \
  --output artifacts/catalog-release/manifest.json
```
