//! 版本查询模块
//!
//! # 概述
//!
//! 向游戏服务器查询当前客户端的资源版本号。
//! 这是整个解包管线的第一步 —— 得知最新版本号后，
//! 才能下载对应版本的 manifest 和资源。
//!
//! # 查询流程
//!
//! 1. 生成 SID = MD5(ClientId + DeviceUUID + MD5Salt)
//! 2. 构造 VersionInfoRequest（msgpack 序列化）
//! 3. 用 ConeShell V4 加密请求体
//! 4. POST 到 VersionAddress，带认证 headers
//! 5. 解密响应 → msgpack 反序列化 → VersionInfo
//!
//! # ConeShell V4 加密（待实现）
//!
//! ConeShell 是 Cygames 自研的客户端-服务器通信加密协议。
//! 原 W2AU 的 C# 版本使用 LibConeshell 子模块（ConeshellV4.cs）。
//! Rust 版本需要完整实现此加密协议：
//!
//! ```text
//! CommonHeader (Base64) → 解析 ConeShell header
//! DeviceUUID → 设备密钥派生
//! SID → 会话密钥派生
//! ↓
//! coneShell.EncryptRequest(msgpack_bytes) → { Data, ... }
//! POST → 服务端
//! coneShell.DecryptResponse(base64_response, &request) → msgpack_bytes
//! ```
//!
//! # 注意事项
//!
//! - version 命令不是硬依赖：如果已知版本号，可跳过此命令直接调用 `manifest --version`
//! - MD5 哈希使用 UTF-8 编码的字符串拼接 + MD5Salt 盐值
//! - 日文版（JPN）的请求参数与多语言版不同（无 variant 后缀）

use anyhow::Context;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

// ============================================================================
// 数据结构
// ============================================================================

/// 发送给版本服务器的请求体。
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionInfoRequest {
    /// 客户端 UUID（可为空字符串）
    pub uuid: String,
    /// 客户端 ID（来自配置文件）
    pub client_id: i64,
}

/// 版本服务器返回的资源版本信息。
///
/// 其中 `resource_version` 就是后续 `manifest --version` 需要的值。
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionInfo {
    /// 资源版本号，如 "4.1.0"
    pub resource_version: String,
    /// 客户端应用版本号
    pub app_version: Option<String>,
    /// 服务器时间戳
    pub server_time: Option<i64>,
    /// 是否处于维护状态
    pub is_maintenance: Option<bool>,
}

/// 认证请求的 header 集合。
///
/// 这些 header 模拟了游戏客户端的行为，
/// 服务端会校验它们以确认请求来自合法客户端。
#[derive(Debug, Clone)]
pub struct AuthHeaders {
    /// SID = MD5(ClientId + DeviceUUID + MD5Salt)
    pub sid: String,
    /// 资源版本（首次查询固定为 "00000000"）
    pub res_ver: String,
    /// 应用版本号
    pub app_ver: String,
    /// 客户端 ID
    pub client_id: String,
    /// 平台类型
    pub platform: String,
    /// 设备类型
    pub device: String,
    /// 路由 header
    pub routing_header: String,
    /// 设备名称
    pub device_name: String,
    /// 操作系统版本
    pub platform_os_version: String,
    /// GPU 厂商
    pub gpu_vendor: String,
    /// 显存大小（MB）
    pub graphics_memory_mb: String,
    /// 处理器型号
    pub processor_type: String,
}

// ============================================================================
// 公共 API
// ============================================================================

/// 计算 SID（Session ID）。
///
/// ```text
/// SID = MD5(ClientId.toString() + DeviceUUID + MD5Salt)
/// ```
///
/// 所有字节均为 UTF-8 编码，结果转为小写十六进制字符串。
pub fn compute_sid(client_id: i64, device_uuid: &str, md5_salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(client_id.to_string().as_bytes());
    hasher.update(device_uuid.as_bytes());
    hasher.update(md5_salt.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 查询游戏服务器的当前资源版本号。
///
/// # 参数
/// - `version_url`: 版本查询 API 地址
/// - `headers`: 认证 headers
/// - `common_header_b64`: Base64 编码的 ConeShell CommonHeader
///
/// # 返回
/// 如果查询成功则返回 VersionInfo（包含 resource_version）。
///
/// # 实现状态
/// 当前为骨架 —— ConeShell V4 加密逻辑待实现。
pub async fn query_version(
    _version_url: &str,
    _headers: &AuthHeaders,
    _common_header_b64: &str,
) -> anyhow::Result<VersionInfo> {
    todo!("版本查询实现（需 ConeShell V4 加密）")
}

/// 将 VersionInfo 序列化为 JSON 并保存到文件。
///
/// 输出路径：`data/version/VersionInfo.json`
pub fn save_version_info(info: &VersionInfo, output_dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(output_dir)?;
    let path = output_dir.join("VersionInfo.json");
    let json = serde_json::to_string_pretty(info)?;
    std::fs::write(&path, json)?;
    tracing::info!("版本信息已保存: {}", path.display());
    Ok(())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 SID 计算的可复现性
    #[test]
    fn test_sid_deterministic() {
        let sid1 = compute_sid(12345, "uuid-test", "salt");
        let sid2 = compute_sid(12345, "uuid-test", "salt");
        assert_eq!(sid1, sid2, "SID 应对相同输入产生相同输出");
    }

    /// 验证不同 salt 产生不同 SID
    #[test]
    fn test_sid_different_salt() {
        let sid1 = compute_sid(12345, "uuid", "salt_a");
        let sid2 = compute_sid(12345, "uuid", "salt_b");
        assert_ne!(sid1, sid2, "不同 salt 应产生不同 SID");
    }
}
