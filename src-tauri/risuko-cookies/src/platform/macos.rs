// macOS Chromium cookie decryption
use aes::Aes128;
use cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
use eyre::{bail, Result};
use pbkdf2::pbkdf2_hmac;
use security_framework::passwords::get_generic_password;
use sha1::Sha1;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

pub fn decrypt_value(encrypted: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    if encrypted.len() < 3 {
        bail!("encrypted data too short");
    }

    if &encrypted[..3] == b"v10" {
        decrypt_v10(encrypted, key)
    } else {
        Ok(encrypted.to_vec())
    }
}

fn decrypt_v10(data: &[u8], master_key: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 19 {
        bail!("v10 data too short");
    }

    // v10 format: "v10" + 16-byte IV + ciphertext
    let iv: [u8; 16] = data[3..19].try_into()?;
    let ciphertext = &data[19..];

    let mut key = [0u8; 16];
    pbkdf2_hmac::<Sha1>(master_key, b"saltysalt", 1003, &mut key);

    let decrypted = Aes128CbcDec::new(&key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|e| eyre::eyre!("aes-cbc decrypt failed: {:?}", e))?;

    Ok(decrypted)
}

pub fn extract_master_key(_local_state_path: &std::path::Path) -> Result<Vec<u8>> {
    // Try "Chrome" first, then "Chromium" as fallback
    get_generic_password("Chrome Safe Storage", "Chrome")
        .or_else(|_| get_generic_password("Chromium Safe Storage", "Chromium"))
        .map(|pw| pw.to_vec())
        .map_err(|e| eyre::eyre!("keychain access failed: {}", e))
}
