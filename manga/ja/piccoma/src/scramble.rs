// SPDX-License-Identifier: Apache-2.0
// Piccoma tile layout and ARC4 PRNG adapted from keiyoushi/extensions-source.
use image::{GenericImage, GenericImageView, ImageFormat, ImageReader, RgbaImage};
use manatan_sdk::{Error, Result};
use std::{collections::BTreeMap, io::Cursor};
use url::Url;

const TILE: u32 = 50;
pub fn seed(candidate: &str) -> Result<String> {
    let url = Url::parse(candidate).map_err(|_| Error::new("Invalid Piccoma image URL"))?;
    let checksum = url
        .path_segments()
        .and_then(|mut p| p.nth(3))
        .ok_or_else(|| Error::new("Piccoma image checksum is missing"))?;
    let checksum = percent_encoding::percent_decode_str(checksum)
        .decode_utf8()
        .map_err(|_| Error::new("Invalid Piccoma image checksum encoding"))?;
    if checksum.is_empty() || !checksum.is_ascii() || checksum.len() > 32 {
        return Err(Error::new("Invalid Piccoma image checksum"));
    }
    let expires = url
        .query_pairs()
        .find(|(k, _)| k == "expires")
        .map(|(_, v)| v.into_owned())
        .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
        .ok_or_else(|| Error::new("Piccoma image expiry is missing"))?;
    let rotation = expires.bytes().map(|b| (b - b'0') as usize).sum::<usize>() % checksum.len();
    let split = checksum.len() - rotation;
    let mut bytes = format!("{}{}", &checksum[split..], &checksum[..split]).into_bytes();
    for (i, byte) in bytes.iter_mut().enumerate() {
        if (3_236_551_u32 >> i) & 1 == 1 {
            *byte ^= 1;
        }
    }
    String::from_utf8(bytes).map_err(|_| Error::new("Invalid Piccoma tile seed"))
}

pub fn decode(bytes: &[u8], seed: &str) -> Result<Vec<u8>> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(Error::new("Piccoma page exceeds the image size limit"));
    }
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image_error)?
        .into_dimensions()
        .map_err(image_error)?;
    if u64::from(width) * u64::from(height) > 8_388_608 {
        return Err(Error::new("Piccoma page exceeds the decoded pixel limit"));
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image_error)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let source = reader.decode().map_err(image_error)?.into_rgba8();
    let mut output = RgbaImage::new(source.width(), source.height());
    for ((w, h), tiles) in tile_groups(source.width(), source.height()) {
        let mut generator = SeedRandom::new(seed);
        let mut available = tiles.clone();
        for (x, y) in tiles {
            let index = (generator.next() * available.len() as f64).floor() as usize;
            let (sx, sy) = available.remove(index);
            output
                .copy_from(&source.view(sx, sy, w, h).to_image(), x, y)
                .map_err(image_error)?;
        }
    }
    let mut output_bytes = Cursor::new(Vec::new());
    output
        .write_to(&mut output_bytes, ImageFormat::Png)
        .map_err(image_error)?;
    Ok(output_bytes.into_inner())
}

fn image_error(e: impl std::fmt::Display) -> Error {
    Error::new(format!("Piccoma page image: {e}"))
}
fn tile_groups(width: u32, height: u32) -> BTreeMap<(u32, u32), Vec<(u32, u32)>> {
    let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for y in (0..height).step_by(TILE as usize) {
        for x in (0..width).step_by(TILE as usize) {
            groups
                .entry((TILE.min(width - x), TILE.min(height - y)))
                .or_default()
                .push((x, y));
        }
    }
    groups
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prng_matches_seedrandom_reference_vectors() {
        let mut rng = SeedRandom::new("hello.");
        for expected in [0.9282578795792454, 0.3752569768646784, 0.7316977468919549] {
            assert!((rng.next() - expected).abs() < 1e-15);
        }
        let mut rng = SeedRandom::new("RNOTGGC1GZTQ3GQT6ECYKB");
        let mut keys = (0..12).collect::<Vec<_>>();
        let actual = (0..12)
            .map(|_| {
                let i = (rng.next() * keys.len() as f64) as usize;
                keys.remove(i)
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, [11, 8, 3, 7, 2, 0, 4, 10, 9, 1, 6, 5]);
    }
    #[test]
    fn inverse_tile_mapping_preserves_pixels_and_partial_edges() {
        let (w, h) = (157, 113);
        let mut original = RgbaImage::new(w, h);
        for (x, y, pixel) in original.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8, y as u8, (x ^ y) as u8, 255]);
        }
        let mut scrambled = RgbaImage::new(w, h);
        for ((tw, th), tiles) in tile_groups(w, h) {
            let mut rng = SeedRandom::new("fixture");
            let mut available = tiles.clone();
            for (x, y) in tiles {
                let i = (rng.next() * available.len() as f64) as usize;
                let (sx, sy) = available.remove(i);
                scrambled
                    .copy_from(&original.view(x, y, tw, th).to_image(), sx, sy)
                    .unwrap();
            }
        }
        let mut bytes = Cursor::new(Vec::new());
        scrambled.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let decoded = decode(bytes.get_ref(), "fixture").unwrap();
        assert_eq!(
            image::load_from_memory(&decoded).unwrap().into_rgba8(),
            original
        );
    }
    #[test]
    fn malformed_seed_and_image_fail_with_errors() {
        assert_eq!(seed("https://piccoma.kakaocdn.net/dn/fixture/checksum/GB0G%5BTQ3FPT7ECYJCSONTG/001.jpg?expires=2000000003").unwrap(), "RNOTGGC1GZTQ3GQT6ECYKB");
        assert_eq!(seed("https://piccoma.kakaocdn.net/dn/fixture/checksum/GB0G[TQ3FPT7ECYJCSONTG/001.jpg?expires=2000000003").unwrap(), "RNOTGGC1GZTQ3GQT6ECYKB");
        assert!(seed("https://piccoma.kakaocdn.net/x").is_err());
        assert!(seed("https://piccoma.kakaocdn.net/a/b/c/abcd/x?expires=no").is_err());
        assert!(decode(b"not an image", "seed").is_err());
    }
}
