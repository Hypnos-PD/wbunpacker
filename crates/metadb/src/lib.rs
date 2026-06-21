//! MetaDB 解密模块
//!
//! 通过 libloading 动态加载游戏的 libnative.dll（内嵌 SQLite3MC），
//! 调用 sqlite3_key (3-arg, raw key) 或 sqlite3_key_v2 (4-arg, XOR-derived)。
//!
//! 密钥派生：final_key[i] = sqlite3mc_key[i] ^ sqlite3mc_base_key[i % 0xD]

use anyhow::{anyhow, Context};
use base64::Engine;
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::Path;

type FnOpen = unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int;
type FnKey = unsafe extern "C" fn(*mut c_void, *const c_void, c_int) -> c_int;
type FnKeyV2 = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_void, c_int) -> c_int;
type FnClose = unsafe extern "C" fn(*mut c_void) -> c_int;

const BASE_KEY_PERIOD: usize = 0x0D;

pub fn derive_final_key(key_b64: &str, base_key_b64: &str) -> anyhow::Result<Vec<u8>> {
    let k = base64::engine::general_purpose::STANDARD.decode(key_b64).context("key b64 fail")?;
    let bk = base64::engine::general_purpose::STANDARD.decode(base_key_b64).context("base_key b64 fail")?;
    Ok(k.iter().enumerate().map(|(i, &x)| x ^ bk[i % BASE_KEY_PERIOD]).collect())
}

pub fn decrypt_metadb(input_path: &Path, output_path: &Path, dll_path: &Path, key_b64: &str, base_key_b64: &str) -> anyhow::Result<()> {
    if key_b64.is_empty() { return Err(anyhow!("sqlite3mc_key must be set")); }
    if let Some(p) = output_path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::copy(input_path, output_path).context("copy fail")?;

    let lib = unsafe { Library::new(dll_path) }.context("load DLL fail")?;
    let open_fn: Symbol<FnOpen> = unsafe { lib.get(b"sqlite3_open") }.context("open")?;
    let close_fn: Symbol<FnClose> = unsafe { lib.get(b"sqlite3_close") }.context("close")?;
    let rekey_fn: Symbol<FnKeyV2> = unsafe { lib.get(b"sqlite3_rekey_v2") }.context("rekey_v2")?;

    let c_path = CString::new(output_path.to_str().unwrap_or(""))?;
    let db_name = CString::new("main")?;
    let mut db: *mut c_void = std::ptr::null_mut();

    let rc = unsafe { open_fn(c_path.as_ptr(), &mut db) };
    if rc != 0 { unsafe { close_fn(db) }; return Err(anyhow!("open: {rc}")); }

    if base_key_b64.is_empty() {
        // Game mode: pass raw key bytes to sqlite3_key (3-arg)
        let raw = key_b64.as_bytes();
        let key_fn: Symbol<FnKey> = unsafe { lib.get(b"sqlite3_key") }.context("key")?;
        let rc = unsafe { key_fn(db, raw.as_ptr() as *const c_void, raw.len() as c_int) };
        if rc != 0 { unsafe { close_fn(db) }; return Err(anyhow!("key: {rc}")); }
    } else {
        // W2AU mode: derive via XOR and use sqlite3_key_v2 (4-arg)
        let final_key = derive_final_key(key_b64, base_key_b64)?;
        let key_fn: Symbol<FnKeyV2> = unsafe { lib.get(b"sqlite3_key_v2") }.context("key_v2")?;
        let rc = unsafe { key_fn(db, db_name.as_ptr(), final_key.as_ptr() as *const c_void, final_key.len() as c_int) };
        if rc != 0 { unsafe { close_fn(db) }; return Err(anyhow!("key_v2: {rc}")); }
    }

    let rc = unsafe { rekey_fn(db, db_name.as_ptr(), std::ptr::null(), 0) };
    if rc != 0 { unsafe { close_fn(db) }; return Err(anyhow!("rekey: {rc}")); }

    unsafe { close_fn(db) };
    Ok(())
}

#[cfg(test)] mod tests { use super::*;
    #[test] fn test_derive() { let k=base64::engine::general_purpose::STANDARD.encode(b"aaaa"); let bk=base64::engine::general_purpose::STANDARD.encode(b"C"); assert_eq!(derive_final_key(&k,&bk).unwrap().len(), 4); }
}