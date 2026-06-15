use base64::{Engine as _, engine::general_purpose::STANDARD};
use blake2::{
    Blake2b,
    digest::{Digest, consts::U32},
};
use flate2::read::ZlibDecoder;
use prost::Message;
use sha1::Sha1;
use std::{
    fmt,
    io::{Cursor, Read},
};

type Blake2b256 = Blake2b<U32>;

pub type ReaderResult<T> = Result<T, ReaderError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderError {
    InvalidInput(String),
    Unsupported(String),
    Decode(String),
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReaderError::InvalidInput(message) => f.write_str(message),
            ReaderError::Unsupported(message) => f.write_str(message),
            ReaderError::Decode(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ReaderError {}

#[derive(Clone, PartialEq, Message)]
pub struct E4PQSTicket {
    #[prost(int32, tag = "1")]
    pub r#type: i32,
    #[prost(string, tag = "2")]
    pub content_id: String,
    #[prost(string, tag = "3")]
    pub consumer: String,
    #[prost(message, optional, tag = "4")]
    pub expires: Option<Timestamp>,
    #[prost(message, optional, tag = "5")]
    pub child: Option<E4PQSWrapper>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Timestamp {
    #[prost(int64, tag = "1")]
    pub seconds: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct E4PQSWrapper {
    #[prost(int32, tag = "1")]
    pub r#type: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub iv: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub checksum: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,
    #[prost(int32, tag = "5")]
    pub data_type: i32,
    #[prost(int32, tag = "6")]
    pub dict_checksum: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ProtoPub {
    #[prost(message, repeated, tag = "2")]
    pub spine: Vec<Link>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Link {
    #[prost(message, repeated, tag = "1")]
    pub variants: Vec<Variant>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Variant {
    #[prost(string, tag = "1")]
    pub link: String,
    #[prost(message, optional, tag = "2")]
    pub image: Option<ImageProps>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ImageProps {
    #[prost(message, optional, tag = "3")]
    pub drm: Option<EDRM>,
}

#[derive(Clone, PartialEq, Message)]
pub struct EDRM {
    #[prost(int32, tag = "1")]
    pub version: i32,
    #[prost(bytes = "vec", tag = "3")]
    pub iv: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedManifest {
    pub r#pub: ProtoPub,
    pub pbex_seed: Option<Vec<u8>>,
}

pub mod ticket_type {
    pub const PLAIN_UNSPECIFIED: i32 = 0;
    pub const TDRM_V1: i32 = 2;
}

pub mod wrapper_type {
    pub const PLAIN_UNSPECIFIED: i32 = 0;
    pub const CDRM_V1: i32 = 2;
}

pub mod data_type {
    pub const PROTOPUB: i32 = 2;
    pub const PROTOPUB_ZLIB: i32 = 5;
}

pub mod edrm_version {
    pub const XEBP: i32 = 2;
}

pub fn decode_manifest_full(ticket: E4PQSTicket) -> ReaderResult<DecodedManifest> {
    let wrapper = match ticket.r#type {
        ticket_type::PLAIN_UNSPECIFIED => ticket.child.clone().unwrap_or_default(),
        ticket_type::TDRM_V1 => unwrap_tdrm_v1(&ticket)?,
        other => {
            return Err(ReaderError::Unsupported(format!(
                "unsupported J-Novel ticket type: {other}"
            )));
        }
    };

    let decrypted = match wrapper.r#type {
        wrapper_type::PLAIN_UNSPECIFIED => wrapper.data.clone(),
        wrapper_type::CDRM_V1 => decrypt_cdrm_v1(&ticket.content_id, &wrapper.iv, &wrapper.data)?,
        other => {
            return Err(ReaderError::Unsupported(format!(
                "unsupported J-Novel wrapper type: {other}"
            )));
        }
    };

    let has_pbex = decrypted.len() >= 52 && decrypted.starts_with(b"PBEX");
    let pbex_seed = has_pbex.then(|| decrypted[4..52].to_vec());
    let payload = if has_pbex {
        &decrypted[52..]
    } else {
        decrypted.as_slice()
    };

    let inflated = match wrapper.data_type {
        data_type::PROTOPUB => payload.to_vec(),
        data_type::PROTOPUB_ZLIB => zlib_inflate(payload)?,
        other => {
            return Err(ReaderError::Unsupported(format!(
                "unsupported J-Novel manifest dataType: {other}"
            )));
        }
    };

    let r#pub = ProtoPub::decode(inflated.as_slice())
        .map_err(|error| ReaderError::Decode(error.to_string()))?;
    Ok(DecodedManifest { r#pub, pbex_seed })
}

fn unwrap_tdrm_v1(ticket: &E4PQSTicket) -> ReaderResult<E4PQSWrapper> {
    let wrapper = ticket.child.clone().unwrap_or_default();
    require_len("TDRM child iv", &wrapper.iv, 32)?;
    if ticket.content_id.len() < 3 {
        return Err(ReaderError::InvalidInput("contentId too short".into()));
    }
    if ticket.consumer.len() < 3 {
        return Err(ReaderError::InvalidInput("consumer too short".into()));
    }

    let rc4_out = derive_tdrm_prefix(ticket, &wrapper)?;
    let blowfish_iv = derive_tdrm_blowfish_iv(ticket);
    let new_iv = blowfish_cbc_decrypt(&rc4_out[..wrapper.iv.len()], &blowfish_iv)?;

    let content = ticket.content_id.as_bytes();
    let v261 = wrapper.data.len().min(123 + usize::from(content[2]));
    let mut new_data = vec![0_u8; wrapper.data.len()];
    new_data[..v261].copy_from_slice(&rc4_out[wrapper.iv.len()..wrapper.iv.len() + v261]);
    new_data[v261..].copy_from_slice(&wrapper.data[v261..]);

    Ok(E4PQSWrapper {
        r#type: wrapper.r#type,
        iv: new_iv,
        checksum: wrapper.checksum,
        data: new_data,
        data_type: wrapper.data_type,
        dict_checksum: wrapper.dict_checksum,
    })
}

fn derive_tdrm_blowfish_iv(ticket: &E4PQSTicket) -> [u8; 8] {
    let expires = ticket.expires.as_ref().map_or(0, |expires| expires.seconds);
    let content = ticket.content_id.as_bytes();
    let mut tweak = [0_u8; 8];
    tweak[0] = (expires % 100) as u8;
    tweak[1] = ((expires / 100) % 100) as u8;
    tweak[2] = ((expires / 10_000) % 100) as u8;
    tweak[3] = ((expires / 1_000_000) % 100) as u8;
    tweak[4] = ((expires / 100_000_000) % 100) as u8;
    tweak[5] = content[content.len() - 1];
    tweak[6] = content[content.len() - 2];
    tweak[7] = content[content.len() - 3];
    xor_mask_a6(&mut tweak, 0);

    let vf124 = vf124();
    let mut hasher = Sha1::new();
    Digest::update(&mut hasher, &vf124);
    Digest::update(&mut hasher, tweak);
    let digest = hasher.finalize();
    let mut iv = [0_u8; 8];
    iv.copy_from_slice(&digest[7..15]);
    xor_mask_a6(&mut iv, 3);
    iv
}

fn derive_tdrm_prefix(ticket: &E4PQSTicket, wrapper: &E4PQSWrapper) -> ReaderResult<Vec<u8>> {
    let expires = ticket.expires.as_ref().map_or(0, |expires| expires.seconds);
    let content = ticket.content_id.as_bytes();
    let mut tweak = [0_u8; 8];
    tweak[0] = (expires % 100) as u8;
    tweak[1] = ((expires / 100) % 100) as u8;
    tweak[2] = ((expires / 10_000) % 100) as u8;
    tweak[3] = ((expires / 1_000_000) % 100) as u8;
    tweak[4] = ((expires / 100_000_000) % 100) as u8;
    tweak[5] = content[content.len() - 1];
    tweak[6] = content[content.len() - 2];
    tweak[7] = content[content.len() - 3];
    xor_mask_a6(&mut tweak, 0);

    let vf124 = vf124();
    let mut rc4_key =
        Vec::with_capacity(ticket.consumer.len() + ticket.content_id.len() + vf124.len());
    rc4_key.extend_from_slice(ticket.consumer.as_bytes());
    rc4_key.extend_from_slice(ticket.content_id.as_bytes());
    rc4_key.extend_from_slice(&vf124);
    if rc4_key.len() > 256 {
        rc4_key.truncate(256);
    }

    let v261 = wrapper.data.len().min(123 + usize::from(content[2]));
    let mut rc4_input = Vec::with_capacity(wrapper.iv.len() + v261);
    rc4_input.extend_from_slice(&wrapper.iv);
    rc4_input.extend_from_slice(&wrapper.data[..v261]);
    Ok(rc4(&rc4_key, &rc4_input, 769))
}

fn decrypt_cdrm_v1(content_id: &str, iv: &[u8], data: &[u8]) -> ReaderResult<Vec<u8>> {
    require_len("CDRM iv", iv, 32)?;
    let key = derive_cdrm_key(content_id, iv);
    let nonce = &iv[16..24];
    let initial_counter = u64::from(iv[24]);
    chacha8_decrypt(data, &key, nonce, initial_counter)
}

fn derive_cdrm_key(content_id: &str, iv: &[u8]) -> [u8; 32] {
    const ALPHABET: &[u8; 64] = b"R5zRO0qEKFDfaP3OrLIbbQkjrcwWdgb4f7k6LLJjehQtvTrNXuzLp2_NT-eRnHK1";
    const FINAL_MASK: [u8; 7] = [0xD9, 0xAD, 0xBE, 0xEF, 0xC0, 0xDE, 0xAD];

    let content = content_id.as_bytes();
    let e = content.len().min(24);
    let mut material = [0_u8; 64];
    material[..e].copy_from_slice(&content[..e]);
    material[e..].copy_from_slice(&ALPHABET[..64 - e]);

    let mut key = {
        let mut hasher = Blake2b256::new();
        Digest::update(&mut hasher, material);
        let digest = hasher.finalize();
        let mut key = [0_u8; 32];
        key.copy_from_slice(&digest);
        key
    };

    for c in 0..16 {
        key[2 * c] ^= iv[c];
    }

    let ivk = iv[25];
    let ivl = iv[26];
    let ivm = iv[27];
    let ivn = iv[28];
    let ivo = iv[29];
    let ivp = iv[30];
    let ivq = iv[31];
    key[usize::from(ivq & 15) + 2] ^= ivk;
    key[usize::from(ivp & 15) + 5] ^= ivl;
    key[usize::from(ivo & 15) + 5] ^= ivm;
    key[usize::from(ivn & 15) + 6] ^= ivn;
    key[usize::from(ivm & 15) + 5] ^= ivo;
    key[usize::from(ivl & 15)] ^= ivp;
    key[usize::from(ivk & 15)] ^= ivq;

    for c in 0..32 {
        key[c] ^= FINAL_MASK[(c + 3) % 7];
    }
    key
}

fn zlib_inflate(data: &[u8]) -> ReaderResult<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|error| ReaderError::Decode(format!("zlib inflate failed: {error}")))?;
    Ok(out)
}

pub const QSC_DIR_SIZE: usize = 4096;
const QSC_ENTRY_COUNT: usize = 127;
const QSC_ENTRY_SIZE: usize = 32;
const QSC_MAGIC: &[u8; 8] = b"E4PQSC\x01\x00";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QscEntry {
    pub four_cc: String,
    pub size: u32,
    pub name: String,
    pub offset: u32,
}

pub fn qsc_find_entry(directory: &[u8], name: &str) -> ReaderResult<Option<QscEntry>> {
    if directory.len() < QSC_DIR_SIZE {
        return Err(ReaderError::InvalidInput(format!(
            "QSC directory must be at least {QSC_DIR_SIZE} bytes, got {}",
            directory.len()
        )));
    }
    if &directory[..QSC_MAGIC.len()] != QSC_MAGIC {
        return Err(ReaderError::InvalidInput("invalid QSC magic".into()));
    }

    let mut running_offset = 0_u32;
    for index in 0..QSC_ENTRY_COUNT {
        let base = 32 + index * QSC_ENTRY_SIZE;
        let size = le_u32(directory, base + 4)?;
        if size == 0 {
            break;
        }

        let four_cc = String::from_utf8_lossy(&directory[base..base + 4]).into_owned();
        let raw_name = &directory[base + 8..base + 32];
        let end = raw_name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(raw_name.len());
        let entry_name = String::from_utf8_lossy(&raw_name[..end]).into_owned();
        if entry_name == name {
            return Ok(Some(QscEntry {
                four_cc,
                size,
                name: entry_name,
                offset: running_offset,
            }));
        }
        running_offset = running_offset.wrapping_add(size);
    }
    Ok(None)
}

pub fn strip_to_webp(xebp: &[u8]) -> Vec<u8> {
    if xebp.len() < 20 || &xebp[..4] != b"RIFF" {
        return xebp.to_vec();
    }

    let mut off = 12_usize;
    while off + 8 <= xebp.len() {
        let chunk_size = match read_u32_le_raw(xebp, off + 4) {
            Some(size) => size as usize,
            None => return xebp.to_vec(),
        };
        let payload_start = off + 8;
        let chunk_end = match payload_start
            .checked_add(chunk_size)
            .and_then(|end| end.checked_add(chunk_size & 1))
        {
            Some(end) if end <= xebp.len() => end,
            _ => return xebp.to_vec(),
        };
        if matches!(&xebp[off..off + 4], b"VP8 " | b"VP8L" | b"VP8X") {
            let mut out = xebp[..chunk_end].to_vec();
            let new_riff_size = (chunk_end - 8) as u32;
            out[4..8].copy_from_slice(&new_riff_size.to_le_bytes());
            out[8..12].copy_from_slice(b"WEBP");
            return out;
        }
        off = chunk_end;
    }

    xebp.to_vec()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XebpContext {
    pub iv: Vec<u8>,
    pub content_id: String,
    pub consumer_id: Vec<u8>,
    pub pbex_seed: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZstrMeta {
    pub version: i32,
    pub nonce: [u8; 8],
    pub xor_len: usize,
}

pub fn parse_zstr(zstr: &[u8]) -> ReaderResult<ZstrMeta> {
    const XXTEA_KEY: [u8; 16] = [
        0x6c, 0xa8, 0x7b, 0x0f, 0xa8, 0x51, 0x3e, 0x36, 0x16, 0x53, 0x47, 0xaf, 0x5d, 0xe5, 0x19,
        0x89,
    ];

    let text = String::from_utf8_lossy(zstr);
    let text = text.trim_end_matches('\0');
    let mut lines = text.split('\n');
    let version = lines
        .next()
        .ok_or_else(|| ReaderError::InvalidInput("ZSTR missing version".into()))?
        .trim()
        .parse::<i32>()
        .map_err(|error| ReaderError::InvalidInput(format!("invalid ZSTR version: {error}")))?;
    if version != 1 {
        return Err(ReaderError::Unsupported(format!(
            "unsupported ZSTR version: {version}"
        )));
    }

    let raw_count = lines
        .next()
        .ok_or_else(|| ReaderError::InvalidInput("ZSTR missing xor length".into()))?
        .trim()
        .parse::<usize>()
        .map_err(|error| ReaderError::InvalidInput(format!("invalid ZSTR xor length: {error}")))?;
    let xor_len = (raw_count ^ 65) & 0xffff;

    let encrypted_hex = lines
        .next()
        .ok_or_else(|| ReaderError::InvalidInput("ZSTR missing nonce payload".into()))?
        .trim();
    if encrypted_hex.is_empty() || encrypted_hex.len() % 8 != 0 {
        return Err(ReaderError::InvalidInput(format!(
            "ZSTR encrypted hex must encode whole u32 words, got {} chars",
            encrypted_hex.len()
        )));
    }

    let encrypted = decode_hex(encrypted_hex)?;
    let mut words = encrypted
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if words.len() < 2 {
        return Err(ReaderError::InvalidInput(format!(
            "ZSTR needs at least 2 u32 words, got {}",
            words.len()
        )));
    }

    xxtea_decrypt_words(&mut words, &XXTEA_KEY)?;
    let real_len = words
        .last()
        .copied()
        .ok_or_else(|| ReaderError::InvalidInput("ZSTR decrypted payload is empty".into()))?
        as usize;
    let available = (words.len() - 1) * 4;
    if real_len == 0 || real_len > available {
        return Err(ReaderError::InvalidInput(format!(
            "ZSTR length tag out of range: {real_len} > {available}"
        )));
    }

    let mut decrypted = Vec::with_capacity(real_len);
    for index in 0..real_len {
        decrypted.push((words[index >> 2] >> ((index & 3) << 3)) as u8);
    }
    let mut nonce = [0_u8; 8];
    let copy_len = decrypted.len().min(nonce.len());
    nonce[..copy_len].copy_from_slice(&decrypted[..copy_len]);

    Ok(ZstrMeta {
        version,
        nonce,
        xor_len,
    })
}

pub fn xxtea_decrypt_words(words: &mut [u32], key_bytes: &[u8]) -> ReaderResult<()> {
    const DELTA: u32 = 0x9E37_79B9;

    if words.len() < 2 {
        return Ok(());
    }
    require_len("XXTEA key", key_bytes, 16)?;
    let key = [
        u32::from_le_bytes([key_bytes[0], key_bytes[1], key_bytes[2], key_bytes[3]]),
        u32::from_le_bytes([key_bytes[4], key_bytes[5], key_bytes[6], key_bytes[7]]),
        u32::from_le_bytes([key_bytes[8], key_bytes[9], key_bytes[10], key_bytes[11]]),
        u32::from_le_bytes([key_bytes[12], key_bytes[13], key_bytes[14], key_bytes[15]]),
    ];

    let n = words.len();
    let rounds = 6 + 52 / n;
    let mut sum = (rounds as u32).wrapping_mul(DELTA);
    let mut y = words[0];
    for _ in 0..rounds {
        let e = (sum >> 2) & 3;
        for p in (1..n).rev() {
            let z = words[p - 1];
            let mx = (((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4)))
                ^ ((sum ^ y).wrapping_add(key[(p & 3) ^ e as usize] ^ z));
            words[p] = words[p].wrapping_sub(mx);
            y = words[p];
        }
        let z = words[n - 1];
        let mx = (((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4)))
            ^ ((sum ^ y).wrapping_add(key[e as usize] ^ z));
        words[0] = words[0].wrapping_sub(mx);
        y = words[0];
        sum = sum.wrapping_sub(DELTA);
    }

    Ok(())
}

pub fn chacha8_decrypt(
    data: &[u8],
    key: &[u8],
    nonce: &[u8],
    initial_counter: u64,
) -> ReaderResult<Vec<u8>> {
    require_len("ChaCha8 key", key, 32)?;
    require_len("ChaCha8 nonce", nonce, 8)?;

    let mut state = [0_u32; 16];
    state[0] = 0x6170_7865;
    state[1] = 0x3320_646e;
    state[2] = 0x7962_2d32;
    state[3] = 0x6b20_6574;
    for i in 0..8 {
        state[4 + i] =
            u32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
    }
    state[14] = u32::from_le_bytes([nonce[0], nonce[1], nonce[2], nonce[3]]);
    state[15] = u32::from_le_bytes([nonce[4], nonce[5], nonce[6], nonce[7]]);

    let mut out = vec![0_u8; data.len()];
    let mut counter = initial_counter;
    let mut pos = 0;
    while pos < data.len() {
        state[12] = counter as u32;
        state[13] = (counter >> 32) as u32;
        let mut work = state;
        for _ in 0..4 {
            qround(&mut work, 0, 4, 8, 12);
            qround(&mut work, 1, 5, 9, 13);
            qround(&mut work, 2, 6, 10, 14);
            qround(&mut work, 3, 7, 11, 15);
            qround(&mut work, 0, 5, 10, 15);
            qround(&mut work, 1, 6, 11, 12);
            qround(&mut work, 2, 7, 8, 13);
            qround(&mut work, 3, 4, 9, 14);
        }
        for i in 0..16 {
            work[i] = work[i].wrapping_add(state[i]);
        }

        let block_len = (data.len() - pos).min(64);
        for i in 0..block_len {
            let key_stream = (work[i >> 2] >> ((i & 3) << 3)) as u8;
            out[pos + i] = data[pos + i] ^ key_stream;
        }
        pos += block_len;
        counter = counter.wrapping_add(1);
    }
    Ok(out)
}

fn qround(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XebpContainer {
    pub vp8: Vec<u8>,
    pub vp8_riff_as_webp: Vec<u8>,
    pub zstr: Vec<u8>,
    pub tfix: Vec<u8>,
}

pub fn parse_xebp_container(xebp: &[u8]) -> ReaderResult<XebpContainer> {
    if xebp.len() < 20 {
        return Err(ReaderError::InvalidInput(format!(
            "XEBP too small: {}",
            xebp.len()
        )));
    }
    if &xebp[..4] != b"RIFF" {
        return Err(ReaderError::InvalidInput("XEBP missing RIFF magic".into()));
    }

    let mut off = 12_usize;
    let mut vp8 = None;
    let mut vp8_riff_as_webp = None;
    let mut zstr = None;
    let mut tfix = None;
    while off + 8 <= xebp.len() {
        let size = read_u32_le_raw(xebp, off + 4)
            .ok_or_else(|| ReaderError::InvalidInput("truncated RIFF chunk size".into()))?
            as usize;
        let payload_start = off + 8;
        let payload_end = payload_start
            .checked_add(size)
            .ok_or_else(|| ReaderError::InvalidInput("RIFF chunk size overflow".into()))?;
        if payload_end > xebp.len() {
            return Err(ReaderError::InvalidInput(
                "truncated RIFF chunk payload".into(),
            ));
        }
        let padded_end = payload_end + (size & 1);
        match &xebp[off..off + 4] {
            b"VP8 " | b"VP8L" | b"VP8X" => {
                vp8 = Some(xebp[payload_start..payload_end].to_vec());
                let mut out = xebp[..padded_end.min(xebp.len())].to_vec();
                let new_size = (out.len() - 8) as u32;
                out[4..8].copy_from_slice(&new_size.to_le_bytes());
                out[8..12].copy_from_slice(b"WEBP");
                vp8_riff_as_webp = Some(out);
            }
            b"ZSTR" => zstr = Some(xebp[payload_start..payload_end].to_vec()),
            b"tfix" => tfix = Some(xebp[payload_start..payload_end].to_vec()),
            _ => {}
        }
        off = padded_end;
    }

    Ok(XebpContainer {
        vp8: vp8.unwrap_or_default(),
        vp8_riff_as_webp: vp8_riff_as_webp.unwrap_or_default(),
        zstr: zstr.ok_or_else(|| ReaderError::InvalidInput("XEBP missing ZSTR chunk".into()))?,
        tfix: tfix.ok_or_else(|| ReaderError::InvalidInput("XEBP missing tfix chunk".into()))?,
    })
}

pub fn decrypt_xebp_tiff_patch(xebp: &[u8], ctx: &XebpContext) -> ReaderResult<RgbaImage> {
    require_len("XEBP iv", &ctx.iv, 32)?;
    require_len("XEBP consumerId", &ctx.consumer_id, 32)?;
    require_len("XEBP pbexSeed", &ctx.pbex_seed, 48)?;

    let container = parse_xebp_container(xebp)?;
    let zstr = parse_zstr(&container.zstr)?;
    let mut per_image_key = [0_u8; 32];
    for (index, slot) in per_image_key.iter_mut().enumerate() {
        *slot = ctx.pbex_seed[16 + index] ^ ctx.iv[index];
    }

    let mut workspace = container.tfix;
    for byte in workspace.iter_mut().take(zstr.xor_len) {
        *byte ^= 0x11;
    }
    let tiff = chacha8_decrypt(&workspace, &per_image_key, &zstr.nonce, zstr.xor_len as u64)?;
    if !tiff.starts_with(&[0x49, 0x49, 0x2A, 0x00]) {
        let got = tiff
            .iter()
            .take(4)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        return Err(ReaderError::Decode(format!(
            "tfix did not decrypt to TIFF; expected 49492a00, got {got}"
        )));
    }
    tiff_decode(&tiff)
}

pub fn decrypt_xebp(xebp: &[u8], ctx: &XebpContext) -> ReaderResult<Vec<u8>> {
    let container = parse_xebp_container(xebp)?;
    let patch = decrypt_xebp_tiff_patch(xebp, ctx)?;
    composite_patch(&container.vp8_riff_as_webp, &patch)
}

pub fn composite_patch(vp8_webp: &[u8], patch: &RgbaImage) -> ReaderResult<Vec<u8>> {
    let mut base = image::load_from_memory(vp8_webp)
        .map_err(|error| ReaderError::Decode(format!("base WebP decode failed: {error}")))?
        .to_rgba8();
    let width = base.width().min(patch.width as u32);
    let height = base.height().min(patch.height as u32);

    for y in 0..height {
        for x in 0..width {
            let src = ((y as usize * patch.width) + x as usize) * 4;
            base.put_pixel(
                x,
                y,
                image::Rgba([
                    patch.rgba[src],
                    patch.rgba[src + 1],
                    patch.rgba[src + 2],
                    0xff,
                ]),
            );
        }
    }

    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(base)
        .write_to(&mut out, image::ImageFormat::WebP)
        .map_err(|error| ReaderError::Decode(format!("WebP encode failed: {error}")))?;
    Ok(out.into_inner())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub fn tiff_decode(tiff: &[u8]) -> ReaderResult<RgbaImage> {
    const TAG_IMAGE_WIDTH: u16 = 256;
    const TAG_IMAGE_LENGTH: u16 = 257;
    const TAG_BITS_PER_SAMPLE: u16 = 258;
    const TAG_COMPRESSION: u16 = 259;
    const TAG_PHOTOMETRIC: u16 = 262;
    const TAG_STRIP_OFFSETS: u16 = 273;
    const TAG_SAMPLES_PER_PIXEL: u16 = 277;
    const TAG_STRIP_BYTE_COUNTS: u16 = 279;
    const TAG_PLANAR_CONFIG: u16 = 284;
    const TAG_PREDICTOR: u16 = 317;
    const TAG_COLORMAP: u16 = 320;

    if tiff.len() < 8 {
        return Err(ReaderError::InvalidInput(format!(
            "TIFF too small: {}",
            tiff.len()
        )));
    }
    if &tiff[..4] != b"II*\0" {
        return Err(ReaderError::InvalidInput(
            "TIFF must be little-endian II*\\0".into(),
        ));
    }
    let ifd_offset = le_u32(tiff, 4)? as usize;
    let entries = le_u16(tiff, ifd_offset)? as usize;

    let mut width = 0_usize;
    let mut height = 0_usize;
    let mut bits_per_sample = vec![8_i32];
    let mut compression = 1_i32;
    let mut photometric = 2_i32;
    let mut strip_offsets = Vec::new();
    let mut samples_per_pixel = 1_usize;
    let mut strip_byte_counts = Vec::new();
    let mut planar_config = 1_i32;
    let mut predictor = 1_i32;
    let mut color_map = None;

    for index in 0..entries {
        let entry_off = ifd_offset + 2 + index * 12;
        let tag = le_u16(tiff, entry_off)?;
        let field_type = le_u16(tiff, entry_off + 2)?;
        let count = le_u32(tiff, entry_off + 4)? as usize;
        let values = read_tiff_values(tiff, field_type, count, entry_off + 8)?;
        match tag {
            TAG_IMAGE_WIDTH => width = values.first().copied().unwrap_or_default() as usize,
            TAG_IMAGE_LENGTH => height = values.first().copied().unwrap_or_default() as usize,
            TAG_BITS_PER_SAMPLE => bits_per_sample = values,
            TAG_COMPRESSION => compression = values.first().copied().unwrap_or(1),
            TAG_PHOTOMETRIC => photometric = values.first().copied().unwrap_or(2),
            TAG_STRIP_OFFSETS => strip_offsets = values.into_iter().map(|v| v as usize).collect(),
            TAG_SAMPLES_PER_PIXEL => {
                samples_per_pixel = values.first().copied().unwrap_or(1) as usize
            }
            TAG_STRIP_BYTE_COUNTS => {
                strip_byte_counts = values.into_iter().map(|v| v as usize).collect()
            }
            TAG_PLANAR_CONFIG => planar_config = values.first().copied().unwrap_or(1),
            TAG_PREDICTOR => predictor = values.first().copied().unwrap_or(1),
            TAG_COLORMAP => color_map = Some(values),
            _ => {}
        }
    }

    if compression != 1 && compression != 5 {
        return Err(ReaderError::Unsupported(format!(
            "TIFF compression={compression}; only uncompressed/LZW supported"
        )));
    }
    if predictor != 1 && predictor != 2 {
        return Err(ReaderError::Unsupported(format!(
            "TIFF predictor={predictor}; only none/horizontal supported"
        )));
    }
    if planar_config != 1 {
        return Err(ReaderError::Unsupported(format!(
            "TIFF planar={planar_config}; only chunky supported"
        )));
    }
    if width == 0 || height == 0 {
        return Err(ReaderError::InvalidInput("TIFF empty image".into()));
    }
    if strip_offsets.len() != strip_byte_counts.len() {
        return Err(ReaderError::InvalidInput(
            "TIFF strip offset/count length mismatch".into(),
        ));
    }

    let mut raw = Vec::new();
    for (offset, byte_count) in strip_offsets.into_iter().zip(strip_byte_counts) {
        let end = offset
            .checked_add(byte_count)
            .ok_or_else(|| ReaderError::InvalidInput("TIFF strip range overflow".into()))?;
        if end > tiff.len() {
            return Err(ReaderError::InvalidInput("TIFF strip out of bounds".into()));
        }
        let strip = &tiff[offset..end];
        if compression == 5 {
            raw.extend_from_slice(&decompress_lzw(strip)?);
        } else {
            raw.extend_from_slice(strip);
        }
    }

    if predictor == 2 {
        let row_bytes = width * samples_per_pixel;
        for row in 0..height {
            let row_start = row * row_bytes;
            for x in 1..width {
                for c in 0..samples_per_pixel {
                    let index = row_start + x * samples_per_pixel + c;
                    let prev = raw[row_start + (x - 1) * samples_per_pixel + c];
                    raw[index] = raw[index].wrapping_add(prev);
                }
            }
        }
    }

    match photometric {
        0 | 1 => decode_grayscale_tiff(
            width,
            height,
            &bits_per_sample,
            samples_per_pixel,
            &raw,
            photometric == 0,
        ),
        2 => decode_rgb_tiff(width, height, &bits_per_sample, samples_per_pixel, &raw),
        3 => decode_palette_tiff(
            width,
            height,
            &bits_per_sample,
            samples_per_pixel,
            &raw,
            color_map
                .ok_or_else(|| ReaderError::InvalidInput("palette TIFF missing ColorMap".into()))?,
        ),
        other => Err(ReaderError::Unsupported(format!(
            "TIFF photometric={other} not supported"
        ))),
    }
}

fn decode_rgb_tiff(
    width: usize,
    height: usize,
    bits_per_sample: &[i32],
    samples_per_pixel: usize,
    raw: &[u8],
) -> ReaderResult<RgbaImage> {
    if !bits_per_sample.iter().all(|bits| *bits == 8) {
        return Err(ReaderError::Unsupported(format!(
            "RGB TIFF only supports 8 bits/sample, got {bits_per_sample:?}"
        )));
    }
    if samples_per_pixel != 3 && samples_per_pixel != 4 {
        return Err(ReaderError::Unsupported(format!(
            "RGB TIFF samplesPerPixel must be 3 or 4, got {samples_per_pixel}"
        )));
    }
    let pixels = width * height;
    if raw.len() < pixels * samples_per_pixel {
        return Err(ReaderError::InvalidInput(
            "RGB TIFF raw data truncated".into(),
        ));
    }
    let mut rgba = vec![0_u8; pixels * 4];
    for pixel in 0..pixels {
        let src = pixel * samples_per_pixel;
        let dst = pixel * 4;
        rgba[dst] = raw[src];
        rgba[dst + 1] = raw[src + 1];
        rgba[dst + 2] = raw[src + 2];
        rgba[dst + 3] = if samples_per_pixel == 4 {
            raw[src + 3]
        } else {
            0xff
        };
    }
    Ok(RgbaImage {
        width,
        height,
        rgba,
    })
}

fn decode_grayscale_tiff(
    width: usize,
    height: usize,
    bits_per_sample: &[i32],
    samples_per_pixel: usize,
    raw: &[u8],
    invert: bool,
) -> ReaderResult<RgbaImage> {
    if bits_per_sample != [8] {
        return Err(ReaderError::Unsupported(
            "grayscale TIFF only supports 8 bits/sample".into(),
        ));
    }
    if samples_per_pixel != 1 && samples_per_pixel != 2 {
        return Err(ReaderError::Unsupported(format!(
            "grayscale TIFF samplesPerPixel must be 1 or 2, got {samples_per_pixel}"
        )));
    }
    let pixels = width * height;
    if raw.len() < pixels * samples_per_pixel {
        return Err(ReaderError::InvalidInput(
            "grayscale TIFF raw data truncated".into(),
        ));
    }
    let mut rgba = vec![0_u8; pixels * 4];
    for pixel in 0..pixels {
        let src = pixel * samples_per_pixel;
        let dst = pixel * 4;
        let value = if invert { !raw[src] } else { raw[src] };
        rgba[dst] = value;
        rgba[dst + 1] = value;
        rgba[dst + 2] = value;
        rgba[dst + 3] = if samples_per_pixel == 2 {
            raw[src + 1]
        } else {
            0xff
        };
    }
    Ok(RgbaImage {
        width,
        height,
        rgba,
    })
}

fn decode_palette_tiff(
    width: usize,
    height: usize,
    bits_per_sample: &[i32],
    samples_per_pixel: usize,
    raw: &[u8],
    color_map: Vec<i32>,
) -> ReaderResult<RgbaImage> {
    if bits_per_sample != [8] {
        return Err(ReaderError::Unsupported(
            "palette TIFF only supports 8 bits/sample".into(),
        ));
    }
    if samples_per_pixel != 1 {
        return Err(ReaderError::Unsupported(format!(
            "palette TIFF samplesPerPixel must be 1, got {samples_per_pixel}"
        )));
    }
    let colors = 1_usize << bits_per_sample[0];
    if color_map.len() != 3 * colors {
        return Err(ReaderError::InvalidInput(format!(
            "TIFF ColorMap size mismatch: {} vs {}",
            color_map.len(),
            3 * colors
        )));
    }

    let pixels = width * height;
    if raw.len() < pixels {
        return Err(ReaderError::InvalidInput(
            "palette TIFF raw data truncated".into(),
        ));
    }
    let mut rgba = vec![0_u8; pixels * 4];
    for (pixel, raw_index) in raw.iter().enumerate().take(pixels) {
        let index = *raw_index as usize;
        let dst = pixel * 4;
        rgba[dst] = (color_map[index] >> 8) as u8;
        rgba[dst + 1] = (color_map[colors + index] >> 8) as u8;
        rgba[dst + 2] = (color_map[2 * colors + index] >> 8) as u8;
        rgba[dst + 3] = 0xff;
    }
    Ok(RgbaImage {
        width,
        height,
        rgba,
    })
}

fn read_tiff_values(
    tiff: &[u8],
    field_type: u16,
    count: usize,
    value_off: usize,
) -> ReaderResult<Vec<i32>> {
    let elem_size = match field_type {
        1 | 2 => 1,
        3 => 2,
        4 => 4,
        5 => 8,
        other => {
            return Err(ReaderError::Unsupported(format!(
                "unsupported TIFF field type: {other}"
            )));
        }
    };
    let total_size = elem_size * count;
    let base = if total_size <= 4 {
        value_off
    } else {
        le_u32(tiff, value_off)? as usize
    };

    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        values.push(match field_type {
            1 => *tiff
                .get(base + index)
                .ok_or_else(|| ReaderError::InvalidInput("TIFF BYTE out of bounds".into()))?
                as i32,
            3 => le_u16(tiff, base + index * 2)? as i32,
            4 => le_u32(tiff, base + index * 4)? as i32,
            _ => {
                return Err(ReaderError::Unsupported(format!(
                    "unexpected TIFF field type in read_tiff_values: {field_type}"
                )));
            }
        });
    }
    Ok(values)
}

fn decompress_lzw(input: &[u8]) -> ReaderResult<Vec<u8>> {
    const CLEAR_CODE: usize = 256;
    const EOI_CODE: usize = 257;

    let mut out = Vec::new();
    let mut dict = (0..=255)
        .map(|value| vec![value as u8])
        .collect::<Vec<Vec<u8>>>();
    dict.push(Vec::new());
    dict.push(Vec::new());

    let mut bit_buf = 0_u64;
    let mut bit_count = 0_usize;
    let mut input_pos = 0_usize;
    let mut code_width = 9_usize;
    let mut prev_code = None::<usize>;

    loop {
        while bit_count < code_width {
            if input_pos >= input.len() {
                return Ok(out);
            }
            bit_buf = (bit_buf << 8) | u64::from(input[input_pos]);
            input_pos += 1;
            bit_count += 8;
        }
        let code = ((bit_buf >> (bit_count - code_width)) & ((1_u64 << code_width) - 1)) as usize;
        bit_count -= code_width;

        if code == EOI_CODE {
            break;
        }
        if code == CLEAR_CODE {
            dict.truncate(258);
            code_width = 9;
            prev_code = None;
            continue;
        }

        let entry = if code < dict.len() {
            dict[code].clone()
        } else if code == dict.len() {
            let previous = prev_code.ok_or_else(|| {
                ReaderError::Decode(format!("LZW invalid code {code} without previous code"))
            })?;
            let mut entry = dict[previous].clone();
            entry.push(entry[0]);
            entry
        } else {
            return Err(ReaderError::Decode(format!(
                "LZW invalid code {code} (dict size {}, prev {prev_code:?})",
                dict.len()
            )));
        };
        out.extend_from_slice(&entry);

        if let Some(previous) = prev_code
            && dict.len() < 4096
        {
            let mut new_entry = dict[previous].clone();
            new_entry.push(entry[0]);
            dict.push(new_entry);
            if dict.len() == (1_usize << code_width) - 1 && code_width < 12 {
                code_width += 1;
            }
        }
        prev_code = Some(code);
    }

    Ok(out)
}

fn rc4(key: &[u8], data: &[u8], skip: usize) -> Vec<u8> {
    let mut s = [0_u8; 256];
    for (index, slot) in s.iter_mut().enumerate() {
        *slot = index as u8;
    }

    let mut j = 0_usize;
    for i in 0..256 {
        j = (j + usize::from(s[i]) + usize::from(key[i % key.len()])) & 0xff;
        s.swap(i, j);
    }

    let mut i = 0_usize;
    j = 0;
    for _ in 0..skip {
        i = (i + 1) & 0xff;
        j = (j + usize::from(s[i])) & 0xff;
        s.swap(i, j);
        let _ = s[(usize::from(s[i]) + usize::from(s[j])) & 0xff];
    }

    let mut out = Vec::with_capacity(data.len());
    for byte in data {
        i = (i + 1) & 0xff;
        j = (j + usize::from(s[i])) & 0xff;
        s.swap(i, j);
        let key_byte = s[(usize::from(s[i]) + usize::from(s[j])) & 0xff];
        out.push(*byte ^ key_byte);
    }
    out
}

fn vf124() -> Vec<u8> {
    rc4(
        b"error",
        &[
            0x8F, 0x08, 0xBE, 0x6C, 0x0F, 0xDE, 0x6A, 0xF8, 0x20, 0xED, 0x7E, 0xAF, 0x0E, 0x52,
            0xDD, 0x9D,
        ],
        771,
    )
}

struct BlowfishTables {
    p: [u32; 18],
    s: [[u32; 256]; 4],
}

fn blowfish_cbc_decrypt(ciphertext: &[u8], iv: &[u8; 8]) -> ReaderResult<Vec<u8>> {
    if ciphertext.len() % 8 != 0 {
        return Err(ReaderError::InvalidInput(format!(
            "Blowfish ciphertext must be a multiple of 8 bytes, got {}",
            ciphertext.len()
        )));
    }
    let tables = blowfish_tables()?;
    let mut prev_l = be_u32(iv, 0)?;
    let mut prev_r = be_u32(iv, 4)?;
    let mut out = vec![0_u8; ciphertext.len()];

    for (block_index, block) in ciphertext.chunks_exact(8).enumerate() {
        let cl = be_u32(block, 0)?;
        let cr = be_u32(block, 4)?;
        let (dl, dr) = blowfish_decrypt_block(cl, cr, &tables);
        write_be_u32(&mut out, block_index * 8, dl ^ prev_l)?;
        write_be_u32(&mut out, block_index * 8 + 4, dr ^ prev_r)?;
        prev_l = cl;
        prev_r = cr;
    }
    Ok(out)
}

fn blowfish_tables() -> ReaderResult<BlowfishTables> {
    const UNDEFINED_KEY: &[u8] = b"undefined";
    let s_bytes = STANDARD
        .decode(include_str!("blowfish_s.b64").trim())
        .map_err(|error| ReaderError::Decode(format!("Blowfish S table base64 failed: {error}")))?;
    let p_bytes = STANDARD
        .decode(include_str!("blowfish_p.b64").trim())
        .map_err(|error| ReaderError::Decode(format!("Blowfish P table base64 failed: {error}")))?;
    let s_bytes = rc4(UNDEFINED_KEY, &s_bytes, 769);
    let p_bytes = rc4(UNDEFINED_KEY, &p_bytes, 769);
    if p_bytes.len() < 18 * 4 || s_bytes.len() < 4 * 256 * 4 {
        return Err(ReaderError::Decode(format!(
            "Blowfish tables are truncated: p={}, s={}",
            p_bytes.len(),
            s_bytes.len()
        )));
    }

    let mut p = [0_u32; 18];
    for (index, slot) in p.iter_mut().enumerate() {
        *slot = le_u32(&p_bytes, index * 4)?;
    }
    let mut s = [[0_u32; 256]; 4];
    for box_index in 0..4 {
        for entry_index in 0..256 {
            s[box_index][entry_index] =
                le_u32(&s_bytes, (box_index * 256 + entry_index) * 4)?;
        }
    }
    Ok(BlowfishTables { p, s })
}

fn blowfish_decrypt_block(xl_in: u32, xr_in: u32, tables: &BlowfishTables) -> (u32, u32) {
    let mut xl = xl_in ^ tables.p[17];
    let mut xr = xr_in ^ tables.p[16];
    std::mem::swap(&mut xl, &mut xr);
    for i in (0..=15).rev() {
        std::mem::swap(&mut xl, &mut xr);
        xr ^= blowfish_f(xl, tables);
        xl ^= tables.p[i];
    }
    (xl, xr)
}

fn blowfish_f(x: u32, tables: &BlowfishTables) -> u32 {
    let a = ((x >> 24) & 0xff) as usize;
    let b = ((x >> 16) & 0xff) as usize;
    let c = ((x >> 8) & 0xff) as usize;
    let d = (x & 0xff) as usize;
    (tables.s[0][a].wrapping_add(tables.s[1][b]) ^ tables.s[2][c])
        .wrapping_add(tables.s[3][d])
}

fn be_u32(buf: &[u8], off: usize) -> ReaderResult<u32> {
    if off + 4 > buf.len() {
        return Err(ReaderError::InvalidInput(format!(
            "big-endian u32 read out of bounds at offset {off}"
        )));
    }
    Ok(u32::from_be_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
    ]))
}

fn write_be_u32(buf: &mut [u8], off: usize, value: u32) -> ReaderResult<()> {
    if off + 4 > buf.len() {
        return Err(ReaderError::InvalidInput(format!(
            "big-endian u32 write out of bounds at offset {off}"
        )));
    }
    buf[off..off + 4].copy_from_slice(&value.to_be_bytes());
    Ok(())
}

fn xor_mask_a6(buf: &mut [u8], shift: usize) {
    const A6: [usize; 19] = [
        11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 67, 71, 73, 79, 83, 69,
    ];
    for (index, byte) in buf.iter_mut().enumerate() {
        *byte ^= A6[(A6[index] + shift) % A6.len()] as u8;
    }
}

fn decode_hex(input: &str) -> ReaderResult<Vec<u8>> {
    if input.len() & 1 != 0 {
        return Err(ReaderError::InvalidInput(format!(
            "hex input has odd length: {}",
            input.len()
        )));
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    let bytes = input.as_bytes();
    for index in (0..bytes.len()).step_by(2) {
        let high = hex_nibble(bytes[index])?;
        let low = hex_nibble(bytes[index + 1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> ReaderResult<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(ReaderError::InvalidInput(format!(
            "invalid hex digit: {}",
            byte as char
        ))),
    }
}

fn require_len(name: &str, value: &[u8], expected: usize) -> ReaderResult<()> {
    if value.len() != expected {
        return Err(ReaderError::InvalidInput(format!(
            "{name} must be {expected} bytes, got {}",
            value.len()
        )));
    }
    Ok(())
}

fn le_u16(buf: &[u8], off: usize) -> ReaderResult<u16> {
    if off + 2 > buf.len() {
        return Err(ReaderError::InvalidInput(format!(
            "little-endian u16 read out of bounds at offset {off}"
        )));
    }
    Ok(u16::from_le_bytes([buf[off], buf[off + 1]]))
}

fn le_u32(buf: &[u8], off: usize) -> ReaderResult<u32> {
    read_u32_le_raw(buf, off).ok_or_else(|| {
        ReaderError::InvalidInput(format!(
            "little-endian u32 read out of bounds at offset {off}"
        ))
    })
}

fn read_u32_le_raw(buf: &[u8], off: usize) -> Option<u32> {
    if off + 4 > buf.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qsc_find_entry_returns_entry_with_running_offset() {
        let mut directory = vec![0_u8; QSC_DIR_SIZE];
        directory[..QSC_MAGIC.len()].copy_from_slice(QSC_MAGIC);
        write_qsc_entry(&mut directory, 0, b"XEBP", 10, "page001");
        write_qsc_entry(&mut directory, 1, b"XEBP", 25, "page002");
        write_qsc_entry(&mut directory, 2, b"XEBP", 7, "page003");

        let entry = qsc_find_entry(&directory, "page002")
            .unwrap()
            .expect("entry exists");

        assert_eq!(entry.four_cc, "XEBP");
        assert_eq!(entry.size, 25);
        assert_eq!(entry.name, "page002");
        assert_eq!(entry.offset, 10);
        assert!(qsc_find_entry(&directory, "missing").unwrap().is_none());
    }

    #[test]
    fn qsc_find_entry_rejects_bad_magic() {
        let directory = vec![0_u8; QSC_DIR_SIZE];
        let error = qsc_find_entry(&directory, "page001").unwrap_err();
        assert!(matches!(error, ReaderError::InvalidInput(_)));
    }

    #[test]
    fn strip_to_webp_trims_after_first_vp8_chunk_and_rewrites_size() {
        let mut xebp = Vec::new();
        xebp.extend_from_slice(b"RIFF");
        xebp.extend_from_slice(&0_u32.to_le_bytes());
        xebp.extend_from_slice(b"XEBP");
        xebp.extend_from_slice(b"VP8 ");
        xebp.extend_from_slice(&5_u32.to_le_bytes());
        xebp.extend_from_slice(b"abcde");
        xebp.push(0);
        xebp.extend_from_slice(b"ZSTR");
        xebp.extend_from_slice(&4_u32.to_le_bytes());
        xebp.extend_from_slice(b"meta");

        let stripped = strip_to_webp(&xebp);

        assert_eq!(stripped.len(), 26);
        assert_eq!(&stripped[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(stripped[4..8].try_into().unwrap()), 18);
        assert_eq!(&stripped[8..12], b"WEBP");
        assert_eq!(&stripped[12..20], &xebp[12..20]);
        assert_eq!(&stripped[20..], b"abcde\0");
    }

    #[test]
    fn strip_to_webp_returns_invalid_input_unchanged() {
        let input = b"not a webp";
        assert_eq!(strip_to_webp(input), input);
    }

    fn write_qsc_entry(
        directory: &mut [u8],
        index: usize,
        four_cc: &[u8; 4],
        size: u32,
        name: &str,
    ) {
        let base = 32 + index * QSC_ENTRY_SIZE;
        directory[base..base + 4].copy_from_slice(four_cc);
        directory[base + 4..base + 8].copy_from_slice(&size.to_le_bytes());
        directory[base + 8..base + 8 + name.len()].copy_from_slice(name.as_bytes());
    }
}
