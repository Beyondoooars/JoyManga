//! 登录凭据的本地加密存储。
//!
//! `config.json` 是明文 JSON，直接写密码会让任何能读到文件的工具（备份、文件管理器、
//! 云同步）看到明文。这里用 AES-256-GCM 加密后以 `enc:v1:<base64>` 形式落盘：
//!
//! - 密钥由本机稳定标识经 HKDF-SHA256 派生，不落盘，换设备无法解密（预期行为）；
//! - 每次加密使用新的随机 nonce，相同密码两次加密的密文不同；
//! - 解密失败（密钥变化、文件被改）返回错误而非静默清空，由调用方决定如何提示；
//! - 历史明文密码原样读出，保存时自动转为密文。
//!
//! 这是**本机静态存储保护**，不是防止拿到本机 root 权限的攻击者：密钥派生材料同样
//! 在本机。目标是让凭据不再以明文出现在配置、备份与日志里。
//!
//! 加密与派生的底层算法都来自 `ring`（AES-256-GCM、HKDF-SHA256），不自行实现。

use anyhow::{anyhow, ensure, Context};
use base64::engine::general_purpose;
use base64::Engine as _;
use ring::aead;
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};

/// 密文前缀。带版本号，方便以后换算法时识别旧格式。
const CIPHER_PREFIX: &str = "enc:v1:";
/// GCM nonce 长度（96 位，`ring` 要求）。
const NONCE_LEN: usize = 12;
/// GCM 认证标签长度。
const TAG_LEN: usize = 16;

/// 本机稳定标识：用户主目录 + 操作系统。
///
/// 不读取硬件序列号一类的设备标识：既拿不到也不需要，路径已随安装稳定。
/// 取不到时用固定串兜底，保证同一台机器上前后一致即可。
fn machine_salt() -> String {
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "joymanga".to_string());
    format!("{base}|{}", std::env::consts::OS)
}

/// 由本机标识经 HKDF-SHA256 派生 32 字节密钥，不落盘。
fn derive_key(machine: &str) -> anyhow::Result<[u8; 32]> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, machine.as_bytes());
    let material = salt.extract(b"joymanga-credential-v1");
    let info: [&[u8]; 1] = [b"config-password"];
    let okm = material
        .expand(&info, KeyLength)
        .map_err(|_| anyhow!("派生凭据密钥失败"))?;
    let mut key = [0u8; 32];
    okm.fill(&mut key)
        .map_err(|_| anyhow!("派生凭据密钥失败"))?;
    Ok(key)
}

/// `hkdf` 要求实现 `KeyType`，这里只需要固定的 32 字节。
struct KeyLength;
impl hkdf::KeyType for KeyLength {
    fn len(&self) -> usize {
        32
    }
}

fn random_nonce() -> anyhow::Result<[u8; NONCE_LEN]> {
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| anyhow!("生成随机数失败"))?;
    Ok(nonce)
}

/// 加密凭据；空字符串原样返回（未设置账号时不必加密空值）。
pub fn encrypt(plaintext: &str, machine: &str) -> anyhow::Result<String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let key = derive_key(machine)?;
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &key)
        .map_err(|_| anyhow!("初始化加密算法失败"))?;
    let key = aead::LessSafeKey::new(unbound);
    let nonce_bytes = random_nonce()?;
    let nonce = aead::Nonce::assume_unique_for_key(nonce_bytes);
    // 前缀作为 AAD：把版本号绑进认证范围，篡改版本前缀会导致校验失败。
    let mut in_out = plaintext.as_bytes().to_vec();
    key.seal_in_place_append_tag(
        nonce,
        aead::Aad::from(CIPHER_PREFIX.as_bytes()),
        &mut in_out,
    )
    .map_err(|_| anyhow!("加密凭据失败"))?;
    // in_out = 密文 || tag，与解密时 split_at 的布局保持一致
    let mut payload = Vec::with_capacity(NONCE_LEN + in_out.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&in_out);
    Ok(format!(
        "{CIPHER_PREFIX}{}",
        general_purpose::STANDARD.encode(payload)
    ))
}

/// 解密凭据。非加密格式（历史明文配置）原样返回，便于一次性迁移。
pub fn decrypt(stored: &str, machine: &str) -> anyhow::Result<String> {
    let Some(encoded) = stored.strip_prefix(CIPHER_PREFIX) else {
        return Ok(stored.to_string());
    };
    let payload = general_purpose::STANDARD
        .decode(encoded)
        .context("凭据密文不是有效的 base64")?;
    ensure!(
        payload.len() > NONCE_LEN + TAG_LEN,
        "凭据密文长度异常，可能已损坏"
    );
    let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
    let mut nonce_bytes_array = [0u8; NONCE_LEN];
    nonce_bytes_array.copy_from_slice(nonce_bytes);
    let key = derive_key(machine)?;
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &key)
        .map_err(|_| anyhow!("初始化解密算法失败"))?;
    let key = aead::LessSafeKey::new(unbound);
    let mut in_out = ciphertext.to_vec();
    let plaintext = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce_bytes_array),
            aead::Aad::from(CIPHER_PREFIX.as_bytes()),
            &mut in_out,
        )
        .map_err(|_| anyhow!("凭据解密失败：密钥不匹配或密文已损坏"))?;
    String::from_utf8(plaintext.to_vec()).context("凭据明文不是有效的 UTF-8")
}

/// 供配置层使用：用当前机器的密钥加密。
pub fn encrypt_for_this_machine(plaintext: &str) -> anyhow::Result<String> {
    encrypt(plaintext, &machine_salt())
}

/// 供配置层使用：用当前机器的密钥解密；兼容历史明文。
pub fn decrypt_for_this_machine(stored: &str) -> anyhow::Result<String> {
    decrypt(stored, &machine_salt())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &str = "test-machine";

    #[test]
    fn roundtrip_preserves_plaintext() {
        let plaintext = "correct horse battery staple 密码";
        let encrypted = encrypt(plaintext, MACHINE).unwrap();
        assert!(encrypted.starts_with(CIPHER_PREFIX));
        assert!(!encrypted.contains(plaintext));
        assert_eq!(decrypt(&encrypted, MACHINE).unwrap(), plaintext);
    }

    #[test]
    fn same_plaintext_encrypts_differently_each_time() {
        let first = encrypt("secret", MACHINE).unwrap();
        let second = encrypt("secret", MACHINE).unwrap();
        // 随机 nonce：密文不应相同，否则能看出两个账号用了同一个密码
        assert_ne!(first, second);
        assert_eq!(decrypt(&first, MACHINE).unwrap(), "secret");
        assert_eq!(decrypt(&second, MACHINE).unwrap(), "secret");
    }

    #[test]
    fn wrong_machine_cannot_decrypt() {
        let encrypted = encrypt("secret", MACHINE).unwrap();
        assert!(decrypt(&encrypted, "another-machine").is_err());
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let encrypted = encrypt("secret", MACHINE).unwrap();
        let encoded = encrypted.strip_prefix(CIPHER_PREFIX).unwrap();
        let mut payload = general_purpose::STANDARD.decode(encoded).unwrap();
        let last = payload.len() - 1;
        payload[last] ^= 0x01;
        let tampered = format!(
            "{CIPHER_PREFIX}{}",
            general_purpose::STANDARD.encode(payload)
        );
        assert!(decrypt(&tampered, MACHINE).is_err());
    }

    #[test]
    fn legacy_plaintext_is_returned_unchanged() {
        // 历史配置里是明文密码：迁移期必须能读出来，不能当成损坏
        assert_eq!(
            decrypt("plain-password", MACHINE).unwrap(),
            "plain-password"
        );
    }

    #[test]
    fn empty_value_stays_empty() {
        assert_eq!(encrypt("", MACHINE).unwrap(), "");
        assert_eq!(decrypt("", MACHINE).unwrap(), "");
    }
}
