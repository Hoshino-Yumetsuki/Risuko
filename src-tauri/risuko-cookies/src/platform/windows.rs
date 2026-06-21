// Windows Chromium cookie decryption

use aes_gcm::{aead::Aead, Aes256Gcm, Key, KeyInit, Nonce};
use base64::Engine;
use eyre::{bail, Result};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB},
};

pub const ELEVATION_REQUIRED: &str = "risuko:elevation-required";

pub fn decrypt_value(encrypted: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    if encrypted.len() < 3 {
        bail!("encrypted data too short");
    }

    match &encrypted[..3] {
        b"v10" => decrypt_v10(encrypted, key),
        b"v20" => decrypt_v20(encrypted, key),
        _ => decrypt_dpapi(encrypted),
    }
}

fn decrypt_v10(data: &[u8], master_key: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 15 {
        bail!("v10 data too short");
    }

    let nonce = Nonce::from_slice(&data[3..15]);
    let ciphertext = &data[15..];

    let key = Key::<Aes256Gcm>::from_slice(master_key);
    let cipher = Aes256Gcm::new(key);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| eyre::eyre!("aes-gcm decrypt failed: {}", e))
}

fn decrypt_v20(data: &[u8], _master_key: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 35 {
        bail!("v20 data too short");
    }

    // Chrome 130+ app-bound encryption needs an elevated token to access the key
    bail!("{}", ELEVATION_REQUIRED);
}

fn decrypt_dpapi(data: &[u8]) -> Result<Vec<u8>> {
    // Use the Windows DPAPI to decrypt the master key — this runs under the current user's context
    unsafe {
        let mut blob_in = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };

        let mut blob_out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let result = CryptUnprotectData(
            &mut blob_in,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut blob_out,
        );

        if result == 0 {
            bail!("DPAPI decrypt failed");
        }

        let decrypted =
            std::slice::from_raw_parts(blob_out.pbData, blob_out.cbData as usize).to_vec();

        LocalFree(blob_out.pbData as isize);

        Ok(decrypted)
    }
}

pub fn extract_master_key(local_state_path: &std::path::Path) -> Result<Vec<u8>> {
    // Read the "Local State" JSON, extract the base64-encoded encrypted key, strip the DPAPI prefix
    let content = std::fs::read_to_string(local_state_path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;

    let encrypted_key_b64 = json
        .get("os_crypt")
        .and_then(|v| v.get("encrypted_key"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("missing os_crypt.encrypted_key"))?;

    let encrypted_key = base64::engine::general_purpose::STANDARD.decode(encrypted_key_b64)?;

    if encrypted_key.len() < 5 || &encrypted_key[..5] != b"DPAPI" {
        bail!("invalid encrypted key format");
    }

    decrypt_dpapi(&encrypted_key[5..])
}
