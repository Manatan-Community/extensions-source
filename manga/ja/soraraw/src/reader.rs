// SPDX-License-Identifier: Apache-2.0
// Tile layouts follow the SoraRaw reader and the Apache-2.0 Keiyoushi implementation.
use std::{collections::BTreeMap, io::Cursor};

use aes::cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, Mac};
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader,
    RgbaImage,
};
use manatan_sdk::{services, Error, MangaPage, ProcessedImage, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

const PATH_MASK: &[u8] = b"202508055d0db38bae2e86cc41649f90";
const CANVA2_SALT: &str = "6a0248ad1ca4208275aed64d336e81595ecb149422a8e621f70e23b9f01b9c1c";
const TILE_LAYOUT: &str = "sorarawTileLayout";
const RASTER_SERVICE: &str = "image.raster.v1";
const MAX_NATIVE_RASTER_INPUT: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
pub(super) struct Chapter {
    pub id: u64,
    manga_id: u64,
    #[serde(default)]
    mode: String,
    #[serde(rename = "_b")]
    image_base: Option<String>,
    uuid: Option<String>,
    token: Option<String>,
    updated_at: Option<String>,
    n: Option<Value>,
}

impl Chapter {
    pub(super) fn manifest_url(&self, base: &str) -> Result<Url> {
        let mut url = https_base(base)?;
        url.path_segments_mut()
            .map_err(err)?
            .pop_if_empty()
            .push(&self.manga_id.to_string())
            .push(&format!("{}.json", self.id));
        if let Some(updated) = self.updated_at.as_deref() {
            let updated = chrono::DateTime::parse_from_rfc3339(updated).map_err(err)?;
            url.query_pairs_mut()
                .append_pair("t", &updated.timestamp_millis().to_string());
        }
        Ok(url)
    }

    pub(super) fn decode_image_url(&self, encrypted: &str) -> Result<String> {
        let key = self
            .uuid
            .as_deref()
            .ok_or_else(|| Error::new("SoraRaw image key is missing"))?;
        let key = hex::decode(key).map_err(err)?;
        let mut bytes = super::decode_base64(encrypted)?;
        xor(&mut bytes, PATH_MASK);
        if bytes.len() <= 16 {
            return Err(Error::new("SoraRaw image path is truncated"));
        }
        let (iv, bytes) = bytes.split_at_mut(16);
        ctr::Ctr128BE::<aes::Aes256>::new_from_slices(&key, iv)
            .map_err(err)?
            .apply_keystream(bytes);
        let path = std::str::from_utf8(bytes)
            .map_err(err)?
            .trim_start_matches('/');
        let mut url = https_base(
            self.image_base
                .as_deref()
                .ok_or_else(|| Error::new("SoraRaw image host is missing"))?,
        )?;
        for segment in path.split('/') {
            if segment.is_empty() || matches!(segment, "." | "..") {
                return Err(Error::new("SoraRaw image path is invalid"));
            }
            url.path_segments_mut()
                .map_err(err)?
                .pop_if_empty()
                .push(segment);
        }
        Ok(url.into())
    }

    pub(super) fn image_metadata(&self) -> Result<BTreeMap<String, Value>> {
        let layout = match self.mode.as_str() {
            "canva2" => {
                let grid = self
                    .n
                    .as_ref()
                    .and_then(|value| {
                        value
                            .as_u64()
                            .or_else(|| value.as_str()?.trim().parse().ok())
                    })
                    .filter(|&grid| (1..=64).contains(&grid))
                    .unwrap_or(8) as u32;
                Layout::Canva2 { id: self.id, grid }
            }
            "canva" => {
                let token = self
                    .token
                    .as_deref()
                    .ok_or_else(|| Error::new("SoraRaw tile token is missing"))?;
                let mut hmac = Hmac::<Sha256>::new_from_slice(token.as_bytes()).map_err(err)?;
                hmac.update(self.id.to_string().as_bytes());
                Layout::Canva(hex::encode(hmac.finalize().into_bytes()))
            }
            _ => return Ok(BTreeMap::new()),
        };
        Ok(BTreeMap::from_iter([(
            TILE_LAYOUT.to_owned(),
            json!(layout),
        )]))
    }
}

fn https_base(base: &str) -> Result<Url> {
    let url = Url::parse(base).map_err(err)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::new("SoraRaw image base must be an HTTPS URL"));
    }
    Ok(url)
}

fn err(error: impl std::fmt::Debug) -> Error {
    Error::new(format!("SoraRaw image: {error:?}"))
}

pub(super) fn xor(bytes: &mut [u8], key: &[u8]) {
    for (byte, key_byte) in bytes.iter_mut().zip(key.iter().cycle()) {
        *byte ^= key_byte;
    }
}

#[derive(serde::Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Layout {
    Canva(String),
    Canva2 { id: u64, grid: u32 },
}

pub(super) fn process(page: &MangaPage, bytes: &[u8]) -> Result<Option<ProcessedImage>> {
    let Some(layout) = page.extra.get(TILE_LAYOUT) else {
        return Ok(None);
    };
    let layout: Layout = serde_json::from_value(layout.clone())?;
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(err)?;
    let jpeg = reader.format() == Some(ImageFormat::Jpeg);
    let native =
        jpeg && bytes.len() <= MAX_NATIVE_RASTER_INPUT && services::is_available(RASTER_SERVICE);
    let source = if native {
        let (dimensions, rgba): (RasterDimensions, Vec<u8>) =
            services::invoke_binary(RASTER_SERVICE, &json!({ "operation": "decodeJpeg" }), bytes)?;
        RgbaImage::from_raw(dimensions.width, dimensions.height, rgba)
            .ok_or_else(|| Error::new("SoraRaw host returned an invalid raster"))?
    } else {
        decode_raster(reader)?
    };
    let image = match layout {
        Layout::Canva2 { id, grid } if (1..=64).contains(&grid) => canva2(&source, id, grid),
        Layout::Canva2 { .. } => return Err(Error::new("SoraRaw tile grid is invalid")),
        Layout::Canva(seed) => canva(&source, &seed),
    };
    let output = if native {
        let (_, png): ((), Vec<u8>) = services::invoke_binary(
            RASTER_SERVICE,
            &json!({
                "operation": "encodeOpaquePng",
                "width": image.width(),
                "height": image.height(),
            }),
            image.as_raw(),
        )?;
        png
    } else {
        encode_raster(image, jpeg)?
    };
    Ok(Some(ProcessedImage {
        bytes: output,
        mime_type: Some("image/png".into()),
    }))
}

#[derive(Deserialize)]
struct RasterDimensions {
    width: u32,
    height: u32,
}

fn decode_raster(mut reader: ImageReader<Cursor<&[u8]>>) -> Result<RgbaImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().map_err(err)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > 8_388_608 {
        return Err(Error::new("SoraRaw page exceeds the decoded pixel limit"));
    }
    limits.reserve(decoder.total_bytes()).map_err(err)?;
    decoder.set_limits(limits).map_err(err)?;
    Ok(DynamicImage::from_decoder(decoder)
        .map_err(err)?
        .into_rgba8())
}

fn encode_raster(image: RgbaImage, jpeg: bool) -> Result<Vec<u8>> {
    let (width, height) = image.dimensions();
    let mut output = Vec::new();
    if jpeg {
        // A fixed row filter keeps lossless encoding practical on older mobile hosts.
        let rgb = DynamicImage::ImageRgba8(image).into_rgb8();
        PngEncoder::new_with_quality(&mut output, CompressionType::Fast, FilterType::Up)
            .write_image(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
            .map_err(err)?;
    } else {
        PngEncoder::new(&mut output)
            .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
            .map_err(err)?;
    }
    Ok(output)
}

#[derive(Clone, Copy)]
struct Tile {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

fn tiles(cols: &[u32], rows: &[u32]) -> Vec<Tile> {
    let mut tiles = Vec::new();
    let mut y = 0;
    for &h in rows {
        let mut x = 0;
        for &w in cols {
            tiles.push(Tile { x, y, w, h });
            x += w;
        }
        y += h;
    }
    tiles
}

fn groups(tiles: &[Tile]) -> Vec<((u32, u32), Vec<Tile>)> {
    let mut groups: Vec<((u32, u32), Vec<Tile>)> = Vec::new();
    for &tile in tiles {
        let size = (tile.w, tile.h);
        if let Some((_, members)) = groups.iter_mut().find(|(key, _)| *key == size) {
            members.push(tile);
        } else {
            groups.push((size, vec![tile]));
        }
    }
    groups
}

fn canva2(source: &RgbaImage, id: u64, grid: u32) -> RgbaImage {
    let (width, height) = source.dimensions();
    let digest = hex::encode(Sha256::digest(format!("{id}{CANVA2_SALT}")));
    let seed = Sha256::digest(format!("{digest}:nxn:{grid}:{width}x{height}"));
    let mut state = u32::from_be_bytes([seed[0], seed[1], seed[2], seed[3]]);
    let mut next = || {
        state = state.wrapping_add(0x6d2b79f5);
        let mut t = (state ^ (state >> 15)).wrapping_mul(state | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4294967296.0
    };
    let mut out = RgbaImage::new(width, height);
    for ((w, h), destinations) in groups(&tiles(&split(width, grid), &split(height, grid))) {
        let mut sources = destinations.clone();
        shuffle(&mut sources, &mut next);
        for (dst, src) in destinations.into_iter().zip(sources) {
            let rotation = if w == h {
                (next() * 4.0) as u8
            } else if next() < 0.5 {
                0
            } else {
                2
            };
            match rotation {
                0 | 2 => draw_rows(source, &mut out, src, dst, rotation == 2),
                _ => draw(source, &mut out, src, dst, rotation, false, false),
            }
        }
    }
    out
}

fn split(total: u32, count: u32) -> Vec<u32> {
    (0..count)
        .map(|i| total / count + u32::from(i < total % count))
        .collect()
}

fn shuffle<T>(values: &mut [T], next: &mut impl FnMut() -> f64) {
    for i in (1..values.len()).rev() {
        values.swap(i, (next() * (i + 1) as f64) as usize);
    }
}

fn draw_rows(source: &RgbaImage, out: &mut RgbaImage, src: Tile, dst: Tile, reversed: bool) {
    let stride = source.width() as usize * 4;
    let row_len = src.w as usize * 4;
    for y in 0..src.h {
        let src_y = if reversed { src.h - 1 - y } else { y };
        let from = (src.y + src_y) as usize * stride + src.x as usize * 4;
        let to = (dst.y + y) as usize * stride + dst.x as usize * 4;
        let src_row = &source.as_raw()[from..from + row_len];
        let dst_row = &mut out.as_mut()[to..to + row_len];
        if reversed {
            for (dst, src) in dst_row
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src_row.as_chunks::<4>().0.iter().rev())
            {
                *dst = *src;
            }
        } else {
            dst_row.copy_from_slice(src_row);
        }
    }
}

fn draw(
    source: &RgbaImage,
    out: &mut RgbaImage,
    src: Tile,
    dst: Tile,
    rotation: u8,
    flip_h: bool,
    flip_v: bool,
) {
    let (w, h) = if rotation % 2 == 0 {
        (src.w, src.h)
    } else {
        (src.h, src.w)
    };
    for y in 0..src.h {
        for x in 0..src.w {
            // The legacy canvas applies flips using the rotated bounds before rotating.
            let x_flip = if flip_h {
                i64::from(w) - 1 - i64::from(x)
            } else {
                i64::from(x)
            };
            let y_flip = if flip_v {
                i64::from(h) - 1 - i64::from(y)
            } else {
                i64::from(y)
            };
            let (ox, oy) = match rotation {
                1 => (y_flip, i64::from(w) - 1 - x_flip),
                2 => (i64::from(w) - 1 - x_flip, i64::from(h) - 1 - y_flip),
                3 => (i64::from(h) - 1 - y_flip, x_flip),
                _ => (x_flip, y_flip),
            };
            if ox >= 0 && oy >= 0 && ox < i64::from(dst.w) && oy < i64::from(dst.h) {
                out.put_pixel(
                    dst.x + ox as u32,
                    dst.y + oy as u32,
                    *source.get_pixel(src.x + x, src.y + y),
                );
            }
        }
    }
}

fn canva(source: &RgbaImage, seed: &str) -> RgbaImage {
    let (width, height) = source.dimensions();
    let mut rng = SeedRandom::new(seed);
    let tiles = tiles(
        &legacy_split(width, &mut rng),
        &legacy_split(height, &mut rng),
    );
    let groups = groups(&tiles);
    let mut out = RgbaImage::from_pixel(width, height, image::Rgba([255; 4]));
    let mut done = std::collections::BTreeSet::new();
    for &((w, h), ref destinations) in &groups {
        if !done.insert((w, h)) {
            continue;
        }
        let a = format!("{w}x{h}");
        let b = format!("{h}x{w}");
        let mut available = destinations.clone();
        if let Some((_, partner)) = groups.iter().find(|(size, _)| w != h && *size == (h, w)) {
            done.insert((h, w));
            let mut partner = partner.clone();
            let mut pair_rng = SeedRandom::new(&format!("{seed}|pair|{a}|{b}"));
            shuffle(&mut available, &mut || pair_rng.next());
            shuffle(&mut partner, &mut || pair_rng.next());
            let shared = available.len().min(partner.len());
            for i in 0..shared {
                draw_legacy(
                    source,
                    &mut out,
                    partner[i],
                    available[i],
                    &format!("{seed}|tf|AtoB|{a}|{i}"),
                    true,
                );
                draw_legacy(
                    source,
                    &mut out,
                    available[i],
                    partner[i],
                    &format!("{seed}|tf|BtoA|{b}|{i}"),
                    true,
                );
            }
            draw_group(
                source,
                &mut out,
                &available[shared..],
                &format!("{seed}|perm|A|{a}"),
                &format!("{seed}|tf|A|{a}|rest"),
            );
            draw_group(
                source,
                &mut out,
                &partner[shared..],
                &format!("{seed}|perm|B|{b}"),
                &format!("{seed}|tf|B|{b}|rest"),
            );
        } else {
            draw_group(
                source,
                &mut out,
                destinations,
                &format!("{seed}|perm|{a}"),
                &format!("{seed}|tf|self|{a}"),
            );
        }
    }
    out
}

fn legacy_split(total: u32, rng: &mut SeedRandom) -> Vec<u32> {
    let mut count = ((total + 111) / 222).max(1);
    while total / count > 259 {
        count += 1;
    }
    while total / count < 185 && count > 1 {
        count -= 1;
    }
    let mut sizes = split(total, count);
    shuffle(&mut sizes, &mut || rng.next());
    sizes
}

fn draw_group(
    source: &RgbaImage,
    out: &mut RgbaImage,
    destinations: &[Tile],
    perm_seed: &str,
    transform_seed: &str,
) {
    let mut available = destinations.to_vec();
    let mut rng = SeedRandom::new(perm_seed);
    shuffle(&mut available, &mut || rng.next());
    for (i, (&dst, src)) in destinations.iter().zip(available).enumerate() {
        draw_legacy(
            source,
            out,
            src,
            dst,
            &format!("{transform_seed}|{i}"),
            false,
        );
    }
}

fn draw_legacy(
    source: &RgbaImage,
    out: &mut RgbaImage,
    src: Tile,
    dst: Tile,
    seed: &str,
    swap: bool,
) {
    let mut rng = SeedRandom::new(seed);
    let roll = rng.next();
    let rotation = if swap {
        if roll < 0.5 {
            1
        } else {
            3
        }
    } else if dst.w == dst.h {
        (roll * 4.0) as u8
    } else if roll < 0.5 {
        0
    } else {
        2
    };
    draw(
        source,
        out,
        src,
        dst,
        rotation,
        rng.next() < 0.5,
        rng.next() < 0.5,
    );
}
struct SeedRandom {
    s: [u8; 256],
    i: usize,
    j: usize,
}
impl SeedRandom {
    fn new(seed: &str) -> Self {
        let mut key = [0u8; 256];
        let mut smear = 0i32;
        for (i, ch) in seed.encode_utf16().enumerate() {
            smear ^= i32::from(key[i & 255]) * 19;
            key[i & 255] = (smear.wrapping_add(i32::from(ch)) & 255) as u8;
        }
        let length = seed.encode_utf16().count().clamp(1, 256);
        let mut result = Self {
            s: std::array::from_fn(|i| i as u8),
            i: 0,
            j: 0,
        };
        let mut j = 0;
        for i in 0..256 {
            j = (j + key[i % length] as usize + result.s[i] as usize) & 255;
            result.s.swap(i, j);
        }
        for _ in 0..256 {
            result.g(1);
        }
        result
    }
    fn g(&mut self, count: u32) -> u64 {
        let mut result = 0u64;
        for _ in 0..count {
            self.i = (self.i + 1) & 255;
            let t = self.s[self.i];
            self.j = (self.j + t as usize) & 255;
            self.s.swap(self.i, self.j);
            result = result * 256 + self.s[(self.s[self.i] as usize + t as usize) & 255] as u64;
        }
        result
    }
    fn next(&mut self) -> f64 {
        let mut n = self.g(6) as f64;
        let mut d = 256_f64.powi(6);
        let mut x = 0;
        while n < 2_f64.powi(52) {
            n = (n + x as f64) * 256.;
            d *= 256.;
            x = self.g(1);
        }
        while n >= 2_f64.powi(53) {
            n /= 2.;
            d /= 2.;
            x >>= 1;
        }
        (n + x as f64) / d
    }
}
