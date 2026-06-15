use crate::{html, manga, manga_image, sdk::http::HttpClient, url};
use manatan_extension::{MangaPage, PageContent, abi::ExtensionResult};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const URLSAFE_BASE64_LOOKUP: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Debug)]
pub struct SpeedBinbReader {
    pub base_url: &'static str,
    pub high_quality: bool,
}

impl SpeedBinbReader {
    pub fn pages(&self, reader_url: &str, body: &str) -> ExtensionResult<Vec<MangaPage>> {
        if !body.contains("data-ptbinb") {
            return Ok(self.ptimg_pages(reader_url, body));
        }
        Ok(self.ptbinb_pages(reader_url, body))
    }

    fn client(&self, referer: &str) -> HttpClient {
        HttpClient::browser()
            .with_desktop_user_agent()
            .with_referer(referer)
            .with_cookies_for(self.base_url)
            .with_webview_challenge_fallback()
    }

    fn get_text(&self, target: &str, referer: &str) -> Option<String> {
        self.client(referer)
            .get(target)
            .browser_document()
            .referer(referer)
            .send_text()
            .ok()
    }

    fn ptimg_pages(&self, reader_url: &str, body: &str) -> Vec<MangaPage> {
        body.split('<')
            .filter_map(|chunk| {
                if !chunk.contains("data-ptimg") {
                    return None;
                }
                let raw = html::attr(chunk, "data-ptimg")?;
                let meta_url = url::join_url(reader_url, &raw);
                let meta = self.get_text(&meta_url, reader_url)?;
                let root = serde_json::from_str::<Value>(&meta).ok()?;
                let image_url = root.pointer("/resources/i/src").and_then(Value::as_str)?;
                let coords = root
                    .pointer("/views/0/coords")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Some(page(
                    &url::join_url(&meta_url, image_url),
                    reader_url,
                    json!({ "coords": coords }),
                ))
            })
            .collect()
    }

    fn ptbinb_pages(&self, reader_url: &str, body: &str) -> Vec<MangaPage> {
        let content = content_block(body);
        let Some(ptbinb) = html::attr(content, "data-ptbinb") else {
            return Vec::new();
        };
        let Some(cid) =
            html::attr(content, "data-ptbinb-cid").or_else(|| query_param(reader_url, "cid"))
        else {
            return Vec::new();
        };
        let shared_key = generate_shared_key(&cid);
        let info_url = add_query_params(
            &url::join_url(reader_url, &ptbinb),
            &[
                ("cid", cid.as_str()),
                ("k", shared_key.as_str()),
                ("dmytime", "1"),
            ],
            reader_url,
        );
        let Some(info_body) = self.get_text(&info_url, reader_url) else {
            return Vec::new();
        };
        let Ok(info) = serde_json::from_str::<Value>(&info_body) else {
            return Vec::new();
        };
        if info.get("result").and_then(Value::as_i64) != Some(1) {
            return Vec::new();
        }
        let Some(item) = info
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
        else {
            return Vec::new();
        };
        let ctbl = decode_table_json(&cid, &shared_key, item.get("ctbl").and_then(Value::as_str));
        let ptbl = decode_table_json(&cid, &shared_key, item.get("ptbl").and_then(Value::as_str));
        let (Some(ctbl), Some(ptbl)) = (ctbl, ptbl) else {
            return Vec::new();
        };
        let sbc_url = sbc_url(item, reader_url, &cid);
        let Some(mut sbc_body) = self.get_text(&sbc_url, reader_url) else {
            return Vec::new();
        };
        if server_type(item) == 1 {
            sbc_body = sbc_body
                .split_once("DataGet_Content(")
                .and_then(|(_, rest)| rest.rsplit_once(')').map(|(inner, _)| inner.to_string()))
                .unwrap_or(sbc_body);
        }
        let Ok(sbc) = serde_json::from_str::<Value>(&sbc_body) else {
            return Vec::new();
        };
        if sbc.get("result").and_then(Value::as_i64) != Some(1) {
            return Vec::new();
        }
        let is_single_quality =
            sbc.get("ImageClass").and_then(Value::as_str) == Some("singlequality");
        let ttx = sbc.get("ttx").and_then(Value::as_str).unwrap_or_default();
        let base = page_base_url(item, &sbc_url);
        ttx.split("<t-img")
            .skip(1)
            .filter_map(|chunk| {
                let src = html::attr(chunk, "src")?;
                let (s, u) = determine_key_pair(&src, &ptbl, &ctbl);
                Some(page(
                    &image_url(
                        &base,
                        &src,
                        item,
                        reader_url,
                        is_single_quality,
                        self.high_quality,
                    ),
                    reader_url,
                    json!({ "s": s, "u": u }),
                ))
            })
            .collect()
    }
}

fn page(image_url: &str, reader_url: &str, speedbinb: Value) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image_url.to_string(),
            context: Some(manga::image_headers(reader_url)),
        },
        headers: manga::image_headers(reader_url),
        extra: BTreeMap::from([("speedbinb".into(), speedbinb)]),
        ..MangaPage::default()
    }
}

fn content_block(body: &str) -> &str {
    body.split("id=\"content\"")
        .nth(1)
        .or_else(|| body.split("id='content'").nth(1))
        .unwrap_or(body)
}

fn generate_shared_key(cid: &str) -> String {
    let random_chars = "ABCDEFGHIJKLMNOP";
    let repeat_count = (16 + cid.len() - 1) / cid.len().max(1);
    let repeated = cid.repeat(repeat_count.max(1));
    let head = &repeated[..16.min(repeated.len())];
    let tail = &repeated[repeated.len().saturating_sub(16)..];
    let mut s = 0usize;
    let mut h = 0usize;
    let mut u = 0usize;
    let mut out = String::new();
    for (index, ch) in random_chars.chars().enumerate() {
        s ^= ch as usize;
        h ^= head.as_bytes().get(index).copied().unwrap_or_default() as usize;
        u ^= tail.as_bytes().get(index).copied().unwrap_or_default() as usize;
        out.push(ch);
        out.push(URLSAFE_BASE64_LOOKUP.as_bytes()[(s + h + u) & 63] as char);
    }
    out
}

fn decode_scramble_table(cid: &str, shared_key: &str, table: &str) -> String {
    let seed = format!("{cid}:{shared_key}");
    let mut e = 0i64;
    for (index, ch) in seed.chars().enumerate() {
        e += (ch as i64) << (index % 16);
    }
    e &= 2_147_483_647;
    if e == 0 {
        e = 0x1234_5678;
    }
    let mut out = String::new();
    for ch in table.chars() {
        e = ((e as u64) >> 1) as i64 ^ (1_210_056_708i64 & -((1 & e) as i64));
        out.push(((ch as i64 - 32 + e) % 94 + 32) as u8 as char);
    }
    out
}

fn decode_table_json(cid: &str, shared_key: &str, table: Option<&str>) -> Option<Vec<String>> {
    let decoded = decode_scramble_table(cid, shared_key, table?);
    serde_json::from_str(&decoded).ok()
}

fn determine_key_pair(src: &str, ptbl: &[String], ctbl: &[String]) -> (String, String) {
    let filename = src.rsplit('/').next().unwrap_or(src);
    let mut index = [0usize, 0usize];
    for (position, ch) in filename.chars().enumerate() {
        index[position % 2] += ch as usize;
    }
    index[0] %= 8;
    index[1] %= 8;
    (
        ptbl.get(index[0]).cloned().unwrap_or_default(),
        ctbl.get(index[1]).cloned().unwrap_or_default(),
    )
}

fn sbc_url(item: &Value, reader_url: &str, cid: &str) -> String {
    let content_server = item
        .get("ContentsServer")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match server_type(item) {
        1 => url::join_url(content_server, "content.js"),
        2 => url::join_url(content_server, "content"),
        0 => {
            let mut params = vec![
                ("cid".to_string(), cid.to_string()),
                ("q".to_string(), "1".to_string()),
                ("vm".to_string(), view_mode(item).to_string()),
                (
                    "dmytime".to_string(),
                    item.get("ContentDate")
                        .and_then(Value::as_str)
                        .unwrap_or("1")
                        .to_string(),
                ),
            ];
            if let Some(token) = item.get("p").and_then(Value::as_str) {
                params.push(("p".to_string(), token.to_string()));
            }
            for index in 0..=9 {
                if let Some(value) = query_param(reader_url, &format!("u{index}")) {
                    params.push((format!("u{index}"), value));
                }
            }
            format!(
                "{}?{}",
                url::join_url(content_server, "sbcGetCntnt.php"),
                encode_params(&params)
            )
        }
        _ => content_server.to_string(),
    }
}

fn page_base_url(item: &Value, sbc_url: &str) -> String {
    match server_type(item) {
        0 => sbc_url.replace("/sbcGetCntnt.php", "/sbcGetImg.php"),
        _ => item
            .get("ContentsServer")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn image_url(
    base: &str,
    src: &str,
    item: &Value,
    reader_url: &str,
    is_single_quality: bool,
    high_quality: bool,
) -> String {
    let content_date = item.get("ContentDate").and_then(Value::as_str);
    match server_type(item) {
        1 => {
            let filename = if is_single_quality {
                "M.jpg"
            } else if high_quality {
                "M_H.jpg"
            } else {
                "M_L.jpg"
            };
            let mut target = format!(
                "{}/{}/{}",
                base.trim_end_matches('/'),
                src.trim_matches('/'),
                filename
            );
            if let Some(date) = content_date {
                target.push_str(&format!("?dmytime={}", url::query_escape(date)));
            }
            target
        }
        2 => {
            let mut params = Vec::new();
            if !is_single_quality && !high_quality {
                params.push(("q".to_string(), "1".to_string()));
            }
            if let Some(date) = content_date {
                params.push(("dmytime".to_string(), date.to_string()));
            }
            for index in 0..=9 {
                if let Some(value) = query_param(reader_url, &format!("u{index}")) {
                    params.push((format!("u{index}"), value));
                }
            }
            let target = format!(
                "{}/img/{}",
                base.trim_end_matches('/'),
                src.trim_matches('/')
            );
            append_params(&target, &params)
        }
        0 => {
            let mut params = vec![
                ("src".to_string(), src.to_string()),
                ("vm".to_string(), view_mode(item).to_string()),
            ];
            if let Some(token) = item.get("p").and_then(Value::as_str) {
                params.push(("p".to_string(), token.to_string()));
            }
            if !is_single_quality {
                let trial = matches!(view_mode(item), 2 | 3);
                params.push((
                    "q".to_string(),
                    if high_quality && !trial { "0" } else { "1" }.to_string(),
                ));
            }
            if let Some(date) = content_date {
                params.push(("dmytime".to_string(), date.to_string()));
            }
            append_params(base, &params)
        }
        _ => base.to_string(),
    }
}

fn server_type(item: &Value) -> i64 {
    item.get("ServerType").and_then(Value::as_i64).unwrap_or(0)
}

fn view_mode(item: &Value) -> i64 {
    item.get("ViewMode").and_then(Value::as_i64).unwrap_or(0)
}

fn add_query_params(target: &str, required: &[(&str, &str)], reader_url: &str) -> String {
    let mut params = required
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    for index in 0..=9 {
        if let Some(value) = query_param(reader_url, &format!("u{index}")) {
            params.push((format!("u{index}"), value));
        }
    }
    append_params(target, &params)
}

fn append_params(target: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return target.to_string();
    }
    let separator = if target.contains('?') { '&' } else { '?' };
    format!("{target}{separator}{}", encode_params(params))
}

fn encode_params(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn query_param(target: &str, key: &str) -> Option<String> {
    let query = target
        .split_once('?')?
        .1
        .split('#')
        .next()
        .unwrap_or_default();
    for part in query.split('&') {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

#[allow(dead_code)]
fn _uses_image_helper() {
    let _ = manga_image::SpeedBinb;
}
