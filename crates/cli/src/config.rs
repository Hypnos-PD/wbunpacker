//! CLI 配置加载

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AppConfig {
    /// 数据根目录，所有输出文件都放在这个目录下
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    pub manifest_address: String,
    pub default_version: String,
    pub asset_bundle_address: String,
    pub version_address: String,
    pub common_header: String,
    pub routing_header: String,
    pub device_info: DeviceInfo,
    pub app_version: String,
    pub device_uuid: String,
    pub md5_salt: String,
    pub asset_bundle_base_keys: String,
    /// vgmstream-cli.exe 完整路径
    #[serde(default = "default_vgmstream_path")]
    pub vgmstream_path: String,
    /// ffmpeg.exe 完整路径（留空则用 PATH 中的 ffmpeg）
    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,
    /// AssetStudioModCLI.exe 完整路径
    #[serde(default = "default_asset_studio_path")]
    pub asset_studio_path: String,
    pub client_id: i64,
    pub sqlite3mc_key: String,
    pub sqlite3mc_base_key: String,
}

fn default_data_dir() -> String {
    "data".into()
}

fn default_vgmstream_path() -> String {
    r"D:\Tools\vgmstream-win64\vgmstream-cli.exe".into()
}

fn default_ffmpeg_path() -> String {
    "ffmpeg".into()
}

fn default_asset_studio_path() -> String {
    r"D:\Tools\AssetStudioModCLI_net9_win64\AssetStudioModCLI.exe".into()
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct DeviceInfo {
    pub platform: u32,
    pub device: u32,
    pub device_name: String,
    pub platform_os_version: String,
    pub gpu_vendor: String,
    pub graphics_memory_mb: String,
    pub processor_type: String,
}

pub fn load() -> anyhow::Result<AppConfig> {
    let path =
        std::env::var("WBU_CONFIG").unwrap_or_else(|_| "config/Config.local.toml".into());
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("无法读取配置文件: {path}"))?;
    toml::from_str(&content).context("配置文件 TOML 解析失败")
}
