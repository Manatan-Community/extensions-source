use aes::{Aes128, Aes256};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
};
use cbc::{
    Decryptor,
    cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7},
};
use ctr::cipher::StreamCipher;
use hmac::{Hmac, Mac};
use image::{DynamicImage, GenericImage, GenericImageView, ImageFormat, RgbaImage};
use manatan_extension::{ProcessedImage, abi::ExtensionResult};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Cursor};

type Aes128CbcDec = Decryptor<Aes128>;
type Aes256CbcDec = Decryptor<Aes256>;
type Aes256Ctr = ctr::Ctr128BE<Aes256>;
type HmacSha256 = Hmac<Sha256>;

pub fn passthrough_processed_image(request: &Value) -> ProcessedImage {
    ProcessedImage {
        image_base64: image_base64(request).unwrap_or_default().to_string(),
        mime_type: request
            .get("mimeType")
            .or_else(|| request.get("mime_type"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        ..ProcessedImage::default()
    }
}

pub fn image_base64(request: &Value) -> Option<&str> {
    request
        .get("imageBase64")
        .or_else(|| request.get("image_base64"))
        .and_then(Value::as_str)
}

pub fn page_extra_str<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("page")
        .and_then(|page| page.get("extra"))
        .and_then(|extra| extra.get(key))
        .and_then(Value::as_str)
}

pub fn page_extra_bool(request: &Value, key: &str) -> bool {
    request
        .get("page")
        .and_then(|page| page.get("extra"))
        .and_then(|extra| extra.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn processed_jpeg(image_base64: String) -> ProcessedImage {
    ProcessedImage {
        image_base64,
        mime_type: Some("image/jpeg".to_string()),
        ..ProcessedImage::default()
    }
}

pub struct ComiciViewer;

impl ComiciViewer {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        Self::process_page_image_with_extra_key(request, "comiciScramble")
    }

    pub fn process_page_image_with_extra_key(
        request: Value,
        extra_key: &str,
    ) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(scramble) = page_extra_str(&request, extra_key).filter(|value| !value.is_empty())
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::descramble_base64(input, scramble).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, scramble: &str) -> Option<String> {
        let mapping = TileGrid::parse_mapping(scramble);
        TileGrid::descramble_base64(input, &mapping, 4, 4)
    }
}

pub struct GigaViewer;

impl GigaViewer {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        if !page_extra_bool(&request, "gigaScramble") {
            return Ok(passthrough_processed_image(&request));
        }
        Ok(processed_jpeg(
            Self::descramble_base64(input).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (width, height) = source.dimensions();
        let block_width = (width / 32) * 8;
        let block_height = (height / 32) * 8;
        if block_width == 0 || block_height == 0 {
            return None;
        }
        let mut target = source.clone();
        for index in 0..16 {
            let dst_block = (index % 4) * 4 + (index / 4);
            copy_rect(
                &source,
                &mut target,
                (index % 4) as u32 * block_width,
                (index / 4) as u32 * block_height,
                (dst_block % 4) as u32 * block_width,
                (dst_block / 4) as u32 * block_height,
                block_width,
                block_height,
            );
        }
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }
}

pub struct LineManga;

impl LineManga {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(portal) = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("linePortal"))
        else {
            return Ok(passthrough_processed_image(&request));
        };
        let hc = portal.get("hc").and_then(Value::as_u64).unwrap_or(0) as u32;
        let bwd = portal.get("bwd").and_then(Value::as_u64).unwrap_or(0) as u32;
        let map = portal
            .get("m")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        Ok(processed_jpeg(
            Self::descramble_base64(input, hc, bwd, &map).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, hc: u32, bwd: u32, map: &[&str]) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (width, height) = source.dimensions();
        if hc == 0 || bwd == 0 {
            return None;
        }
        let mut target = source.clone();
        for (index, encoded) in map.iter().enumerate() {
            let source_index = u32::from_str_radix(encoded, 35).ok()?;
            let sx = (source_index % hc) * bwd;
            let sy = (source_index / hc) * bwd;
            let dx = (index as u32 % hc) * bwd;
            let dy = (index as u32 / hc) * bwd;
            if sx + bwd <= width && sy + bwd <= height && dx + bwd <= width && dy + bwd <= height {
                copy_rect(&source, &mut target, sx, sy, dx, dy, bwd, bwd);
            }
        }
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }
}

pub struct MagazinePocket;

impl MagazinePocket {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(meta) = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("magapokeScramble"))
        else {
            return Ok(passthrough_processed_image(&request));
        };
        let seed = meta.get("seed").and_then(Value::as_str).unwrap_or_default();
        let title_id = meta.get("titleId").and_then(Value::as_u64).unwrap_or(0) as u32;
        let episode_id = meta.get("episodeId").and_then(Value::as_u64).unwrap_or(0) as u32;
        Ok(processed_jpeg(
            Self::descramble_base64(input, seed, title_id, episode_id)
                .unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(
        input: &str,
        seed: &str,
        title_id: u32,
        episode_id: u32,
    ) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (width, height) = source.dimensions();
        let block_width = (width / 32) * 8;
        let block_height = (height / 32) * 8;
        if block_width == 0 || block_height == 0 {
            return None;
        }
        let mut target = source.clone();
        for (src, dst) in Self::coords(seed, title_id, episode_id) {
            copy_rect(
                &source,
                &mut target,
                (src % 4) * block_width,
                (src / 4) * block_height,
                (dst % 4) * block_width,
                (dst / 4) * block_height,
                block_width,
                block_height,
            );
        }
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    pub fn coords(seed: &str, title_id: u32, episode_id: u32) -> Vec<(u32, u32)> {
        let charset = if title_id % 2 == 0 {
            "svdk0m7acl"
        } else {
            "q6jtf2xnog"
        };
        let mut parsed = 0u64;
        for ch in seed.chars() {
            let Some(index) = charset.find(ch) else {
                break;
            };
            parsed = parsed * 10 + index as u64;
        }
        let mut seed32 = parsed as u32 ^ (title_id + episode_id);
        let mut pairs = Vec::new();
        for index in 0..16u32 {
            seed32 = xorshift32(seed32);
            pairs.push((seed32, index));
        }
        pairs.sort_by_key(|(value, _)| *value);
        pairs
            .into_iter()
            .enumerate()
            .map(|(dst, (_, src))| (src, dst as u32))
            .collect()
    }
}

pub struct YnJn;

impl YnJn {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        if !page_extra_bool(&request, "ynjnScramble") {
            return Ok(passthrough_processed_image(&request));
        }
        Ok(processed_jpeg(
            Self::descramble_base64(input).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (width, height) = source.dimensions();
        let block_width = width / 4;
        let block_height = height / 4;
        if block_width == 0 || block_height == 0 {
            return None;
        }
        let mut target = source.clone();
        for index in 0..16 {
            let row = index / 4;
            let col = index % 4;
            copy_rect(
                &source,
                &mut target,
                col * block_width,
                row * block_height,
                row * block_width,
                col * block_height,
                block_width,
                block_height,
            );
        }
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }
}

pub struct TileGrid;

impl TileGrid {
    pub fn parse_mapping(input: &str) -> Vec<u32> {
        parse_csv_u32(input)
    }

    pub fn descramble_base64(
        input: &str,
        mapping: &[u32],
        grid_w: u32,
        grid_h: u32,
    ) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        let image = image::load_from_memory(&bytes).ok()?;
        let result = Self::descramble_image(image, mapping, grid_w, grid_h)?;
        encode_jpeg_base64(result)
    }

    pub fn descramble_image(
        image: DynamicImage,
        mapping: &[u32],
        grid_w: u32,
        grid_h: u32,
    ) -> Option<DynamicImage> {
        let (width, height) = image.dimensions();
        if grid_w == 0
            || grid_h == 0
            || mapping.len() < (grid_w * grid_h) as usize
            || width < 8 * grid_w
            || height < 8 * grid_h
        {
            return Some(image);
        }
        let piece_w = (width / grid_w) / 8 * 8;
        let piece_h = (height / grid_h) / 8 * 8;
        if piece_w == 0 || piece_h == 0 {
            return Some(image);
        }
        let mut result = DynamicImage::new_rgba8(width, height);
        for (dest, source) in mapping.iter().enumerate() {
            let dx = (dest as u32 % grid_w) * piece_w;
            let dy = (dest as u32 / grid_w) * piece_h;
            let sx = (source % grid_w) * piece_w;
            let sy = (source / grid_w) * piece_h;
            let tile = image.crop_imm(sx, sy, piece_w, piece_h);
            result.copy_from(&tile, dx, dy).ok()?;
        }
        Some(result)
    }
}

fn copy_rect(
    src: &RgbaImage,
    dst: &mut RgbaImage,
    sx: u32,
    sy: u32,
    dx: u32,
    dy: u32,
    width: u32,
    height: u32,
) {
    for y in 0..height {
        for x in 0..width {
            let pixel = *src.get_pixel(sx + x, sy + y);
            dst.put_pixel(dx + x, dy + y, pixel);
        }
    }
}

fn xorshift32(mut value: u32) -> u32 {
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    value
}

pub fn encode_jpeg_base64(image: DynamicImage) -> Option<String> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Jpeg).ok()?;
    Some(STANDARD.encode(out.into_inner()))
}

pub struct AesCbc;

impl AesCbc {
    pub fn decrypt_128_pkcs7(cipher_text: &[u8], key: &[u8], iv: &[u8]) -> Option<Vec<u8>> {
        Aes128CbcDec::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(cipher_text)
            .ok()
    }

    pub fn decrypt_256_pkcs7(cipher_text: &[u8], key: &[u8], iv: &[u8]) -> Option<Vec<u8>> {
        Aes256CbcDec::new_from_slices(key, iv)
            .ok()?
            .decrypt_padded_vec_mut::<Pkcs7>(cipher_text)
            .ok()
    }

    pub fn decrypt_128_pkcs7_base64(input: &str, key: &[u8], iv: &[u8]) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        Some(STANDARD.encode(Self::decrypt_128_pkcs7(&bytes, key, iv)?))
    }

    pub fn decrypt_256_pkcs7_base64(input: &str, key: &[u8], iv: &[u8]) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        Some(STANDARD.encode(Self::decrypt_256_pkcs7(&bytes, key, iv)?))
    }
}

pub struct AesImage;

impl AesImage {
    pub fn process_128_pkcs7_hex(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let key = page_extra_str(&request, "aesKeyHex").and_then(decode_hex);
        let iv = page_extra_str(&request, "aesIvHex").and_then(decode_hex);
        Ok(match (key, iv) {
            (Some(key), Some(iv)) => ProcessedImage {
                image_base64: AesCbc::decrypt_128_pkcs7_base64(input, &key, &iv)
                    .unwrap_or_else(|| input.to_string()),
                mime_type: request
                    .get("mimeType")
                    .or_else(|| request.get("mime_type"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                ..ProcessedImage::default()
            },
            _ => passthrough_processed_image(&request),
        })
    }

    pub fn process_128_pkcs7_base64_url(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let key = page_extra_str(&request, "aesKeyBase64Url").and_then(decode_base64_any);
        let iv = page_extra_str(&request, "aesIvBase64Url").and_then(decode_base64_any);
        Ok(match (key, iv) {
            (Some(key), Some(iv)) => ProcessedImage {
                image_base64: AesCbc::decrypt_128_pkcs7_base64(input, &key, &iv)
                    .unwrap_or_else(|| input.to_string()),
                mime_type: request
                    .get("mimeType")
                    .or_else(|| request.get("mime_type"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                ..ProcessedImage::default()
            },
            _ => passthrough_processed_image(&request),
        })
    }
}

pub struct CoronaExImage;

impl CoronaExImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let hash = page_extra_str(&request, "drmHash")
            .or_else(|| page_extra_str(&request, "drm_hash"))
            .and_then(decode_base64_any);
        Ok(processed_jpeg(
            hash.and_then(|hash| Self::descramble_base64(input, &hash))
                .unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, drm_hash: &[u8]) -> Option<String> {
        if drm_hash.len() < 3 {
            return None;
        }
        let bytes = decode_base64_any(input)?;
        let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let cols = drm_hash[0] as u32;
        let rows = drm_hash[1] as u32;
        if cols == 0 || rows == 0 {
            return None;
        }
        let block_map = &drm_hash[2..];
        if block_map.len() < cols.saturating_mul(rows) as usize {
            return None;
        }

        let width = image.width();
        let height = image.height();
        let block_width = (width - width % 8) / cols;
        let block_height = (height - height % 8) / rows;
        if block_width == 0 || block_height == 0 {
            return encode_jpeg_base64(DynamicImage::ImageRgba8(image));
        }

        let mut output = image.clone();
        for dst_index in 0..cols.saturating_mul(rows) {
            let src_index = block_map.get(dst_index as usize).copied()? as u32;
            let src_x = (src_index % cols) * block_width;
            let src_y = (src_index / cols) * block_height;
            let dst_x = (dst_index % cols) * block_width;
            let dst_y = (dst_index / cols) * block_height;
            if src_x + block_width <= width
                && src_y + block_height <= height
                && dst_x + block_width <= width
                && dst_y + block_height <= height
            {
                copy_rect(
                    &image,
                    &mut output,
                    src_x,
                    src_y,
                    dst_x,
                    dst_y,
                    block_width,
                    block_height,
                );
            }
        }
        encode_jpeg_base64(DynamicImage::ImageRgba8(output))
    }
}

pub struct XorImage;

impl XorImage {
    pub fn process_key_hex_extra(
        request: Value,
        extra_key: &str,
    ) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(key) = page_extra_str(&request, extra_key).and_then(decode_hex) else {
            return Ok(passthrough_processed_image(&request));
        };
        Self::process_bytes_with_key(&request, input, &key)
    }

    pub fn process_drm_hash_hex(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(hash) = page_extra_str(&request, "drmHash").and_then(decode_hex) else {
            return Ok(passthrough_processed_image(&request));
        };
        Self::process_bytes_with_key(&request, input, &hash)
    }

    fn process_bytes_with_key(
        request: &Value,
        input: &str,
        key: &[u8],
    ) -> ExtensionResult<ProcessedImage> {
        if key.is_empty() {
            return Ok(passthrough_processed_image(request));
        }
        let Some(mut bytes) = decode_base64_any(input) else {
            return Ok(passthrough_processed_image(request));
        };
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte ^= key[index % key.len()];
        }
        Ok(ProcessedImage {
            image_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            mime_type: Some("image/jpeg".into()).or_else(|| {
                request
                    .get("mimeType")
                    .or_else(|| request.get("mime_type"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            }),
            ..ProcessedImage::default()
        })
    }
}

pub struct CiaoImage;

impl CiaoImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let seed = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("scrambleSeed"))
            .and_then(Value::as_i64)
            .unwrap_or_default() as u32;
        let version = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("scrambleVersion"))
            .and_then(Value::as_u64)
            .unwrap_or(1);
        if seed == 0 {
            return Ok(passthrough_processed_image(&request));
        }
        Ok(processed_jpeg(
            Self::descramble_base64(input, seed, version).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, seed: u32, version: u64) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (width, height) = source.dimensions();
        let tile_width = if version == 2 {
            (width / 32) * 8
        } else {
            ((width / 8) * 8) / 4
        };
        let tile_height = if version == 2 {
            (height / 32) * 8
        } else {
            ((height / 8) * 8) / 4
        };
        if tile_width == 0 || tile_height == 0 {
            return Some(input.to_string());
        }
        let mut target = RgbaImage::new(width, height);
        for (src_index, dst_index) in Self::coords(seed) {
            let sx = (src_index % 4) * tile_width;
            let sy = (src_index / 4) * tile_height;
            let dx = (dst_index % 4) * tile_width;
            let dy = (dst_index / 4) * tile_height;
            copy_rect(
                &source,
                &mut target,
                sx,
                sy,
                dx,
                dy,
                tile_width,
                tile_height,
            );
        }
        if version == 2 {
            let processed_width = tile_width * 4;
            let processed_height = tile_height * 4;
            if width > processed_width {
                copy_rect(
                    &source,
                    &mut target,
                    processed_width,
                    0,
                    processed_width,
                    0,
                    width - processed_width,
                    height,
                );
            }
            if height > processed_height {
                copy_rect(
                    &source,
                    &mut target,
                    0,
                    processed_height,
                    0,
                    processed_height,
                    processed_width.min(width),
                    height - processed_height,
                );
            }
        }
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    fn coords(seed: u32) -> Vec<(u32, u32)> {
        let mut seed = seed;
        let mut pairs = Vec::new();
        for index in 0..16u32 {
            seed = xorshift32(seed);
            pairs.push((seed, index));
        }
        pairs.sort_by_key(|(value, _)| *value);
        pairs
            .into_iter()
            .enumerate()
            .map(|(dest, (_, source))| (source, dest as u32))
            .collect()
    }
}

pub struct GuardianBlockImage;

impl GuardianBlockImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        Self::process_page_image_with_extra_key(request, "guardianKey")
    }

    pub fn process_page_image_with_extra_key(
        request: Value,
        extra_key: &str,
    ) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(key) = page_extra_str(&request, extra_key).filter(|value| !value.is_empty())
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::descramble_base64(input, key).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, key: &str) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let target = Self::descramble_image(&source, key);
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    pub fn descramble_image(source: &RgbaImage, key: &str) -> RgbaImage {
        const BLOCK_WIDTH: u32 = 96;
        const BLOCK_HEIGHT: u32 = 128;

        let width = source.width();
        let height = source.height();
        let cols = width / BLOCK_WIDTH;
        let rows = height / BLOCK_HEIGHT;
        let total_blocks = (cols * rows) as usize;
        let mapping = Self::shuffle_mapping(total_blocks, key);
        let mut target = RgbaImage::new(width, height);

        for (source_index, dest_index) in mapping.into_iter().enumerate() {
            let source_index = source_index as u32;
            let dest_index = dest_index as u32;
            let sx = (source_index % cols) * BLOCK_WIDTH;
            let sy = (source_index / cols) * BLOCK_HEIGHT;
            let dx = (dest_index % cols) * BLOCK_WIDTH;
            let dy = (dest_index / cols) * BLOCK_HEIGHT;
            copy_rect(
                source,
                &mut target,
                sx,
                sy,
                dx,
                dy,
                BLOCK_WIDTH,
                BLOCK_HEIGHT,
            );
        }

        if width % BLOCK_WIDTH > 0 {
            let rem_x = cols * BLOCK_WIDTH;
            copy_rect(
                source,
                &mut target,
                rem_x,
                0,
                rem_x,
                0,
                width - rem_x,
                height,
            );
        }
        if height % BLOCK_HEIGHT > 0 {
            let rem_y = rows * BLOCK_HEIGHT;
            let processed_width = cols * BLOCK_WIDTH;
            if processed_width > 0 {
                copy_rect(
                    source,
                    &mut target,
                    0,
                    rem_y,
                    0,
                    rem_y,
                    processed_width,
                    height - rem_y,
                );
            }
        }

        target
    }

    pub fn shuffle_mapping(total_blocks: usize, key: &str) -> Vec<usize> {
        let mut mapping = (0..total_blocks).collect::<Vec<_>>();
        let mut randomizer = GuardianRandomizer::new(key);
        for index in 0..mapping.len() {
            let target = randomizer.rand(mapping.len() - 1);
            mapping.swap(index, target);
        }
        mapping
    }
}

pub struct MangaMiraiImage;

impl MangaMiraiImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(content_id) =
            page_extra_str(&request, "contentId").filter(|value| !value.is_empty())
        else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(scramble_key) =
            page_extra_str(&request, "scrambleKey").filter(|value| !value.is_empty())
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::decrypt_and_descramble_base64(input, content_id, scramble_key)
                .unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn decrypt_and_descramble_base64(
        input: &str,
        content_id: &str,
        scramble_key: &str,
    ) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        if bytes.len() <= 16 {
            return None;
        }
        let (iv, cipher_text) = bytes.split_at(16);
        let key = Sha256::digest(format!("manga{content_id}mirai").as_bytes());
        let decrypted = AesCbc::decrypt_256_pkcs7(cipher_text, &key, iv)?;
        let source = image::load_from_memory(&decrypted).ok()?.to_rgba8();
        let target = Self::descramble_image(&source, &Self::scramble_order(scramble_key)?);
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    pub fn descramble_image(source: &RgbaImage, order: &[u32]) -> RgbaImage {
        const TILE: u32 = 96;

        let width = source.width();
        let height = source.height();
        let columns = width.div_ceil(TILE).max(1);
        let mut target = RgbaImage::new(width, height);

        for (dest_index, source_index) in order.iter().copied().enumerate() {
            let sx = (source_index % columns) * TILE;
            let sy = (source_index / columns) * TILE;
            if sx >= width || sy >= height {
                continue;
            }
            let dx = (dest_index as u32 % columns) * TILE;
            let dy = (dest_index as u32 / columns) * TILE;
            if dx >= width || dy >= height {
                continue;
            }
            copy_rect(
                source,
                &mut target,
                sx,
                sy,
                dx,
                dy,
                TILE.min(width - sx),
                TILE.min(height - sy),
            );
        }

        target
    }

    pub fn scramble_order(key: &str) -> Option<Vec<u32>> {
        let bytes = decode_base64_any(key)?;
        let mut order = Vec::new();
        let mut current: Option<u32> = None;
        for byte in bytes {
            if byte.is_ascii_digit() {
                current = Some(current.unwrap_or(0) * 10 + (byte - b'0') as u32);
            } else if let Some(value) = current.take() {
                order.push(value);
            }
        }
        if let Some(value) = current {
            order.push(value);
        }
        Some(order)
    }
}

pub struct PiccomaImage;

impl PiccomaImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(seed) = page_extra_str(&request, "piccomaSeed").filter(|value| !value.is_empty())
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::descramble_base64(input, seed).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn seed_from_image_url(image_url: &str) -> Option<String> {
        let (without_fragment, _) = image_url.split_once('#').unwrap_or((image_url, ""));
        let (without_query, query) = without_fragment
            .split_once('?')
            .unwrap_or((without_fragment, ""));
        let path = without_query
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/').map(|(_, path)| path))
            .unwrap_or(without_query);
        let checksum = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .nth(3)?;
        let expiration = query
            .split('&')
            .filter_map(|part| part.split_once('='))
            .find_map(|(key, value)| (key == "expires").then_some(value))?;
        let sum = expiration
            .chars()
            .filter_map(|ch| ch.to_digit(10))
            .sum::<u32>() as usize;
        if checksum.is_empty() {
            return None;
        }
        let residual_index = sum % checksum.len();
        let split = checksum.len().saturating_sub(residual_index);
        let rotated = format!("{}{}", &checksum[split..], &checksum[..split]);
        Some(Self::decode_seed(&rotated))
    }

    pub fn descramble_base64(input: &str, seed: &str) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let target = Self::descramble_image(&source, seed);
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    pub fn descramble_image(source: &RgbaImage, seed: &str) -> RgbaImage {
        const TILE: u32 = 50;

        let width = source.width();
        let height = source.height();
        let columns = width.div_ceil(TILE);
        let rows = height.div_ceil(TILE);
        let mut groups: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();

        for index in 0..columns * rows {
            let x = (index % columns) * TILE;
            let y = (index / columns) * TILE;
            let tile_width = TILE.min(width - x);
            let tile_height = TILE.min(height - y);
            groups
                .entry((tile_width, tile_height))
                .or_default()
                .push((x, y));
        }

        let mut target = RgbaImage::new(width, height);
        for ((tile_width, tile_height), tiles) in groups {
            let order = SeedRandom::new(seed).shuffle_indices(tiles.len());
            for (dest_index, source_index) in order.into_iter().enumerate() {
                let Some(&(sx, sy)) = tiles.get(source_index) else {
                    continue;
                };
                let (dx, dy) = tiles[dest_index];
                copy_rect(source, &mut target, sx, sy, dx, dy, tile_width, tile_height);
            }
        }
        target
    }

    fn decode_seed(seed: &str) -> String {
        const MASK: usize = 3_236_551;
        let mut bytes = seed.as_bytes().to_vec();
        for (index, byte) in bytes.iter_mut().enumerate() {
            if ((MASK >> index) & 1) == 1 {
                *byte ^= 1;
            }
        }
        String::from_utf8(bytes).unwrap_or_else(|_| seed.to_string())
    }
}

pub struct MangaKingdomImage;

impl MangaKingdomImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let scene_no = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("mangaKingdomScene"))
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        Ok(processed_jpeg(
            Self::process_content_base64(input, scene_no).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn process_content_base64(input: &str, scene_no: u32) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        let text = String::from_utf8(bytes).ok()?;
        let root = serde_json::from_str::<Value>(&strip_jsonp(&text)).ok()?;
        let image = root
            .get("scenes")?
            .as_array()?
            .iter()
            .find(|scene| scene.get("sceneNo").and_then(Value::as_u64) == Some(scene_no as u64))?
            .get("images")?
            .as_array()?
            .first()?;
        let width = image.get("width")?.as_u64()? as u32;
        let height = image.get("height")?.as_u64()? as u32;
        let key = image.get("key")?.as_i64()? as i64;
        let image_base64 = image.get("imgBase64")?.as_str()?;
        let image_bytes = decode_base64_any(image_base64)?;
        let source = image::load_from_memory(&image_bytes).ok()?.to_rgba8();
        let target = Self::descramble_image(&source, key, width, height);
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    pub fn descramble_image(
        source: &RgbaImage,
        key: i64,
        dest_width: u32,
        dest_height: u32,
    ) -> RgbaImage {
        let source_width = source.width();
        let source_height = source.height();
        let mut block_width = 32;
        let mut block_height = 32;
        if source_width > 1000 || source_height > 1000 {
            block_width *= 3;
            block_height *= 3;
        } else if source_width > 300 || source_height > 300 {
            block_width *= 2;
            block_height *= 2;
        }
        let inner_width = block_width - 2;
        let inner_height = block_height - 2;
        let columns = source_width / block_width;
        let rows = source_height / block_height;
        let total = columns * rows;
        if total == 0 {
            return source.clone();
        }

        let mut lcg = key;
        let mut available = (0..total).collect::<Vec<_>>();
        let mut result = RgbaImage::new(columns * inner_width, rows * inner_height);
        for target_index in 0..total {
            lcg = (lcg * 8_741 + 30_873) % 131_071;
            let source_position = (lcg as usize % available.len()).min(available.len() - 1);
            let source_index = available.remove(source_position);
            let sx = (source_index % columns) * block_width + 1;
            let sy = (source_index / columns) * block_height + 1;
            let dx = (target_index % columns) * inner_width;
            let dy = (target_index / columns) * inner_height;
            copy_rect(
                source,
                &mut result,
                sx,
                sy,
                dx,
                dy,
                inner_width,
                inner_height,
            );
        }
        let crop_width = result.width().min(dest_width);
        let crop_height = result.height().min(dest_height);
        if crop_width == result.width() && crop_height == result.height() {
            result
        } else {
            image::imageops::crop_imm(&result, 0, 0, crop_width, crop_height).to_image()
        }
    }
}

fn strip_jsonp(input: &str) -> String {
    let trimmed = input.trim();
    let open = trimmed.find('(');
    let close = trimmed.rfind(')');
    match (open, close) {
        (Some(open), Some(close)) if open < close => trimmed[open + 1..close].to_string(),
        _ => trimmed.to_string(),
    }
}

pub struct NicovideoSeigaImage;

impl NicovideoSeigaImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(key) = page_extra_str(&request, "nicoImageKey").and_then(decode_hex) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(mut bytes) = decode_base64_any(input) else {
            return Ok(passthrough_processed_image(&request));
        };
        if key.is_empty() {
            return Ok(passthrough_processed_image(&request));
        }
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte ^= key[index % key.len()];
        }
        Ok(ProcessedImage {
            mime_type: Some(mime_from_magic(&bytes).to_string()),
            image_base64: STANDARD.encode(bytes),
            ..ProcessedImage::default()
        })
    }
}

pub struct PixivComicImage;

impl PixivComicImage {
    const SHUFFLE_SALT: &'static str = "4wXCKprMMoxnyJ3PocJFs4CYbfnbazNe";
    const GRID_SIZE: u32 = 32;

    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(key) =
            page_extra_str(&request, "pixivShuffleKey").filter(|value| !value.is_empty())
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(ProcessedImage {
            image_base64: Self::deshuffle_base64(input, key).unwrap_or_else(|| input.to_string()),
            mime_type: Some("image/png".into()),
            ..ProcessedImage::default()
        })
    }

    pub fn deshuffle_base64(input: &str, key: &str) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let target = Self::deshuffle_image(&source, key);
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(target)
            .write_to(&mut out, ImageFormat::Png)
            .ok()?;
        Some(STANDARD.encode(out.into_inner()))
    }

    pub fn deshuffle_image(source: &RgbaImage, key: &str) -> RgbaImage {
        let width = source.width();
        let height = source.height();
        let horizontal = width / Self::GRID_SIZE;
        if horizontal <= 1 {
            return source.clone();
        }
        let vertical = height.div_ceil(Self::GRID_SIZE);
        let mut hash = PixivHash::new(key);
        for _ in 0..100 {
            hash.next();
        }

        let mut row_maps = Vec::new();
        for _ in 0..vertical {
            let mut row = (0..horizontal).collect::<Vec<_>>();
            for j in (1..horizontal).rev() {
                let hash_index = hash.next() % (j + 1);
                row.swap(j as usize, hash_index as usize);
            }
            let mut inverse = vec![0u32; horizontal as usize];
            for (index, value) in row.into_iter().enumerate() {
                inverse[value as usize] = index as u32;
            }
            row_maps.push(inverse);
        }

        let mut target = RgbaImage::new(width, height);
        for y in 0..height {
            let row_index = (y / Self::GRID_SIZE) as usize;
            let row = &row_maps[row_index.min(row_maps.len() - 1)];
            for horizontal_index in 0..horizontal {
                let from = row[horizontal_index as usize] * Self::GRID_SIZE;
                let to = horizontal_index * Self::GRID_SIZE;
                for dx in 0..Self::GRID_SIZE {
                    let pixel = *source.get_pixel(from + dx, y);
                    target.put_pixel(to + dx, y, pixel);
                }
            }
            for x in horizontal * Self::GRID_SIZE..width {
                target.put_pixel(x, y, *source.get_pixel(x, y));
            }
        }
        target
    }
}

pub struct MangaFireImage;

impl MangaFireImage {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(offset) = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("mangaFireOffset"))
            .and_then(Value::as_u64)
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::descramble_base64(input, offset as u32).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, offset: u32) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let target = Self::descramble_image(&source, offset);
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    pub fn descramble_image(source: &RgbaImage, offset: u32) -> RgbaImage {
        const PIECE_SIZE: u32 = 200;
        const MIN_SPLIT_COUNT: u32 = 5;

        let width = source.width();
        let height = source.height();
        let piece_width = PIECE_SIZE.min(width.div_ceil(MIN_SPLIT_COUNT).max(1));
        let piece_height = PIECE_SIZE.min(height.div_ceil(MIN_SPLIT_COUNT).max(1));
        let x_max = width.div_ceil(piece_width).saturating_sub(1);
        let y_max = height.div_ceil(piece_height).saturating_sub(1);
        let mut target = RgbaImage::new(width, height);

        for y in 0..=y_max {
            for x in 0..=x_max {
                let dx = piece_width * x;
                let dy = piece_height * y;
                let w = piece_width.min(width - dx);
                let h = piece_height.min(height - dy);
                let sx_index = if x == x_max {
                    x
                } else if x_max == 0 {
                    0
                } else {
                    (x_max - x + offset) % x_max
                };
                let sy_index = if y == y_max {
                    y
                } else if y_max == 0 {
                    0
                } else {
                    (y_max - y + offset) % y_max
                };
                copy_rect(
                    source,
                    &mut target,
                    piece_width * sx_index,
                    piece_height * sy_index,
                    dx,
                    dy,
                    w,
                    h,
                );
            }
        }
        target
    }
}

pub struct ComixImage;

impl ComixImage {
    const GRID_COLS: u32 = 5;
    const GRID_ROWS: u32 = 5;
    const NUM_TILES: usize = (Self::GRID_COLS * Self::GRID_ROWS) as usize;
    const LCG_MULTIPLIER: u32 = 1_664_525;
    const LCG_INCREMENT: u32 = 1_013_904_223;

    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(seed) = response_header(&request, "x-scramble-seed")
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|seed| *seed != 0)
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::descramble_base64(input, seed as u32).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, seed: u32) -> Option<String> {
        let bytes = decode_base64_any(input)?;
        let source = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let target = Self::descramble_image(&source, seed);
        encode_jpeg_base64(DynamicImage::ImageRgba8(target))
    }

    pub fn descramble_image(source: &RgbaImage, seed: u32) -> RgbaImage {
        let width = source.width();
        let height = source.height();
        let tile_width = width / Self::GRID_COLS;
        let tile_height = height / Self::GRID_ROWS;
        if tile_width == 0 || tile_height == 0 {
            return source.clone();
        }

        let order = Self::build_order(seed);
        let mut target = source.clone();
        for (src_index, dst_index) in order.into_iter().enumerate() {
            let src_index = src_index as u32;
            let dst_index = dst_index as u32;
            let src_col = src_index % Self::GRID_COLS;
            let src_row = src_index / Self::GRID_COLS;
            let dst_col = dst_index % Self::GRID_COLS;
            let dst_row = dst_index / Self::GRID_COLS;
            copy_rect(
                source,
                &mut target,
                src_col * tile_width,
                src_row * tile_height,
                dst_col * tile_width,
                dst_row * tile_height,
                tile_width,
                tile_height,
            );
        }
        target
    }

    pub fn build_order(seed: u32) -> [usize; Self::NUM_TILES] {
        let mut out = [0usize; Self::NUM_TILES];
        for (index, value) in out.iter_mut().enumerate() {
            *value = index;
        }
        let mut state = seed;
        for i in (1..Self::NUM_TILES).rev() {
            state = state
                .wrapping_mul(Self::LCG_MULTIPLIER)
                .wrapping_add(Self::LCG_INCREMENT);
            let j = (state as u64 % (i as u64 + 1)) as usize;
            out.swap(i, j);
        }
        out
    }
}

pub struct PhiliaScansImage;

impl PhiliaScansImage {
    const AES_MAGIC: [u8; 2] = [0xff, 0x02];

    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(config) = PhiliaConfig::from_request(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(bytes) = decode_base64_any(input) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some((plain, original_width, original_height)) = Self::decrypt_bytes(&bytes, &config)
        else {
            return Ok(passthrough_processed_image(&request));
        };
        if !config.scrambled {
            return Ok(ProcessedImage {
                image_base64: STANDARD.encode(plain),
                mime_type: Some(config.mime_type),
                ..ProcessedImage::default()
            });
        }
        let Some(encoded) = Self::unscramble_base64(
            &plain,
            &config.chapter_key,
            config.page_index,
            config.grid_size,
            original_width,
            original_height,
            &config.mime_type,
        ) else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(ProcessedImage {
            image_base64: encoded,
            mime_type: Some(config.mime_type),
            ..ProcessedImage::default()
        })
    }

    fn decrypt_bytes(bytes: &[u8], config: &PhiliaConfig) -> Option<(Vec<u8>, u32, u32)> {
        let aes_scheme = bytes.starts_with(&Self::AES_MAGIC);
        let header_start = if aes_scheme { 2 } else { 0 };
        if bytes.len() < header_start + 4 {
            return None;
        }
        let original_width =
            u16::from_be_bytes([bytes[header_start], bytes[header_start + 1]]) as u32;
        let original_height =
            u16::from_be_bytes([bytes[header_start + 2], bytes[header_start + 3]]) as u32;
        let mut body = bytes[header_start + 4..].to_vec();
        if aes_scheme {
            let key = hmac_sha256(
                &config.chapter_key,
                format!("aesctr:{}", config.page_index).as_bytes(),
            );
            let mut cipher = Aes256Ctr::new_from_slices(&key, &[0u8; 16]).ok()?;
            cipher.apply_keystream(&mut body);
        } else {
            body = xor_keystream(&config.chapter_key, config.page_index, body);
        }
        Some((body, original_width, original_height))
    }

    fn unscramble_base64(
        bytes: &[u8],
        chapter_key: &[u8],
        page_index: usize,
        grid_size: u32,
        original_width: u32,
        original_height: u32,
        mime_type: &str,
    ) -> Option<String> {
        let source = image::load_from_memory(bytes).ok()?.to_rgba8();
        let target = Self::unscramble_image(
            &source,
            chapter_key,
            page_index,
            grid_size,
            original_width,
            original_height,
        );
        let format = if mime_type.eq_ignore_ascii_case("image/png") {
            ImageFormat::Png
        } else if mime_type.eq_ignore_ascii_case("image/webp") {
            ImageFormat::WebP
        } else {
            ImageFormat::Jpeg
        };
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(target)
            .write_to(&mut out, format)
            .ok()?;
        Some(STANDARD.encode(out.into_inner()))
    }

    pub fn unscramble_image(
        source: &RgbaImage,
        chapter_key: &[u8],
        page_index: usize,
        grid_size: u32,
        original_width: u32,
        original_height: u32,
    ) -> RgbaImage {
        let tile_width = source.width() / grid_size.max(1);
        let tile_height = source.height() / grid_size.max(1);
        if tile_width == 0 || tile_height == 0 {
            return source.clone();
        }
        let count = (grid_size * grid_size) as usize;
        let mut order = (0..count).collect::<Vec<_>>();
        if count >= 2 {
            let tile_signature = hmac_sha256(chapter_key, format!("tiles:{page_index}").as_bytes());
            let mut counter = 0usize;
            let mut randoms = Vec::<u32>::new();
            for idx in (1..count).rev() {
                if randoms.is_empty() {
                    let digest = hmac_sha256(&tile_signature, format!("perm:{counter}").as_bytes());
                    counter += 1;
                    randoms = digest
                        .chunks_exact(4)
                        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                        .rev()
                        .collect();
                }
                let value = randoms.pop().unwrap_or(0);
                order.swap(idx, value as usize % (idx + 1));
            }
        }
        let mut inverse = vec![0usize; count];
        for (index, value) in order.into_iter().enumerate() {
            inverse[value] = index;
        }
        let mut target = RgbaImage::new(original_width, original_height);
        for tile in 0..count {
            let src_index = inverse[tile] as u32;
            let tile = tile as u32;
            copy_rect(
                source,
                &mut target,
                (src_index % grid_size) * tile_width,
                (src_index / grid_size) * tile_height,
                (tile % grid_size) * tile_width,
                (tile / grid_size) * tile_height,
                tile_width.min(original_width.saturating_sub((tile % grid_size) * tile_width)),
                tile_height.min(original_height.saturating_sub((tile / grid_size) * tile_height)),
            );
        }
        target
    }
}

struct PhiliaConfig {
    scrambled: bool,
    mime_type: String,
    chapter_key: Vec<u8>,
    grid_size: u32,
    page_index: usize,
}

impl PhiliaConfig {
    fn from_request(request: &Value) -> Option<Self> {
        let extra = request.get("page")?.get("extra")?;
        let scrambled = extra
            .get("philiaScrambled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mime_type = extra
            .get("philiaMime")
            .and_then(Value::as_str)
            .unwrap_or("image/jpeg")
            .to_string();
        let page_index = extra
            .get("philiaPageIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let grid_size = extra
            .get("philiaGridSize")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        let chapter_key = if let (Some(a), Some(b)) = (
            extra.get("philiaPayloadA").and_then(Value::as_str),
            extra.get("philiaPayloadB").and_then(Value::as_str),
        ) {
            let a = decode_base64_any(a)?;
            let b = decode_base64_any(b)?;
            if a.len() >= 32 && b.len() >= 32 {
                (0..32).map(|index| a[index] ^ b[index]).collect()
            } else {
                decode_base64_any(extra.get("philiaChapterKey").and_then(Value::as_str)?)?
            }
        } else {
            decode_base64_any(extra.get("philiaChapterKey").and_then(Value::as_str)?)?
        };
        Some(Self {
            scrambled,
            mime_type,
            chapter_key,
            grid_size,
            page_index,
        })
    }
}

fn hmac_sha256(key: &[u8], input: &[u8]) -> Vec<u8> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(input);
    mac.finalize().into_bytes().to_vec()
}

fn xor_keystream(chapter_key: &[u8], page_index: usize, mut data: Vec<u8>) -> Vec<u8> {
    let blocks = data.len().div_ceil(32);
    for block in 0..blocks {
        let hash = hmac_sha256(chapter_key, format!("page:{page_index}:{block}").as_bytes());
        let base = block * 32;
        for index in 0..32.min(data.len().saturating_sub(base)) {
            data[base + index] ^= hash[index];
        }
    }
    data
}

struct PixivHash {
    state: [u32; 4],
}

impl PixivHash {
    fn new(key: &str) -> Self {
        let digest = Sha256::digest(format!("{}{}", PixivComicImage::SHUFFLE_SALT, key).as_bytes());
        let mut state = [
            u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]),
            u32::from_le_bytes([digest[4], digest[5], digest[6], digest[7]]),
            u32::from_le_bytes([digest[8], digest[9], digest[10], digest[11]]),
            u32::from_le_bytes([digest[12], digest[13], digest[14], digest[15]]),
        ];
        if state.iter().all(|value| *value == 0) {
            state[0] = 1;
        }
        Self { state }
    }

    fn next(&mut self) -> u32 {
        let e = 9u32.wrapping_mul(5u32.wrapping_mul(self.state[1]).rotate_left(7));
        let t = self.state[1] << 9;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(11);
        e
    }
}

fn mime_from_magic(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 4 && bytes[0..4] == [0x89, b'P', b'N', b'G'] {
        "image/png"
    } else if bytes.len() >= 4 && bytes[0..4] == [b'G', b'I', b'F', b'8'] {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.len() >= 4
        && bytes[0] == 0xff
        && bytes[1] == 0xd8
        && bytes[bytes.len() - 2] == 0xff
        && bytes[bytes.len() - 1] == 0xd9
    {
        "image/jpeg"
    } else {
        "image/webp"
    }
}

struct SeedRandom {
    arc4: Arc4,
}

impl SeedRandom {
    fn new(seed: &str) -> Self {
        Self {
            arc4: Arc4::new(&Self::mix_key(seed)),
        }
    }

    fn next_double(&mut self) -> f64 {
        const WIDTH: f64 = 256.0;
        const CHUNKS: usize = 6;
        const DIGITS: i32 = 52;
        let mut n = self.arc4.g(CHUNKS) as f64;
        let mut d = WIDTH.powi(CHUNKS as i32);
        let mut x = 0u64;
        let significance = 2f64.powi(DIGITS);
        let overflow = significance * 2.0;
        while n < significance {
            n = (n + x as f64) * WIDTH;
            d *= WIDTH;
            x = self.arc4.g(1);
        }
        while n >= overflow {
            n /= 2.0;
            d /= 2.0;
            x >>= 1;
        }
        (n + x as f64) / d
    }

    fn shuffle_indices(mut self, size: usize) -> Vec<usize> {
        let mut keys = (0..size).collect::<Vec<_>>();
        let mut order = Vec::with_capacity(size);
        while !keys.is_empty() {
            let index = (self.next_double() * keys.len() as f64).floor() as usize;
            order.push(keys.remove(index.min(keys.len() - 1)));
        }
        order
    }

    fn mix_key(seed: &str) -> Vec<u8> {
        const WIDTH: usize = 256;
        const MASK: usize = WIDTH - 1;
        let mut key = [0u8; WIDTH];
        let mut smear = 0u8;
        for (index, byte) in seed.bytes().enumerate() {
            let slot = index & MASK;
            smear ^= key[slot].wrapping_mul(19);
            key[slot] = smear.wrapping_add(byte) & MASK as u8;
        }
        let len = seed.len().min(WIDTH);
        key[..len].to_vec()
    }
}

struct Arc4 {
    i: usize,
    j: usize,
    s: [usize; 256],
}

impl Arc4 {
    fn new(key: &[u8]) -> Self {
        const WIDTH: usize = 256;
        const MASK: usize = WIDTH - 1;
        let effective_key = if key.is_empty() { &[0][..] } else { key };
        let mut s = [0usize; WIDTH];
        for (index, value) in s.iter_mut().enumerate() {
            *value = index;
        }
        let mut j_counter = 0usize;
        for k in 0..WIDTH {
            let t = s[k];
            j_counter = MASK & (j_counter + effective_key[k % effective_key.len()] as usize + t);
            s[k] = s[j_counter];
            s[j_counter] = t;
        }
        let mut arc4 = Self { i: 0, j: 0, s };
        arc4.g(WIDTH);
        arc4
    }

    fn g(&mut self, count: usize) -> u64 {
        const WIDTH: u64 = 256;
        const MASK: usize = 255;
        let mut r = 0u64;
        for _ in 0..count {
            self.i = MASK & (self.i + 1);
            let t = self.s[self.i];
            self.j = MASK & (self.j + t);
            let sj = self.s[self.j];
            self.s[self.i] = sj;
            self.s[self.j] = t;
            r = r
                .wrapping_mul(WIDTH)
                .wrapping_add(self.s[MASK & (sj + t)] as u64);
        }
        r
    }
}

struct GuardianRandomizer {
    next: u64,
}

impl GuardianRandomizer {
    fn new(key: &str) -> Self {
        let mut seed = 0u64;
        let chars = key.chars().collect::<Vec<_>>();
        let mut index = 0;
        while index < chars.len() {
            let high = chars[index] as u32;
            index += 1;
            let low = chars.get(index).copied().unwrap_or('\0') as u32;
            index += 1;
            seed += ((high << 8) | low) as u64;
        }
        Self { next: seed }
    }

    fn next_int(&mut self) -> usize {
        self.next = (self.next * 1_103_515_245 + 12_345) % 32_768;
        self.next as usize
    }

    fn rand(&mut self, max: usize) -> usize {
        let n = max + 1;
        self.next_int() / ((32_767 / n) + 1)
    }
}

pub struct ChunkedImages;

impl ChunkedImages {
    pub fn process_vertical_merge(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(sizes) = part_sizes(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::merge_base64(input, &sizes).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn merge_base64(input: &str, sizes: &[usize]) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        let mut offset = 0usize;
        let mut images = Vec::new();
        for size in sizes {
            let end = offset.checked_add(*size)?;
            let part = bytes.get(offset..end)?;
            images.push(image::load_from_memory(part).ok()?.to_rgba8());
            offset = end;
        }
        if images.is_empty() {
            return None;
        }
        let width = images.iter().map(|image| image.width()).max()?;
        let height = images.iter().map(|image| image.height()).sum();
        let mut output = RgbaImage::new(width, height);
        let mut y = 0;
        for image in images {
            copy_rect(
                &image,
                &mut output,
                0,
                0,
                0,
                y,
                image.width(),
                image.height(),
            );
            y += image.height();
        }
        encode_jpeg_base64(DynamicImage::ImageRgba8(output))
    }
}

pub struct SpeedBinb;

impl SpeedBinb {
    pub fn process_page_image(request: Value) -> ExtensionResult<ProcessedImage> {
        let Some(input) = image_base64(&request) else {
            return Ok(passthrough_processed_image(&request));
        };
        let Some(meta) = request
            .get("page")
            .and_then(|page| page.get("extra"))
            .and_then(|extra| extra.get("speedbinb"))
        else {
            return Ok(passthrough_processed_image(&request));
        };
        Ok(processed_jpeg(
            Self::descramble_base64(input, meta).unwrap_or_else(|| input.to_string()),
        ))
    }

    pub fn descramble_base64(input: &str, meta: &Value) -> Option<String> {
        let bytes = STANDARD.decode(input).ok()?;
        let image = image::load_from_memory(&bytes).ok()?;
        let coords = if let Some(coords) = meta.get("coords").and_then(Value::as_array) {
            coords
                .iter()
                .filter_map(parse_translation_value)
                .collect::<Vec<_>>()
        } else {
            let s = meta.get("s").and_then(Value::as_str)?;
            let u = meta.get("u").and_then(Value::as_str)?;
            ptbinb_translations(s, u, image.width(), image.height())
        };
        if coords.is_empty() {
            return encode_jpeg_base64(image);
        }
        apply_translations(image, &coords)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Translation {
    xsrc: u32,
    ysrc: u32,
    width: u32,
    height: u32,
    xdest: u32,
    ydest: u32,
}

fn parse_translation_value(value: &Value) -> Option<Translation> {
    if let Some(raw) = value.as_str() {
        return parse_ptimg_coord(raw);
    }
    Some(Translation {
        xsrc: value.get("xsrc")?.as_u64()? as u32,
        ysrc: value.get("ysrc")?.as_u64()? as u32,
        width: value.get("width")?.as_u64()? as u32,
        height: value.get("height")?.as_u64()? as u32,
        xdest: value.get("xdest")?.as_u64()? as u32,
        ydest: value.get("ydest")?.as_u64()? as u32,
    })
}

fn parse_ptimg_coord(raw: &str) -> Option<Translation> {
    let rest = raw.strip_prefix("i:")?;
    let (source, dest) = rest.split_once('>')?;
    let (xy, size) = source.split_once('+')?;
    let (xsrc, ysrc) = xy.split_once(',')?;
    let (width, height) = size.split_once(',')?;
    let (xdest, ydest) = dest.split_once(',')?;
    Some(Translation {
        xsrc: xsrc.parse().ok()?,
        ysrc: ysrc.parse().ok()?,
        width: width.parse().ok()?,
        height: height.parse().ok()?,
        xdest: xdest.parse().ok()?,
        ydest: ydest.parse().ok()?,
    })
}

fn apply_translations(image: DynamicImage, coords: &[Translation]) -> Option<String> {
    let source = image.to_rgba8();
    let canvas_width = coords
        .iter()
        .map(|coord| coord.xdest.saturating_add(coord.width))
        .max()
        .unwrap_or(source.width())
        .max(1);
    let canvas_height = coords
        .iter()
        .map(|coord| coord.ydest.saturating_add(coord.height))
        .max()
        .unwrap_or(source.height())
        .max(1);
    let mut target = RgbaImage::new(canvas_width, canvas_height);
    for coord in coords {
        copy_translation(&source, &mut target, *coord);
    }
    encode_jpeg_base64(DynamicImage::ImageRgba8(target))
}

fn copy_translation(source: &RgbaImage, target: &mut RgbaImage, coord: Translation) {
    if coord.width == 0
        || coord.height == 0
        || coord.xsrc.saturating_add(coord.width) > source.width()
        || coord.ysrc.saturating_add(coord.height) > source.height()
        || coord.xdest.saturating_add(coord.width) > target.width()
        || coord.ydest.saturating_add(coord.height) > target.height()
    {
        return;
    }
    copy_rect(
        source,
        target,
        coord.xsrc,
        coord.ysrc,
        coord.xdest,
        coord.ydest,
        coord.width,
        coord.height,
    );
}

fn ptbinb_translations(s: &str, u: &str, width: u32, height: u32) -> Vec<Translation> {
    if s.starts_with('=') && u.starts_with('=') {
        ptbinb_f_translations(s, u, width, height)
    } else {
        ptbinb_a_translations(s, u, width, height)
    }
}

fn ptbinb_f_translations(s: &str, u: &str, width: u32, height: u32) -> Vec<Translation> {
    let Some(src_data) = parse_ptbinb_f_key(s) else {
        return Vec::new();
    };
    let Some(dst_data) = parse_ptbinb_f_key(u) else {
        return Vec::new();
    };
    if src_data.width_pieces != dst_data.width_pieces
        || src_data.height_pieces != dst_data.height_pieces
        || src_data.padding != dst_data.padding
        || src_data.sign != '-'
        || dst_data.sign != '+'
        || src_data.width_pieces < 8
        || src_data.height_pieces < 8
    {
        return Vec::new();
    }
    let width_pieces = src_data.width_pieces;
    let height_pieces = src_data.height_pieces;
    let padding = src_data.padding;
    let expected = width_pieces + height_pieces + width_pieces * height_pieces;
    if src_data.encoded.len() != expected as usize || dst_data.encoded.len() != expected as usize {
        return Vec::new();
    }
    let src_tnp = decode_ptbinb_f_piece_data(&src_data.encoded, width_pieces, height_pieces);
    let dst_tnp = decode_ptbinb_f_piece_data(&dst_data.encoded, width_pieces, height_pieces);
    if src_tnp.pieces.len() != (width_pieces * height_pieces) as usize
        || dst_tnp.pieces.len() != (width_pieces * height_pieces) as usize
    {
        return Vec::new();
    }
    let horizontal_padding = 2 * width_pieces * padding;
    let vertical_padding = 2 * height_pieces * padding;
    if width < 64 + horizontal_padding
        || height < 64 + vertical_padding
        || width * height < (320 + horizontal_padding) * (320 + vertical_padding)
    {
        return vec![Translation {
            xsrc: 0,
            ysrc: 0,
            width,
            height,
            xdest: 0,
            ydest: 0,
        }];
    }
    let canvas_width = width - horizontal_padding;
    let canvas_height = height - vertical_padding;
    let piece_width = canvas_width.div_ceil(width_pieces);
    let remainder_width = canvas_width - (width_pieces - 1) * piece_width;
    let piece_height = canvas_height.div_ceil(height_pieces);
    let remainder_height = canvas_height - (height_pieces - 1) * piece_height;
    let mut coords = Vec::new();
    for index in 0..(width_pieces * height_pieces) {
        let h_pos = index % width_pieces;
        let w_pos = index / width_pieces;
        let source_index = src_tnp.pieces[index as usize] as usize;
        let Some(&dest_piece) = dst_tnp.pieces.get(source_index) else {
            continue;
        };
        let h_dst_pos = dest_piece % width_pieces;
        let w_dst_pos = dest_piece / width_pieces;
        coords.push(Translation {
            xsrc: padding
                + h_pos * (piece_width + 2 * padding)
                + if src_tnp.h_pos[w_pos as usize] < h_pos {
                    remainder_width.saturating_sub(piece_width)
                } else {
                    0
                },
            ysrc: padding
                + w_pos * (piece_height + 2 * padding)
                + if src_tnp.w_pos[h_pos as usize] < w_pos {
                    remainder_height.saturating_sub(piece_height)
                } else {
                    0
                },
            width: if src_tnp.h_pos[w_pos as usize] == h_pos {
                remainder_width
            } else {
                piece_width
            },
            height: if src_tnp.w_pos[h_pos as usize] == w_pos {
                remainder_height
            } else {
                piece_height
            },
            xdest: h_dst_pos * piece_width
                + if dst_tnp.h_pos[w_dst_pos as usize] < h_dst_pos {
                    remainder_width.saturating_sub(piece_width)
                } else {
                    0
                },
            ydest: w_dst_pos * piece_height
                + if dst_tnp.w_pos[h_dst_pos as usize] < w_dst_pos {
                    remainder_height.saturating_sub(piece_height)
                } else {
                    0
                },
        });
    }
    coords
}

struct PtBinbFKey {
    width_pieces: u32,
    height_pieces: u32,
    sign: char,
    padding: u32,
    encoded: String,
}

fn parse_ptbinb_f_key(key: &str) -> Option<PtBinbFKey> {
    let mut parts = key.strip_prefix('=')?.splitn(2, '-');
    let width_pieces = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    let (height_part, rest) = split_number_prefix(rest)?;
    let height_pieces = height_part.parse().ok()?;
    let mut chars = rest.chars();
    let sign = chars.next()?;
    if sign != '+' && sign != '-' {
        return None;
    }
    let rest = chars.as_str();
    let (padding_part, encoded) = rest.split_once('-')?;
    Some(PtBinbFKey {
        width_pieces,
        height_pieces,
        sign,
        padding: padding_part.parse().ok()?,
        encoded: encoded.to_string(),
    })
}

fn split_number_prefix(value: &str) -> Option<(&str, &str)> {
    let index = value
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(value.len());
    (index > 0).then(|| value.split_at(index))
}

struct PtBinbFTnp {
    w_pos: Vec<u32>,
    h_pos: Vec<u32>,
    pieces: Vec<u32>,
}

fn decode_ptbinb_f_piece_data(key: &str, width_pieces: u32, height_pieces: u32) -> PtBinbFTnp {
    let values = key.chars().map(ptbinb_f_index).collect::<Vec<_>>();
    let width = width_pieces as usize;
    let height = height_pieces as usize;
    PtBinbFTnp {
        w_pos: values.iter().take(width).copied().collect(),
        h_pos: values.iter().skip(width).take(height).copied().collect(),
        pieces: values.iter().skip(width + height).copied().collect(),
    }
}

fn ptbinb_f_index(ch: char) -> u32 {
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
        .find(ch)
        .unwrap_or(0) as u32
}

fn ptbinb_a_translations(s: &str, u: &str, width: u32, height: u32) -> Vec<Translation> {
    let Some(src_pieces) = calculate_ptbinb_a_pieces(u) else {
        return Vec::new();
    };
    let Some(dst_pieces) = calculate_ptbinb_a_pieces(s) else {
        return Vec::new();
    };
    if src_pieces.ndx != dst_pieces.ndx || src_pieces.ndy != dst_pieces.ndy {
        return Vec::new();
    }
    if width < 64 || height < 64 || width * height < 102_400 {
        return vec![Translation {
            xsrc: 0,
            ysrc: 0,
            width,
            height,
            xdest: 0,
            ydest: 0,
        }];
    }
    let n = width - width % 8;
    let piece_width = ((n - 1) / 7) - ((n - 1) / 7) % 8;
    let e = n - 7 * piece_width;
    let s_height = height - height % 8;
    let piece_height = ((s_height - 1) / 7) - ((s_height - 1) / 7) % 8;
    let u_height = s_height - 7 * piece_height;
    let mut coords = Vec::new();
    for (src, dst) in src_pieces.pieces.iter().zip(dst_pieces.pieces.iter()) {
        coords.push(Translation {
            xsrc: src.x / 2 * piece_width + src.x % 2 * e,
            ysrc: src.y / 2 * piece_height + src.y % 2 * u_height,
            width: src.w / 2 * piece_width + src.w % 2 * e,
            height: src.h / 2 * piece_height + src.h % 2 * u_height,
            xdest: dst.x / 2 * piece_width + dst.x % 2 * e,
            ydest: dst.y / 2 * piece_height + dst.y % 2 * u_height,
        });
    }
    let right_edge = piece_width * (src_pieces.ndx - 1) + e;
    let bottom_edge = piece_height * (src_pieces.ndy - 1) + u_height;
    if right_edge < width {
        coords.push(Translation {
            xsrc: right_edge,
            ysrc: 0,
            width: width - right_edge,
            height: bottom_edge,
            xdest: right_edge,
            ydest: 0,
        });
    }
    if bottom_edge < height {
        coords.push(Translation {
            xsrc: 0,
            ysrc: bottom_edge,
            width,
            height: height - bottom_edge,
            xdest: 0,
            ydest: bottom_edge,
        });
    }
    coords
}

#[derive(Clone, Debug)]
struct PtBinbAPiece {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

struct PtBinbAPieces {
    ndx: u32,
    ndy: u32,
    pieces: Vec<PtBinbAPiece>,
}

fn calculate_ptbinb_a_pieces(key: &str) -> Option<PtBinbAPieces> {
    let parts = key.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let ndx = parts[0].parse::<u32>().ok()?;
    let ndy = parts[1].parse::<u32>().ok()?;
    let encoded = parts[2];
    if (ndx * ndy * 2) as usize != encoded.len() {
        return None;
    }
    let a = (ndx - 1) * (ndy - 1) - 1;
    let f = ndx - 1 + a;
    let c = ndy - 1 + f;
    let l = 1 + c;
    let chars = encoded.chars().collect::<Vec<_>>();
    let mut pieces = Vec::new();
    for index in 0..(ndx * ndy) {
        let x = ptbinb_a_index(chars[(2 * index) as usize]);
        let y = ptbinb_a_index(chars[(2 * index + 1) as usize]);
        let (w, h) = if index <= a {
            (2, 2)
        } else if index <= f {
            (2, 1)
        } else if index <= c {
            (1, 2)
        } else if index <= l {
            (1, 1)
        } else {
            (0, 0)
        };
        pieces.push(PtBinbAPiece { x, y, w, h });
    }
    Some(PtBinbAPieces { ndx, ndy, pieces })
}

fn ptbinb_a_index(ch: char) -> u32 {
    "aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ"
        .find(ch)
        .unwrap_or(0) as u32
}

fn part_sizes(request: &Value) -> Option<Vec<usize>> {
    let from_headers = request
        .get("imageHeaders")
        .or_else(|| request.get("image_headers"))
        .or_else(|| request.get("image").and_then(|image| image.get("headers")))
        .and_then(|headers| {
            headers
                .get("X-Part-Sizes")
                .or_else(|| headers.get("x-part-sizes"))
                .and_then(Value::as_str)
        })
        .map(parse_part_size_csv);
    if let Some(sizes) = from_headers.filter(|sizes| !sizes.is_empty()) {
        return Some(sizes);
    }
    let extra = request.get("page")?.get("extra")?;
    if let Some(array) = extra.get("partSizes").and_then(Value::as_array) {
        return Some(
            array
                .iter()
                .filter_map(Value::as_u64)
                .map(|value| value as usize)
                .collect(),
        );
    }
    extra
        .get("partSizes")
        .and_then(Value::as_str)
        .map(parse_part_size_csv)
}

fn response_header(request: &Value, name: &str) -> Option<String> {
    for container in [
        request.get("responseHeaders"),
        request.get("response_headers"),
        request.get("imageHeaders"),
        request.get("image_headers"),
        request.get("image").and_then(|image| image.get("headers")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(value) = header_value(container, name) {
            return Some(value);
        }
    }
    None
}

fn header_value(headers: &Value, name: &str) -> Option<String> {
    if let Some(value) = headers.get(name).and_then(Value::as_str) {
        return Some(value.to_string());
    }
    let lower = name.to_ascii_lowercase();
    headers.as_object()?.iter().find_map(|(key, value)| {
        (key.to_ascii_lowercase() == lower)
            .then(|| value.as_str().map(ToOwned::to_owned))
            .flatten()
    })
}

fn parse_part_size_csv(value: &str) -> Vec<usize> {
    value
        .split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect()
}

pub fn decode_base64_any(input: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(input)
        .or_else(|_| URL_SAFE.decode(input))
        .or_else(|_| URL_SAFE_NO_PAD.decode(input))
        .ok()
}

pub fn decode_hex(input: &str) -> Option<Vec<u8>> {
    let compact = input.trim().replace([' ', ':', '-'], "");
    if compact.len() % 2 != 0 {
        return None;
    }
    (0..compact.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&compact[index..index + 2], 16).ok())
        .collect()
}

pub fn parse_csv_u32(input: &str) -> Vec<u32> {
    input
        .trim_matches(&['[', ']'][..])
        .replace(' ', "")
        .split(',')
        .filter_map(|value| value.parse::<u32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn parses_scramble_csv() {
        assert_eq!(parse_csv_u32("[1, 2,3]"), vec![1, 2, 3]);
    }

    #[test]
    fn mapped_grid_moves_tiles() {
        let mut source = RgbaImage::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                source.put_pixel(x, y, Rgba([x as u8, y as u8, 0, 255]));
            }
        }
        let image = DynamicImage::ImageRgba8(source);
        let result = TileGrid::descramble_image(image, &[3, 2, 1, 0], 2, 2)
            .unwrap()
            .to_rgba8();
        assert_eq!(result.get_pixel(0, 0).0, [8, 8, 0, 255]);
    }

    #[test]
    fn decodes_hex_with_separators() {
        assert_eq!(decode_hex("00:0f-10 ff"), Some(vec![0, 15, 16, 255]));
    }

    #[test]
    fn parses_part_sizes_from_csv() {
        let request = serde_json::json!({"page":{"extra":{"partSizes":"10, 20,30"}}});
        assert_eq!(part_sizes(&request), Some(vec![10, 20, 30]));
    }

    #[test]
    fn guardian_block_shuffle_preserves_remainders() {
        let mut source = RgbaImage::new(200, 260);
        for y in 0..260 {
            for x in 0..200 {
                source.put_pixel(x, y, Rgba([x as u8, y as u8, 0, 255]));
            }
        }
        let result = GuardianBlockImage::descramble_image(&source, "ab");
        assert_eq!(result.dimensions(), source.dimensions());
        assert_eq!(result.get_pixel(199, 0), source.get_pixel(199, 0));
        assert_eq!(result.get_pixel(0, 259), source.get_pixel(0, 259));
        assert_eq!(
            GuardianBlockImage::shuffle_mapping(4, "ab"),
            GuardianBlockImage::shuffle_mapping(4, "ab")
        );
    }

    #[test]
    fn manga_mirai_parses_scramble_order() {
        let key = STANDARD.encode("[3, 2, 10]");
        assert_eq!(MangaMiraiImage::scramble_order(&key), Some(vec![3, 2, 10]));
    }

    #[test]
    fn piccoma_seed_matches_upstream_example() {
        assert_eq!(
            PiccomaImage::seed_from_image_url(
                "https://img.example/a/b/c/SONTGGB0G[TQ3FPT7ECYJC/page.jpg?expires=0#scrambled"
            ),
            Some("RNOTGGC1GZTQ3GQT6ECYKB".into())
        );
    }

    #[test]
    fn seed_random_shuffle_is_deterministic() {
        assert_eq!(
            SeedRandom::new("seed").shuffle_indices(5),
            SeedRandom::new("seed").shuffle_indices(5)
        );
    }

    #[test]
    fn strips_jsonp_payloads() {
        assert_eq!(strip_jsonp("cb({\"ok\":true})"), "{\"ok\":true}");
        assert_eq!(strip_jsonp("{\"ok\":true}"), "{\"ok\":true}");
    }
}
