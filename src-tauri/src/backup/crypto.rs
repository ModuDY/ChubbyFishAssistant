use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

// v1 fixes Argon2id parameters and authenticates the format marker as AAD.
const MAGIC: &[u8; 8] = b"AIWBK001";
pub const MAX_BYTES: usize = 15 * 1024 * 1024;

fn key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut key = Zeroizing::new([0u8; 32]);
    let params = Params::new(19456, 2, 1, Some(32)).map_err(|_| "加密参数错误")?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|_| "密码派生失败")?;
    Ok(key)
}

pub fn seal(plain: &[u8], password: &str) -> Result<Vec<u8>, String> {
    if plain.len() > MAX_BYTES - 52 {
        return Err("备份超过 15 MiB 限制".into());
    }
    let mut header = [0u8; 36];
    header[..8].copy_from_slice(MAGIC);
    crate::vault::csprng_fill(&mut header[8..])?;
    let key = key(password, &header[8..24])?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "加密初始化失败")?;
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&header[24..]),
            Payload {
                msg: plain,
                aad: MAGIC,
            },
        )
        .map_err(|_| "备份加密失败")?;
    Ok([header.as_slice(), ct.as_slice()].concat())
}

pub fn open(sealed: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    if sealed.len() < 52 || sealed.len() > MAX_BYTES || &sealed[..8] != MAGIC {
        return Err("不是有效的大肥鱼助手加密备份，或文件超过 15 MiB".into());
    }
    let key = key(password, &sealed[8..24])?;
    Aes256Gcm::new_from_slice(key.as_ref())
        .map_err(|_| "解密初始化失败")?
        .decrypt(
            Nonce::from_slice(&sealed[24..36]),
            Payload {
                msg: &sealed[36..],
                aad: MAGIC,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| "备份密码错误或文件已损坏".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_roundtrip() {
        let plain = b"synthetic-token-not-a-real-credential";
        let a = seal(plain, "test-password").unwrap();
        let b = seal(plain, "test-password").unwrap();
        assert_ne!(a, b);
        assert_eq!(&**open(&a, "test-password").unwrap(), plain);
        assert!(!a.windows(plain.len()).any(|w| w == plain));
        assert!(open(&a, "wrong").is_err());
        let mut bad = a.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&bad, "test-password").is_err());
        for len in [0, 7, 35, 51] {
            assert!(open(&a[..len], "test-password").is_err());
        }
    }
}
