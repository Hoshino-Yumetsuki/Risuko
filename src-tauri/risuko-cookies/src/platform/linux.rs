// Linux Chromium cookie decryption

use aes::Aes128;
use cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
use eyre::{bail, Result};
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;
use zbus::blocking::Connection;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

pub fn decrypt_value(encrypted: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    if encrypted.len() < 3 {
        bail!("encrypted data too short");
    }

    if &encrypted[..3] == b"v10" || &encrypted[..3] == b"v11" {
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
    pbkdf2_hmac::<Sha1>(master_key, b"saltysalt", 1, &mut key);

    let decrypted = Aes128CbcDec::new(&key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|e| eyre::eyre!("aes-cbc decrypt failed: {:?}", e))?;

    Ok(decrypted)
}

pub fn extract_master_key(_local_state_path: &std::path::Path) -> Result<Vec<u8>> {
    // Connect to the D-Bus Secret Service and look for a Chrome entry in the login collection
    let conn = Connection::session()?;

    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.secrets",
        "/org/freedesktop/secrets/collection/login",
        "org.freedesktop.Secret.Collection",
    )?;

    let items: Vec<zbus::zvariant::OwnedObjectPath> = proxy.call(
        "SearchItems",
        &(std::collections::HashMap::<&str, &str>::from([(
            "application",
            "chrome",
        )]),),
    )?;

    if items.is_empty() {
        bail!("no chrome password in secret service");
    }

    let item_path = &items[0];
    let item_proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.secrets",
        item_path,
        "org.freedesktop.Secret.Item",
    )?;

    let session_path: zbus::zvariant::OwnedObjectPath =
        proxy.call("OpenSession", &("plain", zbus::zvariant::Value::from("")))?;

    let secret: (zbus::zvariant::OwnedObjectPath, Vec<u8>, Vec<u8>, String) =
        item_proxy.call("GetSecret", &(session_path,))?;

    Ok(secret.2)
}
