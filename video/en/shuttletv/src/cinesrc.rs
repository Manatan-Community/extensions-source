//! CineSrc protocol reference: yuzono/anime-extensions, ShuttleTvExtractor.kt
//! and CinesrcChallenge.kt, revision 7690eae6870c (Apache-2.0).
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use manatan_sdk::{client::Client, html, storage, Error, Result, VideoStream};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::num::Wrapping as W;
const ORIGIN: &str = "https://cinesrc.st";
const API: &str = "https://enc-dec.app/api";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.37";
fn error(s: impl Into<String>) -> Error {
    Error::new(s.into())
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .ok_or_else(|| error(format!("CineSrc response missing {k}")))
}
fn decode(s: &str) -> Result<Vec<u8>> {
    B64.decode(s.trim_end_matches('='))
        .map_err(|_| error("Invalid CineSrc encoded data"))
}
fn bytes32(s: &str) -> Result<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error("Invalid CineSrc seed"));
    }
    let mut b = [0; 32];
    for (i, x) in b.iter_mut().enumerate() {
        *x = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| error("Invalid CineSrc seed"))?;
    }
    Ok(b)
}
fn request(url: &str, body: Option<Vec<u8>>, headers: &[(String, String)]) -> Result<String> {
    let mut r = Client::browser()
        .header("User-Agent", UA)
        .request(if body.is_some() { "POST" } else { "GET" }, url)
        .timeout_ms(8_000)
        .max_body_bytes(2 * 1024 * 1024);
    for (k, v) in headers {
        r = r.header(k, v);
    }
    if let Some(b) = body {
        r = r.body(b);
    }
    let response = r.send()?.error_for_status()?;
    Ok(response.text()?.into())
}
fn json_request(url: &str, body: Option<Value>, headers: &[(String, String)]) -> Result<Value> {
    let mut h = headers.to_vec();
    let b = body.map(|b| {
        h.push(("Content-Type".into(), "application/json".into()));
        b.to_string().into_bytes()
    });
    serde_json::from_str(&request(url, b, &h)?).map_err(Error::from)
}
fn headers() -> Vec<(String, String)> {
    vec![
        ("Origin".into(), ORIGIN.into()),
        ("Referer".into(), format!("{ORIGIN}/")),
    ]
}
fn codec(kind: &str, body: Value) -> Result<Value> {
    let r = json_request(
        &format!("{API}/{kind}-cinesrc"),
        Some(body),
        &[("Referer".into(), "https://enc-dec.app/".into())],
    )?;
    r.get("result")
        .cloned()
        .filter(|v| !v.is_null())
        .ok_or_else(|| error("CineSrc codec service did not return a result"))
}
struct Session {
    embed: String,
    token: String,
    action: Value,
    state: Value,
}
fn prepare(embed: &str) -> Result<Session> {
    let u = url::Url::parse(embed).map_err(|_| error("Invalid CineSrc embed"))?;
    let p: Vec<_> = u.path().trim_matches('/').split('/').collect();
    if u.origin().ascii_serialization() != ORIGIN
        || p.len() != 3
        || p[0] != "embed"
        || !matches!(p[1], "movie" | "tv")
    {
        return Err(error("Invalid CineSrc embed"));
    }
    let params: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    let q = B64.encode(json!([p[1], p[2], params.get("s"), params.get("e")]).to_string());
    let mut h = headers();
    h.push(("x-cs-q".into(), q));
    let b: Value = serde_json::from_str(&request(
        &format!("{ORIGIN}/api/c/bootstrap"),
        Some(vec![]),
        &h,
    )?)?;
    h.extend([
        ("x-cs-b".into(), "20261014-a".into()),
        ("x-cs-r".into(), string(&b, "r")?.into()),
        ("x-cs-p".into(), string(&b, "p")?.into()),
    ]);
    let one = json_request(&format!("{ORIGIN}/api/c/issue"), None, &h)?;
    let seed = codec(
        "enc",
        json!({"type":"seed","data":{"url":embed,"user_agent":UA,"challenge_data":one}}),
    )?;
    let two = json_request(&format!("{ORIGIN}/api/c/stage2/issue"), None, &h)?;
    let solution1 = stage1(string(&one, "w")?, string(&seed, "seed")?)?;
    let solution2 = stage2(&two)?;
    let encrypted = codec(
        "enc",
        json!({"type":"encrypt","data":{"url":embed,"user_agent":UA,"state":seed["state"],"challenge_data":{"stage1":{"challenge":one,"solution":solution1},"stage2":{"challenge":two,"solution":solution2}}}}),
    )?;
    Ok(Session {
        embed: embed.into(),
        token: format!("{}::c3::{}", string(&encrypted, "token")?, string(&b, "r")?),
        action: encrypted["headers"].clone(),
        state: encrypted["state"].clone(),
    })
}
fn flight(text: &str) -> Result<Value> {
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .filter_map(|(_, v)| serde_json::from_str::<Value>(v).ok())
        .find(|v| v.is_array())
        .ok_or_else(|| error("CineSrc server list is missing"))
}
// Provider names are public metadata. The publisher calls this server action
// with [] and no playback proof; authorization belongs only to resolve().
fn provider_assets(page: &str) -> Result<Vec<String>> {
    let document = html::document(page);
    let selector = html::selector("script[src]")?;
    let origin = url::Url::parse(ORIGIN).unwrap();
    let mut assets = Vec::new();
    for node in document.select(&selector) {
        let Some(src) = node.value().attr("src") else {
            continue;
        };
        let Ok(url) = origin.join(src) else { continue };
        if url.origin() != origin.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || !url.path().starts_with("/_next/static/chunks/")
            || !url.path().ends_with(".js")
        {
            continue;
        }
        let url = url.to_string();
        if !assets.contains(&url) {
            assets.push(url);
        }
        if assets.len() > 32 {
            return Err(error("Too many CineSrc player assets"));
        }
    }
    if assets.is_empty() {
        return Err(error("CineSrc player assets are missing"));
    }
    Ok(assets)
}
fn valid_action(id: &str) -> bool {
    (40..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn provider_action(bundle: &str) -> Option<String> {
    for part in bundle.split("createServerReference").skip(1) {
        let Some(args) = part.trim_start_matches(')').strip_prefix("(\"") else {
            continue;
        };
        let Some((id, rest)) = args.split_once('"') else {
            continue;
        };
        let Some((arguments, _)) = rest.split_once(')') else {
            continue;
        };
        if valid_action(id) && arguments.trim_end().ends_with("\"getProviderList\"") {
            return Some(id.into());
        }
    }
    None
}
fn provider_action_for(embed: &str) -> Result<String> {
    let url = url::Url::parse(embed).map_err(|_| error("Invalid CineSrc embed"))?;
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if url.origin().ascii_serialization() != ORIGIN
        || !url.username().is_empty()
        || url.password().is_some()
        || parts.len() != 3
        || parts[0] != "embed"
        || !matches!(parts[1], "movie" | "tv")
    {
        return Err(error("Invalid CineSrc embed"));
    }
    let page = request(embed, None, &headers())?;
    let assets = provider_assets(&page)?;
    if let Ok(Some(cached)) = storage::get::<Value>("cinesrc", "provider-action") {
        if cached["assets"] == json!(assets) {
            if let Some(action) = cached["action"].as_str().filter(|s| valid_action(s)) {
                return Ok(action.to_owned());
            }
        }
    }
    // Route chunks are normally near the end. Fetch in small batches and stop
    // at the first matching action, with both body size and asset count bounded.
    let reversed: Vec<_> = assets.iter().rev().collect();
    for batch in reversed.chunks(3) {
        let requests = batch
            .iter()
            .map(|asset| {
                Client::browser()
                    .header("User-Agent", UA)
                    .get(asset.as_str())
                    .header("Referer", embed)
                    .timeout_ms(8_000)
                    .max_body_bytes(2 * 1024 * 1024)
            })
            .collect();
        for response in Client::send_many(requests, 3) {
            let Ok(response) = response else { continue };
            if response.status() != 200 {
                continue;
            }
            let Ok(text) = response.text() else { continue };
            if let Some(action) = provider_action(text) {
                // Public action IDs are cached against the exact published asset
                // list, so a deployment rotates them without shipping a package.
                let _ = storage::set(
                    "cinesrc",
                    "provider-action",
                    &json!({"assets":assets,"action":action}),
                );
                return Ok(action);
            }
        }
    }
    Err(error(
        "CineSrc provider action is missing from its published player",
    ))
}
pub fn servers(embed: &str) -> Result<Vec<VideoStream>> {
    let action = provider_action_for(embed)?;
    let mut h = headers();
    h.push(("Next-Action".into(), action));
    h.push(("Content-Type".into(), "text/plain;charset=UTF-8".into()));
    let providers = flight(&request(embed, Some(b"[]".to_vec()), &h)?)?;
    let rows = providers
        .as_array()
        .ok_or_else(|| error("Invalid CineSrc server list"))?;
    if rows.len() > 32 {
        return Err(error("Too many CineSrc servers"));
    }
    rows.iter()
        .map(|p| {
            let id = string(p, "id")?;
            let mut stream = VideoStream::default();
            stream.name = Some(p["name"].as_str().unwrap_or(id).into());
            stream.extra.insert("provider".into(), json!(id));
            stream.extra.insert("embed".into(), json!(embed));
            Ok(stream)
        })
        .collect()
}
pub fn resolve(embed: &str, provider: &str) -> Result<Option<VideoStream>> {
    let s = prepare(embed)?;
    let u = url::Url::parse(embed).map_err(|_| error("Invalid CineSrc embed"))?;
    let p: Vec<_> = u.path().trim_matches('/').split('/').collect();
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    let mut h = headers();
    h.push(("Next-Action".into(), string(&s.action, "getStream")?.into()));
    h.push(("Content-Type".into(), "text/plain;charset=UTF-8".into()));
    let body = json!([
        p[2],
        if p[1] == "movie" { "movie" } else { "show" },
        q.get("s").map(String::as_str).unwrap_or("$undefined"),
        q.get("e").map(String::as_str).unwrap_or("$undefined"),
        s.token,
        provider
    ]);
    let text = request(&s.embed, Some(body.to_string().into_bytes()), &h)?;
    if text.contains("e1:invalid_challenge") {
        return Err(error("CineSrc rejected verification"));
    }
    let result = codec("dec", json!({"text":text,"state":s.state}))?;
    if result["error"].is_string() {
        return Err(error(
            "This CineSrc server has no stream for the selected title",
        ));
    }
    let raw = result["url"]
        .as_array()
        .and_then(|a| a.iter().find_map(|v| v["url"].as_str()))
        .ok_or_else(|| error("CineSrc returned no media"))?;
    let url = url::Url::parse(ORIGIN)
        .unwrap()
        .join(raw)
        .map_err(|_| error("Invalid CineSrc media URL"))?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(error("Unsafe CineSrc media URL"));
    }
    let mut stream = VideoStream {
        url: url.into(),
        name: Some(provider.into()),
        format: Some("hls".into()),
        is_hls: true,
        requires_proxy: true,
        initialized: true,
        ..Default::default()
    };
    stream.headers.extend(headers());
    stream.headers.insert("User-Agent".into(), UA.into());
    Ok(Some(stream))
}
fn rot(x: W<u32>, n: u32) -> W<u32> {
    W(x.0.rotate_left(n))
}
fn stage1(encoded: &str, seed: &str) -> Result<String> {
    let c = decode(encoded)?;
    if c.len() != 32 || c[16..20] != [0x7d, 0xbd, 0xd2, 0x43] || c[20] != 4 || &c[28..] != b"CSP4" {
        return Err(error("Unsupported CineSrc challenge"));
    }
    let (mode, diff, v, r, mb, si, pi) = (
        c[21],
        c[22],
        c[23] as usize,
        c[24] as usize,
        c[25] as usize,
        c[26] as usize,
        c[27] as usize,
    );
    if !matches!((mode, r, mb), (0, 12, 15) | (1, 16, 17))
        || !(1..=30).contains(&diff)
        || v > 7
        || si > 7
        || pi > 7
    {
        return Err(error("Unsupported CineSrc proof parameters"));
    }
    let w: Vec<_> = c[..16]
        .chunks_exact(4)
        .map(|x| W(u32::from_le_bytes(x.try_into().unwrap())))
        .collect();
    let mut boxx = vec![W(0u32); 1 << mb];
    let k: Vec<_> = K.iter().copied().map(W).collect();
    let mut x = w[v & 3] ^ k[si] ^ (k[8 + si] * W(v as u32 + 1));
    let mut y = w[(v + 1) & 3] ^ k[15 - si];
    for (i, s) in boxx.iter_mut().enumerate() {
        x += w[(i + si) & 3] + k[(i + v) & 15] * W(i as u32 + 1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        y = rot(
            (k[(i + 9) & 15] * W(i as u32 + 3)) ^ y ^ x,
            (i + v + si) as u32,
        );
        *s = (k[(i + 5) & 15] * W((i ^ v) as u32)) ^ rot(w[(i + v) & 3], (i + si) as u32) ^ x ^ y;
    }
    let mut msg = [0u8; 88];
    msg[24..40].copy_from_slice(&c[..16]);
    msg[48..56].copy_from_slice(&[
        4,
        mode,
        v as u8,
        si as u8,
        pi as u8,
        r as u8,
        ((mb as u16) ^ 0x6b29) as u8,
        (((mb as u16) ^ 0x6b29) >> 8) as u8,
    ]);
    msg[56..].copy_from_slice(&bytes32(seed)?);
    for n in 0..(1u64 << 24) {
        let lo = W(n as u32);
        let hi = W((n >> 32) as u32);
        let (mut a, mut b, mut cc, mut d, mut e, mut f) = (
            w[0] ^ k[v] ^ lo,
            rot(lo, (v + 7) as u32) ^ w[2],
            w[1] ^ k[8 + si] ^ hi,
            rot(lo ^ hi, (si + 11) as u32) ^ w[3],
            k[si + v] ^ lo ^ hi,
            k[15 - v] ^ w[si & 3],
        );
        for rr in 0..r {
            let r = rr as u32;
            let mask = (1u32 << mb) - 1;
            let v1 = boxx[((rot(b, r + 1) ^ (k[rr & 15] * f) ^ e ^ a).0 & mask) as usize];
            let i2 =
                ((rot(e, v as u32 + r + 3) ^ (k[(rr + 7) & 15] * d) ^ cc ^ v1).0 & mask) as usize;
            let op = SCHEDULES[si][rr];
            match op {
                0 => {
                    a = rot(a + d + v1, 5);
                    cc = (cc ^ f ^ a) * W(1187432225) + b
                }
                1 => {
                    b = rot(cc ^ e ^ b ^ v1, 11);
                    d += (b ^ a) * W(1110491921)
                }
                2 => {
                    cc = rot(cc + b + v1, 17);
                    a = ((d ^ f ^ cc) * W(404896285)) ^ a
                }
                3 => {
                    d = rot(a ^ d ^ v1, 23);
                    b += (cc ^ e ^ d) * W(1446550439)
                }
                4 => {
                    a = (a ^ e ^ v1) * W((-727158525i32) as u32) + rot(b, 7);
                    d = rot(cc + f + a, 13) ^ d
                }
                5 => {
                    cc = (cc + f + v1) * W(130784815) ^ rot(d, 9);
                    b += rot(a ^ e ^ cc, 19)
                }
                6 => {
                    b = (b ^ f ^ v1) * W(1929928605) + rot(a, 3);
                    cc = rot(d + e + b, 15) ^ cc
                }
                _ => {
                    d = (d + e + v1) * W((-767989217i32) as u32) ^ rot(cc, 21);
                    a += rot(b ^ f ^ d, 27)
                }
            }
            let v2 = boxx[i2];
            if (f ^ W(r) ^ v1).0 & 1 == 0 {
                f = rot(v2 + f + a, r + 5);
                e = k[8 + op] * (v2 ^ e ^ f) + d;
            } else {
                e = rot(v2 ^ e ^ b, r + 9);
                f = (k[(op + 11) & 15] * (v2 + f + e)) ^ cc;
            }
        }
        let out = [a, cc, b, d, f, e];
        for (i, &j) in PERMUTATIONS[pi].iter().enumerate() {
            msg[i * 4..i * 4 + 4].copy_from_slice(&out[j].0.to_le_bytes());
        }
        msg[40..48].copy_from_slice(&n.to_le_bytes());
        let hash = Sha256::digest(msg);
        if leading(&hash, diff as usize) {
            return Ok(format!("m4.{n:x}"));
        }
    }
    Err(error("CineSrc proof exceeded its bounded work budget"))
}
fn leading(hash: &[u8], bits: usize) -> bool {
    hash[..bits / 8].iter().all(|b| *b == 0)
        && (bits % 8 == 0 || hash[bits / 8] >> (8 - bits % 8) == 0)
}
fn stage2(data: &Value) -> Result<String> {
    let a = data["pack"]
        .as_array()
        .filter(|a| a.len() >= 5)
        .ok_or_else(|| error("Invalid CineSrc second challenge"))?;
    let rev = |i: usize| {
        a[i].as_str()
            .map(|s| s.chars().rev().collect::<String>())
            .ok_or_else(|| error("Invalid CineSrc challenge field"))
    };
    let target = rev(0)?;
    let salt = rev(3)?;
    let blob = rev(4)?;
    let body = String::from_utf8(decode(
        blob.split('.')
            .nth(1)
            .ok_or_else(|| error("Invalid CineSrc challenge blob"))?,
    )?)
    .map_err(|_| error("Invalid challenge text"))?;
    let payload: Value = serde_json::from_slice(&decode(
        body.split_once('.')
            .ok_or_else(|| error("Invalid challenge payload"))?
            .1,
    )?)?;
    let d = payload["d"]
        .as_u64()
        .filter(|d| *d <= 24)
        .ok_or_else(|| error("Unsupported second challenge difficulty"))?;
    for i in 0..1u32 << d {
        let key = format!("{:0width$x}", i, width = d.div_ceil(4) as usize);
        let actual = format!("{:x}", Sha256::digest(format!("{salt}{key}")));
        if actual == target {
            return Ok(key);
        }
    }
    Err(error("CineSrc second proof failed"))
}

const K: [u32; 16] = [
    0x46C6C721, 0x4230C311, 0x18223A1D, 0x56389BA7, 0xD4A87103, 0x07CB9E2F, 0x73085F9D, 0xD2396A1F,
    0x23AE5ADD, 0x4646C48F, 0x0F4E0903, 0x808F5F31, 0x953D4E07, 0x7E238FFF, 0x634595DF, 0xCB5E4301,
];
const PERMUTATIONS: [[usize; 6]; 8] = [
    [3, 5, 1, 0, 2, 4],
    [2, 0, 4, 5, 3, 1],
    [1, 4, 5, 3, 0, 2],
    [2, 0, 1, 5, 4, 3],
    [4, 2, 1, 0, 3, 5],
    [5, 0, 2, 1, 3, 4],
    [4, 5, 1, 0, 3, 2],
    [4, 0, 3, 5, 1, 2],
];
const SCHEDULES: [[usize; 16]; 8] = [
    [0, 7, 6, 3, 2, 1, 5, 4, 0, 2, 4, 6, 5, 1, 7, 3],
    [5, 2, 1, 3, 0, 4, 6, 7, 4, 1, 0, 7, 3, 6, 5, 2],
    [4, 0, 2, 1, 5, 7, 6, 3, 2, 6, 0, 7, 4, 3, 5, 1],
    [5, 4, 7, 0, 2, 3, 1, 6, 5, 1, 4, 0, 7, 2, 3, 6],
    [4, 7, 0, 2, 1, 6, 5, 3, 6, 1, 3, 2, 0, 7, 4, 5],
    [0, 7, 2, 6, 3, 4, 5, 1, 6, 5, 0, 2, 7, 1, 4, 3],
    [0, 4, 5, 3, 1, 7, 2, 6, 3, 7, 5, 6, 1, 0, 2, 4],
    [7, 6, 0, 4, 2, 5, 1, 3, 7, 5, 3, 4, 2, 6, 0, 1],
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn solves_known_second_stage_hash_and_rejects_unbounded_work() {
        let payload = B64.encode(br#"{"d":8}"#);
        let body = B64.encode(format!("prefix.{payload}"));
        let blob = format!("header.{body}.signature");
        let target = format!("{:x}", Sha256::digest(b"salt7b"));
        let data = json!({"pack":[target.chars().rev().collect::<String>(),"","","tlas",blob.chars().rev().collect::<String>()]});
        assert_eq!(stage2(&data).unwrap(), "7b");
        let payload = B64.encode(br#"{"d":31}"#);
        let body = B64.encode(format!("prefix.{payload}"));
        let mut invalid = data;
        invalid["pack"][4] = json!(format!("header.{body}.signature")
            .chars()
            .rev()
            .collect::<String>());
        assert!(stage2(&invalid).is_err());
    }
    #[test]
    fn rejects_bad_challenge_and_seed_without_panicking() {
        assert!(stage1(&B64.encode([0; 32]), &"0".repeat(64)).is_err());
        assert!(bytes32(&"é".repeat(32)).is_err());
    }
    #[test]
    fn provider_list_requires_real_flight_array() {
        assert_eq!(
            flight("0:{\"a\":\"$@1\"}\n1:[{\"id\":\"nebula\"}]").unwrap()[0]["id"],
            "nebula"
        );
        assert!(flight("<!doctype html>").is_err());
    }
}

#[cfg(test)]
mod provider_metadata_tests {
    use super::*;
    #[test]
    fn publisher_action_is_distinguished_from_stream_authorization() {
        let action = "00112233445566778899aabbccddeeff0011223344";
        let js = format!("x.createServerReference)(\"{action}\",x.callServer,void 0,x.findSourceMapURL,\"getProviderList\"), y=x.createServerReference)(\"ffeeddccbbaa99887766554433221100ffeeddccbb\",x.callServer,void 0,x.findSourceMapURL,\"getStream\")");
        assert_eq!(provider_action(&js).as_deref(), Some(action));
        let (first, second) = js.split_once(", y=").unwrap();
        assert_eq!(
            provider_action(&format!("{second},{first}")).as_deref(),
            Some(action)
        );
        assert!(provider_action(&js.replace("getProviderList", "differentAction")).is_none());
        assert!(provider_action(&js.replace(action, "not-an-action")).is_none());
        assert_eq!(
            provider_action(&js.replace("createServerReference)(", "createServerReference("))
                .as_deref(),
            Some(action)
        );
    }
    #[test]
    fn only_bounded_first_party_published_chunks_are_fetched() {
        let page = r#"<script src="/_next/static/chunks/player-a.js?dpl=one"></script><script src="https://evil.test/_next/static/chunks/a.js"></script><script src="https://user@cinesrc.st/_next/static/chunks/a.js"></script><script src="https://cinesrc.st:8443/_next/static/chunks/a.js"></script><script src="/not-a-player.js"></script>"#;
        assert_eq!(
            provider_assets(page).unwrap(),
            vec!["https://cinesrc.st/_next/static/chunks/player-a.js?dpl=one"]
        );
        let too_many: String = (0..33)
            .map(|i| format!("<script src='/_next/static/chunks/{i}.js'></script>"))
            .collect();
        assert!(provider_assets(&too_many).is_err());
        assert!(provider_assets("<script src='/tracker.js'></script>").is_err());
    }
}
