use crate::manga_image::AesCbc;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub struct LunarReader;

impl LunarReader {
    pub fn extract_seed_objects(html: &str) -> Vec<BTreeMap<String, String>> {
        let mut seeds = Vec::new();
        let mut rest = html;
        while let Some(start) = rest.find("self.__next_f.push([1,\"") {
            rest = &rest[start + "self.__next_f.push([1,\"".len()..];
            let Some(end) = rest.find("\"])") else {
                break;
            };
            let segment = rest[..end].replace("\\\\", "\\").replace("\\\"", "\"");
            let mut scan = segment.as_str();
            while let Some(open) = scan.find('{') {
                scan = &scan[open..];
                let Some(close) = scan.find('}') else {
                    break;
                };
                let candidate = &scan[..=close];
                if let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(candidate) {
                    if map.keys().any(|key| key.len() == 2) {
                        seeds.push(map);
                    }
                }
                scan = &scan[close + 1..];
            }
            rest = &rest[end + 3..];
        }
        seeds
    }

    pub fn generate_rctx(seed_obj: &BTreeMap<String, String>) -> Option<String> {
        let (_, value) = seed_obj.iter().find(|(key, _)| key.len() == 2)?;
        let reversed = value.chars().rev().collect::<String>();
        let decoded = decode_padded_base64(&reversed)?;
        let decoded = String::from_utf8(decoded).ok()?;
        let mut parts = decoded.split('.');
        let xor_key = i32::from_str_radix(parts.next()?, 16).ok()?;
        let hex = parts
            .filter_map(|key| seed_obj.get(key))
            .cloned()
            .collect::<String>();
        let a = hex
            .as_bytes()
            .chunks(2)
            .enumerate()
            .filter_map(|(index, chunk)| {
                let byte = std::str::from_utf8(chunk).ok()?;
                let value = u8::from_str_radix(byte, 16).ok()?;
                Some(value ^ ((xor_key + index as i32 * 7 + 3) as u8))
            })
            .collect::<Vec<_>>();
        if a.is_empty() {
            return Some(String::new());
        }
        let mut rand = KotlinXorWow::new(a.len() as i64);
        let mut h = (0u8..=255).collect::<Vec<_>>();
        for i in (1..=255usize).rev() {
            let j = rand.next_int_bound(i + 1);
            h.swap(i, j);
        }
        let mut s = vec![0u8; 256];
        for (index, value) in h.iter().enumerate() {
            s[*value as usize] = index as u8;
        }
        let u = (0..a.len())
            .map(|_| rand.next_int_bound(256) as u8)
            .collect::<Vec<_>>();
        let mut d = a;
        for round in 0..3usize {
            for t in 0..d.len() {
                d[t] ^= u[(t + 7 * round) % u.len()];
                d[t] = h[d[t] as usize];
                let shift = ((t + 3 * round + 1) % 7 + 1) as u32;
                d[t] = d[t].rotate_left(shift);
            }
            for t in 1..d.len() {
                d[t] ^= d[t - 1];
            }
        }
        let mut e = d;
        for round in (0..3usize).rev() {
            for t in (1..e.len()).rev() {
                e[t] ^= e[t - 1];
            }
            for t in 0..e.len() {
                let shift = ((t + 3 * round + 1) % 7 + 1) as u32;
                e[t] = e[t].rotate_right(shift);
                e[t] = s[e[t] as usize];
                e[t] ^= u[(t + 7 * round) % u.len()];
            }
        }
        String::from_utf8(e).ok()
    }

    pub fn generate_token(
        rctx0: &str,
        rctx1: &str,
        slug: &str,
        chapter_number: &str,
        unix_seconds: i64,
    ) -> Option<String> {
        let xor_key = xor_repeating(rctx0.as_bytes(), rctx1.as_bytes())?;
        let mut rng = KotlinXorWow::new(unix_seconds);
        let rand = (0..8)
            .map(|_| RAND_ALPHABET[rng.next_int_bound(RAND_ALPHABET.len())] as char)
            .collect::<String>();
        let payload = format!("{:x}|{rand}|{slug}|{chapter_number}", unix_seconds);
        let encrypted = payload
            .as_bytes()
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ xor_key[index % xor_key.len()])
            .collect::<Vec<_>>();
        Some(URL_SAFE_NO_PAD.encode(encrypted))
    }

    pub fn decrypt_session_images(session_data: &str, rctx0: &str) -> Option<Vec<String>> {
        let cipher_text = decode_base64_url(session_data)?;
        let key = Sha256::digest(rctx0.as_bytes());
        let decrypted = AesCbc::decrypt_256_pkcs7(&cipher_text, &key, &[0u8; 16])?;
        let value = serde_json::from_slice::<Value>(&decrypted).ok()?;
        Some(
            value
                .get("data")?
                .get("images")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect(),
        )
    }
}

fn decode_padded_base64(input: &str) -> Option<Vec<u8>> {
    let mut value = input.to_string();
    while value.len() % 4 != 0 {
        value.push('=');
    }
    STANDARD.decode(value).ok()
}

fn decode_base64_url(input: &str) -> Option<Vec<u8>> {
    let mut value = input.replace('-', "+").replace('_', "/");
    while value.len() % 4 != 0 {
        value.push('=');
    }
    STANDARD.decode(value).ok()
}

fn xor_repeating(a: &[u8], b: &[u8]) -> Option<Vec<u8>> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some(
        (0..a.len().max(b.len()))
            .map(|index| a[index % a.len()] ^ b[index % b.len()])
            .collect(),
    )
}

const RAND_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

struct KotlinXorWow {
    x: i32,
    y: i32,
    z: i32,
    w: i32,
    v: i32,
    addend: i32,
}

impl KotlinXorWow {
    fn new(seed: i64) -> Self {
        let seed1 = seed as i32;
        let seed2 = (seed >> 32) as i32;
        let mut random = Self {
            x: seed1,
            y: seed2,
            z: 0,
            w: 0,
            v: !seed1,
            addend: (seed1 << 10) ^ ((seed2 as u32 >> 4) as i32),
        };
        for _ in 0..64 {
            random.next_i32();
        }
        random
    }

    fn next_i32(&mut self) -> i32 {
        let t = self.x;
        self.x = self.y;
        self.y = self.z;
        self.z = self.w;
        let v0 = self.v;
        self.w = v0;
        let t = t ^ ((t as u32 >> 2) as i32);
        let v1 = v0 ^ (v0 << 4) ^ (t ^ (t << 1));
        self.v = v1;
        self.addend = self.addend.wrapping_add(362437);
        v1.wrapping_add(self.addend)
    }

    fn next_bits(&mut self, bit_count: u32) -> i32 {
        ((self.next_i32() as u32) >> (32 - bit_count)) as i32
    }

    fn next_int_bound(&mut self, bound: usize) -> usize {
        if bound <= 1 {
            return 0;
        }
        let n = bound as i32;
        if n & -n == n {
            return (((n as i64) * (self.next_bits(31) as i64)) >> 31) as usize;
        }
        loop {
            let bits = self.next_bits(31);
            let value = bits % n;
            if bits - value + (n - 1) >= 0 {
                return value as usize;
            }
        }
    }
}
