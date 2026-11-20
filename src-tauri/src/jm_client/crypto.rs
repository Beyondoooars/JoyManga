use aes::cipher::{BlockCipherDecrypt, KeyInit};
use aes::Aes256;
use anyhow::{anyhow, ensure, Context};
use base64::engine::general_purpose;
use base64::Engine;

pub(crate) const APP_TOKEN_SECRET: &str = "18comicAPP";
const APP_TOKEN_SECRET_2: &str = "18comicAPPContent";
const APP_DATA_SECRET: &str = "185Hcomic3PAPP7R";
/// 域名更新载荷的解密密钥（与官方实现一致）
const API_DOMAIN_SERVER_SECRET: &str = "diosfjckwpqpdfjkvnqQjsik";

// 计算MD5哈希并返回十六进制字符串
pub(crate) fn md5_hex(data: &str) -> String {
    format!("{:x}", md5::compute(data))
}

pub(super) fn api_token(ts: u64, is_scramble: bool) -> String {
    let secret = if is_scramble {
        APP_TOKEN_SECRET_2
    } else {
        APP_TOKEN_SECRET
    };
    md5_hex(&format!("{ts}{secret}"))
}

/// 使用 AES-256-ECB 解密 base64 数据（自动去除 PKCS#7 填充）
pub(crate) fn aes256_ecb_decrypt_base64(key: &[u8], data: &str) -> anyhow::Result<Vec<u8>> {
    let encrypted = general_purpose::STANDARD
        .decode(data)
        .context("base64解码失败")?;
    if encrypted.is_empty() {
        return Err(anyhow!("密文为空"));
    }
    if !encrypted.len().is_multiple_of(16) {
        return Err(anyhow!(
            "密文长度`{}`不是AES块大小的整数倍",
            encrypted.len()
        ));
    }

    let cipher = Aes256::new_from_slice(key).map_err(|_| anyhow!("AES密钥长度必须为32字节"))?;
    let mut decrypted = Vec::with_capacity(encrypted.len());
    for block in encrypted.chunks(16) {
        let mut block =
            aes::cipher::Block::<Aes256>::try_from(block).expect("chunks(16) 保证块长度为16字节");
        cipher.decrypt_block(&mut block);
        decrypted.extend_from_slice(&block);
    }

    let pad_byte = *decrypted.last().context("解密结果为空")?;
    let pad = pad_byte as usize;
    ensure!((1..=16).contains(&pad), "非法的PKCS#7填充长度`{pad}`");
    ensure!(decrypted.len() >= pad, "PKCS#7填充长度超过解密结果长度");
    ensure!(
        decrypted[decrypted.len() - pad..]
            .iter()
            .all(|&b| b == pad_byte),
        "PKCS#7填充内容不一致"
    );
    decrypted.truncate(decrypted.len() - pad);
    Ok(decrypted)
}

pub(super) fn decrypt_data(ts: u64, data: &str) -> anyhow::Result<String> {
    let key = md5_hex(&format!("{ts}{APP_DATA_SECRET}"));
    let decrypted = aes256_ecb_decrypt_base64(key.as_bytes(), data).context("解密API响应失败")?;
    String::from_utf8(decrypted).context("解密后的数据不是UTF-8")
}

/// 去掉开头非 ASCII 字符 → base64 解码 → AES-256-ECB 解密（key = md5(密钥)）。
pub(super) fn decrypt_domain_list_payload(raw_text: &str) -> anyhow::Result<Vec<u8>> {
    let cleaned: String = raw_text.chars().skip_while(|c| !c.is_ascii()).collect();
    let cleaned = cleaned.trim();
    anyhow::ensure!(!cleaned.is_empty(), "域名更新载荷为空");

    let key = md5_hex(API_DOMAIN_SERVER_SECRET);
    aes256_ecb_decrypt_base64(key.as_bytes(), cleaned).context("解密域名更新载荷失败")
}

#[cfg(test)]
mod tests {
    use super::decrypt_domain_list_payload;

    #[test]
    fn domain_update_payload_decodes_to_server_list() {
        // 2026-10 从官方地址抓取的真实载荷（含开头 BOM 与封装），
        // 验证「去非 ASCII 前缀 → base64 → AES-256-ECB → Server 字段」链路。
        let payload = "\u{feff}X+bnzYIcwF6C7Rd3T7njPDNH08zsH9zyqCrrjCr7qcnHb1LsmIZGIHtrNVR/GiraHE6OuhvrxEzwciVvhdU0I9OYcmWTxF1K7fLfcwkn7kMQg2DZ2qpE7dKGkqKCmQijaSUOswxL1/p9pSVe/vRYEzbB5pfcAB6Yz/zVVIendBJK629QiqQndRXM9bijtZuYJtKw3YBAA26a+fy06dNszfw9v/4R8akVaSTWLOJc0nJy+9vm2t2W997vcqFL91iklKuKVEZHTtdpaLTgWExXaLjtIz2zlVZfy3jYrzKZ7x+LL7o02c6WB4HV69s1VqCJYl+3l3RNwDjJ0iRNnG9p/caZL/y6sT8i78Wc38WZhOAxkDsOFiGNpvS3eojKA0wGhpTtWsuUrXgSZ7ilEPttJvOhrJGGRJJ7Ux4HgzDYO1A=";
        let decrypted = decrypt_domain_list_payload(payload).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&decrypted).unwrap();
        let servers: Vec<&str> = value["Server"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|server| server.as_str())
            .collect();
        assert!(!servers.is_empty());
        assert!(servers.iter().all(|server| server.contains('.')));
    }
}
