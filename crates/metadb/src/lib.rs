//! MetaDB 解密模块
//!
//! 通过 libloading 动态加载游戏的 libnative.dll（内嵌 SQLite3MC），
//! 调用 sqlite3_key_v2 / sqlite3_rekey_v2 解密客户端的 meta 数据库。
//!
//! # 密钥派生
//!
//! final_key[i] = sqlite3mc_key[i] ^ sqlite3mc_base_key[i % 0xD]

use anyhow::{anyhow, Context};
use base64::Engine;
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::Path;

type FnOpen = unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int;
type FnKeyV2 = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_void, c_int) -> c_int;
type FnClose = unsafe extern "C" fn(*mut c_void) -> c_int;

const BASE_KEY_PERIOD: usize = 0x0D;

/// 从两个 Base64 密钥生成 SQLite3MC 最终密钥。
pub fn derive_final_key(key_b64: &str, base_key_b64: &str) -> anyhow::Result<Vec<u8>> {
    let key = base64::engine::general_purpose::STANDARD
        .decode(key_b64)
        .context("Sqlite3mcKey Base64 decode failed")?;
    let base_key = base64::engine::general_purpose::STANDARD
        .decode(base_key_b64)
        .context("Sqlite3mcBaseKey Base64 decode failed")?;
    Ok(key
        .iter()
        .enumerate()
        .map(|(i, &k)| k ^ base_key[i % BASE_KEY_PERIOD])
        .collect())
}

/// 解密 meta.db：复制 → 打开 → 设密钥 → 移除加密 → 关闭。
pub fn decrypt_metadb(
    input_path: &Path,
    output_path: &Path,
    dll_path: &Path,
    key_b64: &str,
    base_key_b64: &str,
) -> anyhow::Result<()> {
    if key_b64.is_empty() || base_key_b64.is_empty() {
        return Err(anyhow!(
            "sqlite3mc_key and sqlite3mc_base_key must be set in config"
        ));
    }

    let final_key = derive_final_key(key_b64, base_key_b64)?;

    // Copy encrypted file to output first (sqlite3_key_v2 works in-place)
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(input_path, output_path)
        .with_context(|| format!("Cannot copy {} -> {}", input_path.display(), output_path.display()))?;

    // Load libnative.dll (contains SQLite3MC)
    let lib = unsafe { Library::new(dll_path) }
        .with_context(|| format!("Cannot load {}", dll_path.display()))?;

    let sqlite3_open: Symbol<FnOpen> =
        unsafe { lib.get(b"sqlite3_open") }.context("sqlite3_open not found")?;
    let sqlite3_key_v2: Symbol<FnKeyV2> =
        unsafe { lib.get(b"sqlite3_key_v2") }.context("sqlite3_key_v2 not found")?;
    let sqlite3_rekey_v2: Symbol<FnKeyV2> =
        unsafe { lib.get(b"sqlite3_rekey_v2") }.context("sqlite3_rekey_v2 not found")?;
    let sqlite3_close: Symbol<FnClose> =
        unsafe { lib.get(b"sqlite3_close") }.context("sqlite3_close not found")?;

    let c_path = CString::new(
        output_path
            .to_str()
            .ok_or_else(|| anyhow!("Non-UTF-8 output path"))?,
    )?;
    let db_name = CString::new("main")?;
    let mut db: *mut c_void = std::ptr::null_mut();

    // Open
    let rc = unsafe { sqlite3_open(c_path.as_ptr(), &mut db) };
    if rc != 0 {
        unsafe { sqlite3_close(db) };
        return Err(anyhow!("sqlite3_open returned {rc}"));
    }

    // Set key
    let rc = unsafe {
        sqlite3_key_v2(
            db,
            db_name.as_ptr(),
            final_key.as_ptr() as *const c_void,
            final_key.len() as c_int,
        )
    };
    if rc != 0 {
        unsafe { sqlite3_close(db) };
        return Err(anyhow!("sqlite3_key_v2 returned {rc} — maybe wrong keys?"));
    }

    // Remove encryption (rekey to NULL)
    let rc = unsafe { sqlite3_rekey_v2(db, db_name.as_ptr(), std::ptr::null(), 0) };
    if rc != 0 {
        unsafe { sqlite3_close(db) };
        return Err(anyhow!("sqlite3_rekey_v2 returned {rc}"));
    }

    unsafe { sqlite3_close(db) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_final_key_consistent() {
        let k1 = base64::engine::general_purpose::STANDARD.encode(b"aaaaaaaaaaaaaaaa");
        let bk = base64::engine::general_purpose::STANDARD.encode(b"CCCCCCCCCCCCC");
        let r1 = derive_final_key(&k1, &bk).unwrap();
        let r2 = derive_final_key(&k1, &bk).unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_derive_different_inputs_different_keys() {
        let k1 = base64::engine::general_purpose::STANDARD.encode(b"aaaaaaaaaaaaaaaa");
        let k2 = base64::engine::general_purpose::STANDARD.encode(b"bbbbbbbbbbbbbbbb");
        let bk = base64::engine::general_purpose::STANDARD.encode(b"CCCCCCCCCCCCC");
        assert_ne!(
            derive_final_key(&k1, &bk).unwrap(),
            derive_final_key(&k2, &bk).unwrap()
        );
    }
}