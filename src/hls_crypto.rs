//! 原生转发中的 AES-128 分片解密；不解码、转码或缓存完整视频。
use crate::{core::*, net, playback::Play};
use aes::{
    Aes128,
    cipher::{Block, BlockDecrypt, KeyInit},
};
use axum::http::{HeaderMap, Method};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Crypto {
    pub key_url: String,
    pub iv: [u8; 16],
}

pub(crate) struct Encryption {
    key_url: String,
    iv: Option<[u8; 16]>,
}

impl Encryption {
    pub fn parse(line: &str, base: &str) -> Result<Option<Self>> {
        static ATTR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r#"(?:^|,)([A-Z]+)=(?:"([^"]*)"|([^,]*))"#).unwrap()
        });
        let attrs: std::collections::HashMap<_, _> = ATTR
            .captures_iter(line)
            .map(|c| {
                (
                    c[1].to_owned(),
                    c.get(2).or_else(|| c.get(3)).unwrap().as_str().to_owned(),
                )
            })
            .collect();
        let attr = |key| attrs.get(key).map(String::as_str).unwrap_or("");
        if attr("METHOD") == "NONE" {
            return Ok(None);
        }
        if attr("METHOD") != "AES-128" || !matches!(attr("KEYFORMAT"), "" | "identity") {
            return Err(Error::bad("此加密格式无法原画转发，请选择其他播放源"));
        }
        if attr("URI").is_empty() {
            return Err(Error::bad("HLS 密钥地址无效"));
        }
        let key_url = net::valid_url(base)?
            .join(attr("URI"))
            .map_err(|_| Error::bad("HLS 密钥地址无效"))?;
        net::valid_url(key_url.as_str())?;
        let iv = if attr("IV").is_empty() {
            None
        } else {
            let hex = attr("IV")
                .strip_prefix("0x")
                .ok_or_else(|| Error::bad("HLS 初始化向量无效"))?;
            if hex.len() != 32 {
                return Err(Error::bad("HLS 初始化向量无效"));
            }
            let value =
                u128::from_str_radix(hex, 16).map_err(|_| Error::bad("HLS 初始化向量无效"))?;
            Some(value.to_be_bytes())
        };
        Ok(Some(Self {
            key_url: key_url.to_string(),
            iv,
        }))
    }

    pub fn segment(&self, sequence: u64) -> Crypto {
        Crypto {
            key_url: self.key_url.clone(),
            iv: self
                .iv
                .unwrap_or_else(|| u128::from(sequence).to_be_bytes()),
        }
    }

    pub fn initialization(&self) -> Result<Crypto> {
        if self.iv.is_none() {
            return Err(Error::bad("加密 HLS 初始化分片缺少 IV，请换源"));
        }
        Ok(self.segment(0))
    }
}

pub(crate) async fn key(app: &App, play: &Play, crypto: &Crypto) -> Result<[u8; 16]> {
    let cache = format!("hls-key:{}:{}", play.id, hash(&crypto.key_url));
    if let Some(value) = app.cache_get(&cache).await?
        && let Ok(bytes) = serde_json::from_value::<[u8; 16]>(value)
    {
        return Ok(bytes);
    }
    let response = net::request(app, Method::GET, &crypto.key_url, &HeaderMap::new()).await?;
    if !response.status().is_success() {
        return Err(Error::bad("HLS 密钥获取失败，请刷新或切换来源"));
    }
    let bytes: [u8; 16] = net::bytes(response, 16, false)
        .await?
        .try_into()
        .map_err(|_| Error::bad("HLS 密钥长度无效"))?;
    app.cache_put(&cache, &serde_json::json!(bytes), 300)
        .await?;
    Ok(bytes)
}

pub(crate) struct Cbc {
    cipher: Aes128,
    chain: [u8; 16],
    pending: Vec<u8>,
}

impl Cbc {
    pub fn new(key: &[u8; 16], iv: [u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(key.into()),
            chain: iv,
            pending: Vec::new(),
        }
    }

    fn blocks(&mut self, data: &mut [u8]) {
        for bytes in data.as_chunks_mut::<16>().0 {
            let encrypted = *bytes;
            let mut block = Block::<Aes128>::from(encrypted);
            self.cipher.decrypt_block(&mut block);
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = block[index] ^ self.chain[index];
            }
            self.chain = encrypted;
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        // 保留最后一个块，到流结束后校验并移除 PKCS#7 填充。
        let ready = (self.pending.len() / 16).saturating_sub(1) * 16;
        let mut output: Vec<_> = self.pending.drain(..ready).collect();
        self.blocks(&mut output);
        output
    }

    pub fn finish(mut self) -> std::io::Result<Vec<u8>> {
        if self.pending.len() != 16 {
            return Err(std::io::Error::other("HLS 加密分片未完整读取"));
        }
        let mut last = std::mem::take(&mut self.pending);
        self.blocks(&mut last);
        let padding = last[15] as usize;
        if !(1..=16).contains(&padding)
            || !last[16 - padding..]
                .iter()
                .all(|byte| *byte as usize == padding)
        {
            return Err(std::io::Error::other("HLS 分片解密校验失败"));
        }
        last.truncate(16 - padding);
        Ok(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(input: &str) -> Vec<u8> {
        (0..input.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&input[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cbc_matches_nist_blocks_across_uneven_network_chunks() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap();
        let iv = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let input = hex(concat!(
            "7649abac8119b246cee98e9b12e9197d",
            "5086cb9b507219ee95db113a917678b2",
            "73bed6b8e3c1743b7116e69e22229516",
            "3ff1caa1681fac09120eca307586e1a7"
        ));
        let expected = hex(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710"
        ));
        let mut cipher = Cbc::new(&key, iv);
        let mut output = Vec::new();
        for chunk in input.chunks(7) {
            output.extend(cipher.push(chunk));
        }
        output.extend(cipher.push(&[0; 16]));
        assert_eq!(output, expected);
        assert!(cipher.finish().is_err(), "无有效填充的分片须拒绝");
        let mut incomplete = Cbc::new(&key, iv);
        incomplete.push(&input[..15]);
        assert!(incomplete.finish().is_err());
    }

    #[test]
    fn hls_uses_explicit_iv_or_the_actual_media_sequence_and_rejects_drm() {
        let implicit = Encryption::parse(
            "METHOD=AES-128,URI=\"key.bin\"",
            "https://media.example/show/list.m3u8",
        )
        .unwrap()
        .unwrap();
        assert_eq!(implicit.segment(37).iv, 37_u128.to_be_bytes());
        assert_eq!(
            implicit.segment(37).key_url,
            "https://media.example/show/key.bin"
        );
        let explicit = Encryption::parse(
            "METHOD=AES-128,URI=\"key.bin\",IV=0x0123456789abcdef0123456789abcdef",
            "https://media.example/list.m3u8",
        )
        .unwrap()
        .unwrap();
        assert_eq!(explicit.segment(0).iv, explicit.segment(99).iv);
        assert!(
            Encryption::parse(
                "METHOD=SAMPLE-AES,URI=\"key.bin\"",
                "https://media.example/list.m3u8"
            )
            .is_err()
        );
        assert!(
            Encryption::parse(
                "METHOD=AES-128,URI=\"key.bin\",IV=0x123",
                "https://media.example/list.m3u8"
            )
            .is_err()
        );
        assert!(
            Encryption::parse("METHOD=NONE", "https://media.example/list.m3u8")
                .unwrap()
                .is_none()
        );
    }
}
