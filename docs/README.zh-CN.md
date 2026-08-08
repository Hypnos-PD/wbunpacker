# wbunpacker (wbu)

[English version](../README.md)

《Shadowverse: Worlds Beyond》（影之诗：超凡世界）资源提取与解包工具 —— 从 CDN 下载、解密、解析并导出游戏资源。

## 功能

- **Manifest** — 下载并解析资源清单；对比两个版本追踪资源变更
- **Asset** — 并发下载、XOR 解密、基于 blob 存储去重的 Unity AssetBundle 提取
- **Master Data** — 导出 173 张 MasterMemory 数据表为 JSON；生成派生数据（卡牌、卡包、徽章等）
- **Audio** — 从 AKPK 容器中提取 Wwise WEM，解码为 WAV（vgmstream），转码为 MP3（ffmpeg）；提取卡牌与主战者语音
- **Texture** — 提取卡面（848×1024）、卡包图标、卡框、纹章、徽章、印章、主界面插图；叠层渲染完整卡牌图
- **Spine 动画** — 提取主界面插图（HomeIllustration）与主战者皮肤（LeaderSkin）的 Spine 骨骼动画
- **MetaDB** — 通过动态 DLL 加载解密客户端 `meta.db`（SQLite3MC）

## 环境要求

- **Rust** nightly（edition 2024）
- [AssetStudioModCLI](https://github.com/aelurum/AssetStudio) — Unity AssetBundle 资源提取
- [vgmstream-cli](https://github.com/vgmstream/vgmstream) — WEM 解码为 WAV
- [ffmpeg](https://ffmpeg.org/) — WAV 转 MP3（可选）
- 游戏安装目录下的 `libnative.dll` — MetaDB 解密（可选）

## 配置

```bash
git clone <repo-url>
cd wbunpacker

# 复制并编辑配置
cp config/config.example.toml config/Config.local.toml
```

填写 `config/Config.local.toml`：

| 配置项 | 说明 |
|--------|------|
| `data_dir` | 数据输出根目录 |
| `default_version` | 资源版本号（从 CDN 地址获取） |
| `asset_bundle_base_keys` | AssetBundle 解密基础密钥（base64） |
| `asset_studio_path` | AssetStudioModCLI 可执行文件路径 |
| `vgmstream_path` | vgmstream-cli 路径 |
| `ffmpeg_path` | ffmpeg 路径 |
| `manifest_address` | Manifest CDN 地址模板 |
| `asset_bundle_address` | AssetBundle CDN 地址模板 |

也可通过 `WBU_CONFIG` 环境变量指定自定义配置文件路径。

## 编译

```bash
cargo build --release
# 二进制文件: target/release/wbu
```

## 用法

### 清单 (Manifest)

```bash
wbu manifest -v Chs                      # 下载并解析清单（简体中文）
wbu manifest -v Eng --format json        # 导出为 JSON
wbu manifest diff -o old_rev -n new_rev  # 对比两个版本
wbu manifest diff -n latest -t 30        # 与仓库中最新版本对比，显示前 30 项变更
```

### 资源 (Asset)

```bash
wbu asset download <name> -v Chs         # 按名称下载单个 AssetBundle
wbu asset decrypt <file>                 # 解密单个 .ab 文件
wbu asset batch -v Chs                   # 批量下载某语言变体的全部资源
wbu asset batch -v Chs -c 16             # 16 并发下载
wbu asset batch -v Chs --diff            # 仅下载差异部分
wbu asset batch -v Chs --diff --extract  # 差异下载并自动提取
```

### 主数据 (Master Data)

```bash
wbu master -v all                    # 导出全部 173 张表为 JSON
wbu master cards                     # 生成 cards_full.json（卡牌合并数据）
wbu master packs                     # 生成 pack_names.json
wbu master emblems                   # 生成 emblems_full.json
wbu master crests                    # 生成 crests_full.json
wbu master stamps                    # 生成 stamps_full.json
```

### 音频 (Audio)

```bash
wbu audio                            # 构建 Wwise 映射 + 提取 AKPK → WEM → WAV
wbu audio --mp3                      # 同时转码 WAV 为 MP3
wbu audio card                       # 提取卡牌语音（MP3 + voice_index.json）
wbu audio card -F                    # 强制覆盖已存在文件
wbu audio leader-skin                # 提取主战者细节语音
```

### 贴图与渲染 (Texture & Render)

```bash
wbu texture card                     # 导出卡面贴图（848×1024）
wbu texture pack-icons               # 提取卡包图标
wbu texture card-frames              # 提取 Card2D 卡框（PNG）
wbu texture crests                   # 提取纹章图标
wbu texture emblems                  # 提取徽章贴图
wbu texture stamps                   # 提取印章贴图
wbu texture home-illust-picts        # 提取主界面插图静态图
wbu render cards                     # 批量渲染完整卡牌图
wbu render card --id 100101          # 渲染单张卡牌
```

### Spine 动画

```bash
wbu home-illust                      # 提取全部主界面插图 Spine 动画
wbu home-illust --voices             # 同时复制语音文件
wbu leader-skin -v Chs               # 提取主战者皮肤 Spine 动画（中文名称）
```

### 元数据库 (MetaDB)

```bash
wbu metadb meta.db -o meta_decrypted.db --dll ./libnative.dll
```

## 典型工作流

```bash
wbu manifest -v Chs --format json     # 1. 下载并解析清单
wbu asset batch -v Chs                # 2. 下载全部 AssetBundle
wbu master -v all                     # 3. 导出主数据表
wbu master cards                      # 4. 生成卡牌数据
wbu audio                             # 5. 提取音频
wbu texture card                      # 6. 提取卡面贴图
wbu texture card-frames               # 7. 提取卡框贴图
wbu render cards                      # 8. 渲染完整卡牌图
```

## 输出目录结构

```
<data_dir>/
├── manifests/                          # 下载的清单 .raw 文件
├── manifest-json/                      # 解析后的清单 JSON
├── manifest-diffs/                     # 差异对比输出 JSON
├── blobs/                              # 原始 AssetBundle（按哈希存储，去重）
├── variants/<variant>/                 # 指向 blobs 的硬链接（按语言变体）
├── audio/
│   ├── WwiseIdMapping/                 # 解密的 Wwise 事件映射
│   ├── akpk/                           # 提取的 AKPK 容器
│   ├── wem/                            # 提取的 WEM 音频块
│   └── wav/                            # 解码后的 WAV 文件
├── master/                             # MasterMemory 数据表（JSON）
├── derived/                            # 派生数据（cards_full.json 等）
├── textures/                           # 提取的卡面、图标、卡框
├── homeillust/                         # 主界面插图 Spine 动画
├── leaderskin/                         # 主战者皮肤 Spine 动画
└── rendered/                           # 最终渲染的卡牌图
```

## 许可

MIT
