use aes::{Aes128, Aes192, Aes256};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use rsa::{
    Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
};
use serde::Deserialize;

type Aes128CbcDec = Decryptor<Aes128>;
type Aes192CbcDec = Decryptor<Aes192>;
type Aes256CbcDec = Decryptor<Aes256>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirgoKeyPair {
    pub public_pem: String,
    pub private_der_base64: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirgoPages {
    pub base_url: String,
    pub path_prefix: String,
    pub files: Vec<String>,
}

pub struct VirgoCrypto;

impl VirgoCrypto {
    pub fn key_pair_for(seed_text: &str) -> Option<VirgoKeyPair> {
        let private = deterministic_private_key(seed_text, 512)?;
        let public_der = RsaPublicKey::from(&private).to_public_key_der().ok()?;
        let private_der = private.to_pkcs8_der().ok()?;
        let public_base64 = STANDARD.encode(public_der.as_bytes());
        Some(VirgoKeyPair {
            public_pem: format!(
                "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
                public_base64
            ),
            private_der_base64: URL_SAFE_NO_PAD.encode(private_der.as_bytes()),
        })
    }

    pub fn decrypt_pages(body: &str, private_der_base64: &str) -> Option<VirgoPages> {
        let encrypted = serde_json::from_str::<EncryptedVirgoPayload>(body).ok()?;
        let private_der = URL_SAFE_NO_PAD.decode(private_der_base64).ok()?;
        let private = RsaPrivateKey::from_pkcs8_der(&private_der).ok()?;
        let key = private
            .decrypt(Pkcs1v15Encrypt, &STANDARD.decode(encrypted.ek).ok()?)
            .ok()?;
        let iv = STANDARD.decode(encrypted.bi).ok()?;
        let cipher_text = STANDARD.decode(encrypted.data).ok()?;
        let decrypted = aes_cbc_decrypt(&cipher_text, &key, &iv)?;
        let decrypted = String::from_utf8(decrypted).ok()?;
        let pages = serde_json::from_str::<DecryptedVirgoPages>(&decrypted).ok()?;
        Some(VirgoPages {
            base_url: pages.location.base,
            path_prefix: pages.location.st,
            files: pages.images.into_iter().map(|image| image.file).collect(),
        })
    }
}

fn deterministic_private_key(seed_text: &str, bits: usize) -> Option<RsaPrivateKey> {
    let mut seed = [0u8; 32];
    for (index, byte) in seed_text.as_bytes().iter().enumerate() {
        seed[index % 32] = seed[index % 32].wrapping_mul(31).wrapping_add(*byte);
    }
    let mut rng = ChaCha20Rng::from_seed(seed);
    RsaPrivateKey::new(&mut rng, bits).ok()
}

fn aes_cbc_decrypt(cipher_text: &[u8], key: &[u8], iv: &[u8]) -> Option<Vec<u8>> {
    match key.len() {
        16 => Aes128CbcDec::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(cipher_text)
            .ok(),
        24 => Aes192CbcDec::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(cipher_text)
            .ok(),
        32 => Aes256CbcDec::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(cipher_text)
            .ok(),
        _ => None,
    }
}

#[derive(Deserialize)]
struct EncryptedVirgoPayload {
    bi: String,
    ek: String,
    data: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DecryptedVirgoPages {
    images: Vec<DecryptedVirgoImage>,
    location: DecryptedVirgoLocation,
}

#[derive(Deserialize)]
struct DecryptedVirgoImage {
    file: String,
}

#[derive(Deserialize)]
struct DecryptedVirgoLocation {
    base: String,
    st: String,
}
