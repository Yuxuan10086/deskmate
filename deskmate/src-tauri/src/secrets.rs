//! 外部服务密钥的安全存储。
//!
//! Windows 平台使用 DPAPI（`CryptProtectData`）以「当前用户」为密钥材料加密，
//! 密文以 base64 落盘到 `%APPDATA%\com.deskmate.app\secrets.json`；
//! 明文仅在进程内存中短暂持有，数据库与磁盘不出现明文密钥。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// 存储键名（对外不可见）。
pub mod key {
    pub const SILICONFLOW_API_KEY: &str = "siliconflow_api_key";
    pub const SMTP_AUTH_CODE: &str = "smtp_auth_code";
    pub const SMTP_USER: &str = "smtp_user";
    pub const SMTP_TO: &str = "smtp_to";
    /// 火山引擎（豆包语音）API Key：用于 AST 2.0 实时同声传译 WebSocket。
    pub const VOLC_API_KEY: &str = "volc_api_key";
}

/// 密钥仓库：持有解密后的明文缓存，落盘时逐项加密。
pub struct SecretsStore {
    path: PathBuf,
    data: BTreeMap<String, String>,
}

impl SecretsStore {
    /// 从数据目录加载；文件缺失或不合法时按空仓库处理（不阻塞启动）。
    pub fn load(data_dir: &Path) -> AppResult<Self> {
        let path = data_dir.join("secrets.json");
        let mut data = BTreeMap::new();
        if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let file: BTreeMap<String, String> = serde_json::from_str(&raw).unwrap_or_default();
            for (k, v) in file {
                if let Ok(plain) = unprotect_b64(&v) {
                    data.insert(k, plain);
                }
            }
        }
        Ok(Self { path, data })
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.data.get(key).filter(|s| !s.is_empty()).cloned()
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// 写入（空字符串视为删除该项）并立即持久化。
    pub fn set(&mut self, key: &str, value: &str) -> AppResult<()> {
        if value.is_empty() {
            self.data.remove(key);
        } else {
            self.data.insert(key.to_string(), value.to_string());
        }
        self.persist()
    }

    pub fn clear(&mut self, key: &str) -> AppResult<()> {
        self.data.remove(key);
        self.persist()
    }

    fn persist(&self) -> AppResult<()> {
        let mut file: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in &self.data {
            file.insert(k.clone(), protect_b64(v.as_bytes())?);
        }
        let json = serde_json::to_string_pretty(&file)?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }
}

/* ------------------------------ 前端配置视图 ------------------------------ */

/// 暴露给设置页的配置概览：只回传掩码，不回传明文密钥。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretConfigDto {
    pub siliconflow_key_set: bool,
    pub siliconflow_key_masked: String,
    pub smtp_user: String,
    pub smtp_to: String,
    pub smtp_auth_set: bool,
    pub volc_key_set: bool,
    pub volc_key_masked: String,
}

impl SecretConfigDto {
    pub fn from_store(store: &SecretsStore) -> Self {
        let sf = store.get(key::SILICONFLOW_API_KEY).unwrap_or_default();
        let volc = store.get(key::VOLC_API_KEY).unwrap_or_default();
        Self {
            siliconflow_key_set: !sf.is_empty(),
            siliconflow_key_masked: mask(&sf),
            smtp_user: store.get(key::SMTP_USER).unwrap_or_default(),
            smtp_to: store.get(key::SMTP_TO).unwrap_or_default(),
            smtp_auth_set: store.has(key::SMTP_AUTH_CODE),
            volc_key_set: !volc.is_empty(),
            volc_key_masked: mask(&volc),
        }
    }
}

/// 局部更新：空字段表示「不修改」；clear* 为显式清除。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SecretConfigPatch {
    pub siliconflow_key: Option<String>,
    pub smtp_user: Option<String>,
    pub smtp_to: Option<String>,
    pub smtp_auth_code: Option<String>,
    pub clear_siliconflow: bool,
    pub clear_smtp: bool,
    pub volc_key: Option<String>,
    pub clear_volc: bool,
}

impl SecretsStore {
    pub fn apply_patch(&mut self, patch: SecretConfigPatch) -> AppResult<()> {
        if patch.clear_siliconflow {
            self.clear(key::SILICONFLOW_API_KEY)?;
        }
        if patch.clear_volc {
            self.clear(key::VOLC_API_KEY)?;
        }
        if patch.clear_smtp {
            self.clear(key::SMTP_AUTH_CODE)?;
            self.clear(key::SMTP_USER)?;
            self.clear(key::SMTP_TO)?;
        }
        // 非空才覆盖，避免掩码占位被误写回
        if let Some(v) = patch.siliconflow_key.filter(|v| !v.trim().is_empty()) {
            self.set(key::SILICONFLOW_API_KEY, v.trim())?;
        }
        if let Some(v) = patch.volc_key.filter(|v| !v.trim().is_empty()) {
            self.set(key::VOLC_API_KEY, v.trim())?;
        }
        if let Some(v) = patch.smtp_user {
            self.set(key::SMTP_USER, v.trim())?;
        }
        if let Some(v) = patch.smtp_to {
            self.set(key::SMTP_TO, v.trim())?;
        }
        if let Some(v) = patch.smtp_auth_code.filter(|v| !v.trim().is_empty()) {
            self.set(key::SMTP_AUTH_CODE, v.trim())?;
        }
        Ok(())
    }
}

/// 生成掩码：保留前缀与后 4 位，中间固定 8 个星号。
pub fn mask(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let tail: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let head: String = value.chars().take(3).collect();
    format!("{head}********{tail}")
}

/* ------------------------------ DPAPI 加解密 ----------------------------- */

fn protect_b64(plain: &[u8]) -> AppResult<String> {
    let blob = dpapi::protect(plain)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(blob))
}

fn unprotect_b64(encoded: &str) -> AppResult<String> {
    let blob = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    let plain = dpapi::unprotect(&blob)?;
    String::from_utf8(plain).map_err(|e| AppError::new(format!("密钥文本解码失败：{e}")))
}

#[cfg(windows)]
mod dpapi {
    use crate::error::{AppError, AppResult};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    pub fn protect(data: &[u8]) -> AppResult<Vec<u8>> {
        crypt(data, true)
    }

    pub fn unprotect(data: &[u8]) -> AppResult<Vec<u8>> {
        crypt(data, false)
    }

    fn crypt(data: &[u8], encrypt: bool) -> AppResult<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            let ok = if encrypt {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(AppError::new(if encrypt {
                    "密钥加密失败（DPAPI）"
                } else {
                    "密钥解密失败（DPAPI）"
                }));
            }
            let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData as *mut core::ffi::c_void);
            Ok(out)
        }
    }
}

/// 非 Windows 平台（本项目不发布）退化为明文存储，保证可编译。
#[cfg(not(windows))]
mod dpapi {
    use crate::error::AppResult;

    pub fn protect(data: &[u8]) -> AppResult<Vec<u8>> {
        Ok(data.to_vec())
    }

    pub fn unprotect(data: &[u8]) -> AppResult<Vec<u8>> {
        Ok(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let plain = "sk-test-1234567890";
        let encrypted = protect_b64(plain.as_bytes()).expect("加密失败");
        assert_ne!(encrypted, plain, "密文不应等于明文");
        let back = unprotect_b64(&encrypted).expect("解密失败");
        assert_eq!(back, plain);
    }

    #[test]
    fn mask_keeps_edges() {
        assert_eq!(mask("sk-abcdefgh1234"), "sk-********1234");
        assert_eq!(mask(""), "");
    }
}
