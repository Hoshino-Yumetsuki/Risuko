use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Fetch the AES-128 decryption key from a remote URI
pub async fn fetch_decryption_key(
    key_uri: &str,
    client: &reqwest::Client,
) -> Result<[u8; 16], String> {
    let resp = client
        .get(key_uri)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch decryption key: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Key fetch failed with status {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read key body: {e}"))?;

    if bytes.len() != 16 {
        return Err(format!(
            "Invalid key length: expected 16 bytes, got {}",
            bytes.len()
        ));
    }

    let mut key = [0u8; 16];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Derive IV from media sequence number (big-endian u128)
pub fn iv_from_sequence(sequence_number: u64) -> [u8; 16] {
    let val = sequence_number as u128;
    val.to_be_bytes()
}

/// Decrypt a segment using AES-128-CBC with PKCS7 padding
pub fn decrypt_segment(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    // AES-128-CBC requires data length to be multiple of 16
    if data.len() % 16 != 0 {
        return Err(format!(
            "Ciphertext length {} is not a multiple of 16",
            data.len()
        ));
    }

    let mut buf = data.to_vec();
    let decrypted = Aes128CbcDec::new(key.into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| format!("AES decryption failed: {e}"))?;

    Ok(decrypted.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iv_from_sequence() {
        let iv = iv_from_sequence(0);
        assert_eq!(iv, [0u8; 16]);

        let iv = iv_from_sequence(1);
        assert_eq!(iv[15], 1);
        assert_eq!(iv[0..15], [0u8; 15]);

        let iv = iv_from_sequence(256);
        assert_eq!(iv[14], 1);
        assert_eq!(iv[15], 0);
    }

    #[test]
    fn test_decrypt_empty() {
        let key = [0u8; 16];
        let iv = [0u8; 16];
        let result = decrypt_segment(&[], &key, &iv);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_decrypt_invalid_length() {
        let key = [0u8; 16];
        let iv = [0u8; 16];
        let result = decrypt_segment(&[1, 2, 3], &key, &iv);
        assert!(result.is_err());
    }
}
