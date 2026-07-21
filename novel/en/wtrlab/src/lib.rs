// Ported from the observable behavior of LNReader's MIT-licensed WTR-LAB source.

use aes_gcm::{
    aead::{consts::U16, AeadInPlace},
    aes::Aes256,
    Aes256Gcm, AesGcm, KeyInit, Nonce, Tag,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::DateTime;
use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequest, ImageRequestContext, NovelChapter,
        NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use regex::Regex;
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "wtr-lab";
const BASE_URL: &str = "https://wtr-lab.com";
const TRANSLATE_URL: &str = "https://translate-pa.googleapis.com/v1/translateHtml";
const GOOGLE_TRANSLATE_KEY: &str = "AIzaSyATBXajvzQLTDHEQbcpq0Ihe0vWDHmO520";

pub struct WtrLabSource {
    client: Client,
}

impl Default for WtrLabSource {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE_URL),
        }
    }
}

impl WtrLabSource {
    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok((
            html::document(response.text()?),
            response.final_url().to_owned(),
        ))
    }

    fn next_data(document: &Html) -> Result<Value> {
        let query = selector("#__NEXT_DATA__")?;
        let raw = document
            .select(&query)
            .next()
            .map(|node| node.inner_html())
            .ok_or_else(|| Error::new("WTR-LAB page has no __NEXT_DATA__ payload"))?;
        serde_json::from_str(&raw)
            .map_err(|error| Error::new(format!("invalid WTR-LAB page data: {error}")))
    }

    fn build_id(&self) -> Result<String> {
        let (document, _) = self.document(&format!("{BASE_URL}/en/novel-finder"))?;
        Self::next_data(&document)?
            .get("buildId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::new("WTR-LAB page data has no build id"))
    }

    fn finder_url(&self, page: u32, filters: &Value) -> Result<String> {
        let build_id = self.build_id()?;
        let mut url = Url::parse(&format!(
            "{BASE_URL}/_next/data/{build_id}/en/novel-finder.json"
        ))
        .map_err(|error| Error::new(error.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("orderBy", string_filter(filters, "orderBy", "update"));
            query.append_pair("order", string_filter(filters, "order", "desc"));
            query.append_pair("status", string_filter(filters, "status", "all"));
            query.append_pair(
                "release_status",
                string_filter(filters, "release_status", "all"),
            );
            query.append_pair(
                "addition_age",
                string_filter(filters, "addition_age", "all"),
            );
            query.append_pair("page", &page.max(1).to_string());
            for (key, target) in [
                ("text", "text"),
                ("min_chapters", "minc"),
                ("min_rating", "minr"),
                ("min_review_count", "minrc"),
            ] {
                if let Some(value) = filters
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    query.append_pair(target, value);
                }
            }
            for (key, target) in [("genres", "gi"), ("tags", "ti")] {
                if let Some(values) = filters.get(key).and_then(Value::as_array) {
                    let value = values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",");
                    if !value.is_empty() {
                        query.append_pair(target, &value);
                    }
                }
            }
        }
        Ok(url.to_string())
    }

    fn parse_series(value: &Value) -> Result<CatalogItem> {
        let raw_id = value
            .get("raw_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::new("WTR-LAB series has no raw id"))?;
        let slug = value
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("WTR-LAB series has no slug"))?;
        let data = value.get("data").unwrap_or(value);
        let title = data
            .get("title")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(slug);
        let url = format!("{BASE_URL}/en/serie-{raw_id}/{slug}");
        let mut item = CatalogItem::new(url.clone(), title);
        item.url = Some(url.clone());
        item.cover = data
            .get("image")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| image(value, &url));
        item.description = data
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        item.authors = data
            .get("author")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .into_iter()
            .collect();
        item.language = Some("en".into());
        item.content_rating = Some("suggestive".into());
        item.extra.insert("rawId".into(), json!(raw_id));
        item.extra.insert("slug".into(), json!(slug));
        Ok(item)
    }

    fn finder(&self, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let response: Value = self
            .client
            .get(self.finder_url(page, filters)?)
            .send()?
            .error_for_status()?
            .json()?;
        let page_props = response
            .get("pageProps")
            .or_else(|| response.pointer("/props/pageProps"))
            .ok_or_else(|| Error::new("WTR-LAB finder response has no pageProps"))?;
        let values = page_props
            .get("series")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("WTR-LAB finder response has no series"))?;
        let mut seen = std::collections::BTreeSet::new();
        let entries = values
            .iter()
            .filter(|value| {
                value
                    .get("raw_id")
                    .and_then(Value::as_u64)
                    .map(|id| seen.insert(id))
                    .unwrap_or(false)
            })
            .map(Self::parse_series)
            .collect::<Result<Vec<_>>>()?;
        let count = page_props
            .get("count")
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
            .unwrap_or(entries.len() as u64);
        Ok(Paged::new(entries, page.max(1) as u64 * 10 < count))
    }

    fn latest_page(&self, page: u32) -> Result<Paged<CatalogItem>> {
        let response: Value = self
            .client
            .post(format!("{BASE_URL}/api/home/recent"))
            .header("Content-Type", "application/json")
            .body(json!({"page": page.max(1)}).to_string())
            .send()?
            .error_for_status()?
            .json()?;
        let values = response
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("WTR-LAB recent response has no data"))?;
        let entries = values
            .iter()
            .filter_map(|value| value.get("serie"))
            .map(Self::parse_series)
            .collect::<Result<Vec<_>>>()?;
        let has_next = !entries.is_empty();
        Ok(Paged::new(entries, has_next))
    }

    fn series_url(item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        let mut url = Url::parse(&absolute_url(BASE_URL, candidate)?)
            .map_err(|error| Error::new(error.to_string()))?;
        url.set_query(None);
        url.set_fragment(None);
        Ok(url.to_string())
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let next = Self::next_data(document)?;
        let serie = next
            .pointer("/props/pageProps/serie/serie_data")
            .ok_or_else(|| Error::new("WTR-LAB series data is missing"))?;
        let mut item = Self::parse_series(serie)?;
        item.key = page_url.into();
        item.url = Some(page_url.into());
        item.initialized = true;
        item.status = Some(json!(match serie.get("status").and_then(Value::as_i64) {
            Some(0) => "ongoing",
            Some(1) => "completed",
            _ => "unknown",
        }));
        item.extra.insert(
            "chapterCount".into(),
            serie.get("chapter_count").cloned().unwrap_or(json!(0)),
        );
        let tags = tags_from_document(document)?;
        if !tags.is_empty() {
            item.tags = tags;
        }
        Ok(item)
    }

    fn identity(item: &CatalogItem) -> Result<(u64, String, u32)> {
        if let (Some(raw_id), Some(slug)) = (
            item.extra.get("rawId").and_then(Value::as_u64),
            item.extra.get("slug").and_then(Value::as_str),
        ) {
            let count = item
                .extra
                .get("chapterCount")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            return Ok((raw_id, slug.to_owned(), count));
        }
        let url =
            Url::parse(&Self::series_url(item)?).map_err(|error| Error::new(error.to_string()))?;
        let re = Regex::new(r"/(?:serie-|novel/)(\d+)/([^/?#]+)")
            .map_err(|error| Error::new(error.to_string()))?;
        let captures = re
            .captures(url.path())
            .ok_or_else(|| Error::new("invalid WTR-LAB series URL"))?;
        let raw_id = captures[1]
            .parse()
            .map_err(|_| Error::new("invalid WTR-LAB raw id"))?;
        let count = item
            .extra
            .get("chapterCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        Ok((raw_id, captures[2].to_owned(), count))
    }

    fn chapter_count(&self, item: &CatalogItem) -> Result<(CatalogItem, u32)> {
        if let Some(count) = item
            .extra
            .get("chapterCount")
            .and_then(Value::as_u64)
            .filter(|count| *count > 0)
        {
            return Ok((item.clone(), count as u32));
        }
        let url = Self::series_url(item)?;
        let (document, final_url) = self.document(&url)?;
        let details = Self::parse_details(&document, &final_url)?;
        let count = details
            .extra
            .get("chapterCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        require((count > 0).then_some(()), "WTR-LAB series has no chapters")?;
        Ok((details, count))
    }

    fn fetch_chapters(&self, item: &CatalogItem) -> Result<Vec<NovelChapter>> {
        let (item, total) = self.chapter_count(item)?;
        let (raw_id, slug, _) = Self::identity(&item)?;
        let mut chapters = Vec::new();
        for start in (1..=total).step_by(250) {
            let end = (start + 249).min(total);
            let response: Value = self
                .client
                .get(format!(
                    "{BASE_URL}/api/chapters/{raw_id}?start={start}&end={end}"
                ))
                .send()?
                .error_for_status()?
                .json()?;
            let values = response
                .get("chapters")
                .or_else(|| response.pointer("/data/chapters"))
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("WTR-LAB chapter response has no chapters"))?;
            for value in values {
                let Some(order) = value.get("order").and_then(Value::as_f64) else {
                    continue;
                };
                let url = format!(
                    "{BASE_URL}/en/serie-{raw_id}/{slug}/chapter-{}",
                    order as u32
                );
                chapters.push(NovelChapter {
                    key: url.clone(),
                    title: value
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    chapter_number: Some(order as f32),
                    date_uploaded: value
                        .get("updated_at")
                        .and_then(Value::as_str)
                        .and_then(parse_date),
                    url: Some(url),
                    language: Some("en".into()),
                    source_order: Some(chapters.len() as i32),
                    ..NovelChapter::default()
                });
            }
            if values.len() < 250 {
                break;
            }
        }
        chapters.sort_by(|a, b| {
            a.chapter_number
                .partial_cmp(&b.chapter_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for (index, chapter) in chapters.iter_mut().enumerate() {
            chapter.source_order = Some(index as i32);
        }
        Ok(chapters)
    }

    fn get_reader_payload(
        &self,
        url: &str,
        raw_id: u64,
        chapter_no: u32,
    ) -> Result<(Value, String)> {
        let mut last_error = String::new();
        for mode in ["ai", "web"] {
            let response = self.client.post(format!("{BASE_URL}/api/reader/get"))
                .header("Content-Type", "application/json").header("Accept", "application/json").header("Referer", url)
                .body(json!({"translate":mode,"language":"en","raw_id":raw_id,"chapter_no":chapter_no,"retry":false,"force_retry":false}).to_string())
                .send()?;
            let value: Value = response.json()?;
            if response.status() >= 200
                && response.status() < 300
                && value.get("error").is_none()
                && value.get("success").and_then(Value::as_bool) != Some(false)
            {
                return Ok((value, last_error));
            }
            last_error = value
                .get("error")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("reader request failed")
                .to_owned();
        }
        Err(Error::new(last_error))
    }

    fn encryption_key(&self, document: &Html) -> Result<String> {
        let scripts = selector("head script[src]")?;
        let marker = "TextEncoder().encode(\"";
        for src in document
            .select(&scripts)
            .filter_map(|element| attr(element, "src"))
        {
            let response = self
                .client
                .get(absolute_url(BASE_URL, &src)?)
                .max_body_bytes(8 * 1024 * 1024)
                .send()?
                .error_for_status()?;
            let script = response.text()?;
            if let Some(start) = script.find(marker) {
                let value = &script[start + marker.len()..];
                if value.len() >= 32 {
                    return Ok(value[..32].to_owned());
                }
            }
        }
        Err(Error::new(
            "WTR-LAB encryption key was not found in current application scripts",
        ))
    }

    fn translate(&self, lines: &[String]) -> Result<Vec<String>> {
        if lines.is_empty() {
            return Ok(Vec::new());
        }
        let contained = lines
            .iter()
            .enumerate()
            .map(|(index, line)| format!("<a i={index}>{line}</a>"))
            .collect::<Vec<_>>();
        let body = json!([[contained, "zh-CN", "en"], "te_lib"]).to_string();
        let response: Value = self
            .client
            .post(TRANSLATE_URL)
            .header("Content-Type", "application/json+protobuf")
            .header("X-Goog-API-Key", GOOGLE_TRANSLATE_KEY)
            .header("Referer", format!("{BASE_URL}/"))
            .body(body)
            .send()?
            .error_for_status()?
            .json()?;
        let translated = response
            .get(0)
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("Google Translate returned no translated lines"))?;
        Ok(translated
            .iter()
            .filter_map(Value::as_str)
            .map(strip_translation_anchor)
            .collect())
    }

    fn render_text(
        &self,
        chapter_url: &str,
        payload: Value,
        login_error: &str,
        document: Option<&Html>,
    ) -> Result<NovelText> {
        let data = payload
            .pointer("/data/data")
            .ok_or_else(|| Error::new("WTR-LAB reader response has no content"))?;
        let body = data
            .get("body")
            .ok_or_else(|| Error::new("WTR-LAB reader response has no body"))?;
        let mut translated_locally = false;
        let mut lines = if let Some(encrypted) = body
            .as_str()
            .filter(|value| value.starts_with("arr:") || value.starts_with("str:"))
        {
            let document = document
                .ok_or_else(|| Error::new("WTR-LAB encrypted content requires the chapter page"))?;
            let decrypted = decrypt(encrypted, &self.encryption_key(document)?)?;
            let original = match decrypted {
                Value::Array(values) => values
                    .into_iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect(),
                Value::String(value) => vec![value],
                _ => {
                    return Err(Error::new(
                        "WTR-LAB decrypted content has an unsupported shape",
                    ))
                }
            };
            translated_locally = true;
            self.translate(&original)?
        } else if let Some(values) = body.as_array() {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        } else if let Some(value) = body.as_str() {
            vec![value.to_owned()]
        } else {
            Vec::new()
        };
        let glossary = data
            .pointer("/glossary_data/terms")
            .and_then(Value::as_array)
            .map(|terms| {
                terms
                    .iter()
                    .filter_map(|term| term.get(0).and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let marker = Regex::new(r"(?i)(?:wtr-lab\s+)?※([0-9]+)[⛬〓]")
            .map_err(|error| Error::new(error.to_string()))?;
        for line in &mut lines {
            *line = marker
                .replace_all(line, |captures: &regex::Captures<'_>| {
                    captures[1]
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| glossary.get(index))
                        .cloned()
                        .unwrap_or_else(|| captures[0].to_owned())
                })
                .into_owned();
        }
        require(
            (!lines.is_empty()).then_some(()),
            "WTR-LAB chapter has no readable content",
        )?;
        let mut rendered = String::new();
        if translated_locally {
            rendered.push_str("<p><small>Translated on demand using the source's public web translation method.</small></p>");
        }
        if !login_error.is_empty() {
            rendered.push_str(&format!(
                "<p><small>{}</small></p>",
                escape_html(login_error)
            ));
        }
        for line in lines {
            rendered.push_str("<p>");
            rendered.push_str(&sanitize_line(&line));
            rendered.push_str("</p>");
        }
        Ok(NovelText {
            html: Some(rendered.clone()),
            base_url: Some(chapter_url.into()),
            image_context: Some(ImageRequestContext {
                headers: [("Referer".into(), chapter_url.into())]
                    .into_iter()
                    .collect(),
                cookie_url: Some(BASE_URL.into()),
            }),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }
}

impl NovelSource for WtrLabSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.finder(page, &json!({}))
    }
    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.latest_page(page)
    }
    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.finder(page, filters),
            "latest" => self.latest_page(page),
            _ => Err(Error::new(format!("unknown novel listing {listing:?}"))),
        }
    }
    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let mut filters = filters.clone();
        filters
            .as_object_mut()
            .ok_or_else(|| Error::new("WTR-LAB filters must be an object"))?
            .insert("text".into(), json!(query));
        self.finder(page, &filters)
    }
    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = Self::series_url(&item)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_details(&document, &final_url)
    }
    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        self.fetch_chapters(&item)
    }
    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = absolute_url(BASE_URL, chapter.url.as_deref().unwrap_or(&chapter.key))?;
        let re = Regex::new(r"/(?:serie-|novel/)(\d+)/[^/]+/chapter-(\d+)")
            .map_err(|error| Error::new(error.to_string()))?;
        let captures = re
            .captures(&url)
            .ok_or_else(|| Error::new("invalid WTR-LAB chapter URL"))?;
        let raw_id = captures[1]
            .parse()
            .map_err(|_| Error::new("invalid WTR-LAB raw id"))?;
        let chapter_no = captures[2]
            .parse()
            .map_err(|_| Error::new("invalid WTR-LAB chapter number"))?;
        let (payload, login_error) = self.get_reader_payload(&url, raw_id, chapter_no)?;
        let encrypted = payload
            .pointer("/data/data/body")
            .and_then(Value::as_str)
            .map(|value| value.starts_with("arr:") || value.starts_with("str:"))
            .unwrap_or(false);
        let document = if encrypted {
            Some(self.document(&url)?.0)
        } else {
            None
        };
        self.render_text(&url, payload, &login_error, document.as_ref())
    }
    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            FilterDefinition::Text {
                id: "text".into(),
                name: "Search".into(),
                default: String::new(),
            },
            select_filter("orderBy", "Order by", ORDER_BY),
            select_filter("order", "Order", ORDER),
            select_filter("status", "Translation status", STATUS),
            select_filter("release_status", "Original status", STATUS),
            select_filter("addition_age", "Added", ADDITION_AGE),
            FilterDefinition::Text {
                id: "min_chapters".into(),
                name: "Minimum chapters".into(),
                default: String::new(),
            },
            FilterDefinition::Text {
                id: "min_rating".into(),
                name: "Minimum rating".into(),
                default: String::new(),
            },
            FilterDefinition::Text {
                id: "min_review_count".into(),
                name: "Minimum reviews".into(),
                default: String::new(),
            },
        ])
    }
    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("wtr-lab.com") {
            return Ok(None);
        }
        let re = Regex::new(r"^/en/(?:serie-|novel/)(\d+)/([^/]+)(?:/chapter-(\d+))?").unwrap();
        let Some(captures) = re.captures(url.path()) else {
            return Ok(None);
        };
        let series = format!("{BASE_URL}/en/serie-{}/{}", &captures[1], &captures[2]);
        let mut item = CatalogItem::new(series.clone(), "");
        item.url = Some(series);
        item.language = Some("en".into());
        let novel_chapter = captures.get(3).map(|_| NovelChapter {
            key: candidate.into(),
            url: Some(candidate.into()),
            chapter_number: captures
                .get(3)
                .and_then(|value| value.as_str().parse().ok()),
            language: Some("en".into()),
            ..NovelChapter::default()
        });
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter,
            ..UrlResolveResult::default()
        }))
    }
}

fn decrypt(value: &str, key: &str) -> Result<Value> {
    let (is_array, payload) = if let Some(value) = value.strip_prefix("arr:") {
        (true, value)
    } else if let Some(value) = value.strip_prefix("str:") {
        (false, value)
    } else {
        return Err(Error::new("WTR-LAB encrypted content has no type prefix"));
    };
    let parts = payload.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(Error::new("WTR-LAB encrypted content has an invalid shape"));
    }
    let iv = BASE64
        .decode(parts[0])
        .map_err(|error| Error::new(error.to_string()))?;
    let tag = BASE64
        .decode(parts[1])
        .map_err(|error| Error::new(error.to_string()))?;
    let mut ciphertext = BASE64
        .decode(parts[2])
        .map_err(|error| Error::new(error.to_string()))?;
    if !matches!(iv.len(), 12 | 16) || tag.len() != 16 || key.len() < 32 {
        return Err(Error::new("WTR-LAB encryption parameters are invalid"));
    }
    if iv.len() == 12 {
        let cipher = Aes256Gcm::new_from_slice(&key.as_bytes()[..32])
            .map_err(|error| Error::new(error.to_string()))?;
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&iv),
                b"",
                &mut ciphertext,
                Tag::from_slice(&tag),
            )
            .map_err(|_| Error::new("WTR-LAB content decryption failed"))?;
    } else {
        let cipher = AesGcm::<Aes256, U16>::new_from_slice(&key.as_bytes()[..32])
            .map_err(|error| Error::new(error.to_string()))?;
        cipher
            .decrypt_in_place_detached(
                Nonce::<U16>::from_slice(&iv),
                b"",
                &mut ciphertext,
                Tag::from_slice(&tag),
            )
            .map_err(|_| Error::new("WTR-LAB content decryption failed"))?;
    }
    let text = String::from_utf8(ciphertext).map_err(|error| Error::new(error.to_string()))?;
    if is_array {
        serde_json::from_str(&text).map_err(|error| Error::new(error.to_string()))
    } else {
        Ok(Value::String(text))
    }
}

fn tags_from_document(document: &Html) -> Result<Vec<String>> {
    let query = selector(".genre, .genres .genre, .tag, .tags .tag")?;
    let mut tags = Vec::new();
    for node in document.select(&query) {
        let value = normalize_space(&html::text(node))
            .trim_end_matches(',')
            .to_owned();
        if !value.is_empty() && !tags.contains(&value) {
            tags.push(value);
        }
    }
    Ok(tags)
}
fn image(url: &str, referer: &str) -> ImageRequest {
    ImageRequest::get(url).header("Referer", referer)
}
fn string_filter<'a>(filters: &'a Value, key: &str, default: &'a str) -> &'a str {
    filters.get(key).and_then(Value::as_str).unwrap_or(default)
}
fn parse_date(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp_millis())
}
fn strip_translation_anchor(value: &str) -> String {
    let re = Regex::new(r"(?is)^\s*<a\b[^>]*>(.*)</a>\s*$").unwrap();
    re.captures(value)
        .map(|capture| capture[1].to_owned())
        .unwrap_or_else(|| value.to_owned())
}
fn sanitize_line(value: &str) -> String {
    let scripts = Regex::new(
        r"(?is)<(?:script|iframe|object|embed)\b[^>]*>.*?</(?:script|iframe|object|embed)\s*>",
    )
    .unwrap();
    scripts.replace_all(value, "").into_owned()
}
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn select_filter(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: values
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).into(),
                value: (*value).into(),
            })
            .collect(),
        default_index: 0,
    }
}

const ORDER_BY: &[(&str, &str)] = &[
    ("Update Date", "update"),
    ("Addition Date", "date"),
    ("Random", "random"),
    ("Weekly View", "weekly_rank"),
    ("Monthly View", "monthly_rank"),
    ("All-Time View", "view"),
    ("Name", "name"),
    ("Reader", "reader"),
    ("Chapter", "chapter"),
    ("Rating", "rating"),
    ("Review Count", "total_rate"),
    ("Vote Count", "vote"),
];
const ORDER: &[(&str, &str)] = &[("Descending", "desc"), ("Ascending", "asc")];
const STATUS: &[(&str, &str)] = &[
    ("All", "all"),
    ("Ongoing", "ongoing"),
    ("Completed", "completed"),
];
const ADDITION_AGE: &[(&str, &str)] = &[
    ("All time", "all"),
    ("Today", "day"),
    ("This week", "week"),
    ("This month", "month"),
    ("This year", "year"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, WtrLabSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Aead;

    #[test]
    fn parses_series_fixture() {
        let item = WtrLabSource::parse_series(&json!({"raw_id":7,"slug":"fixture","data":{"title":"Fixture Novel","image":"https://img.wtr-lab.com/a.jpg"}})).unwrap();
        assert_eq!(item.title, "Fixture Novel");
        assert!(item.key.ends_with("/serie-7/fixture"));
    }

    #[test]
    fn decrypts_array_payload() {
        let key = "01234567890123456789012345678901";
        let iv = [7u8; 12];
        let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).unwrap();
        let combined = cipher
            .encrypt(Nonce::from_slice(&iv), br#"["one","two"]"#.as_slice())
            .unwrap();
        let split = combined.len() - 16;
        let value = format!(
            "arr:{}:{}:{}",
            BASE64.encode(iv),
            BASE64.encode(&combined[split..]),
            BASE64.encode(&combined[..split])
        );
        assert_eq!(decrypt(&value, key).unwrap(), json!(["one", "two"]));
    }

    #[test]
    fn decrypts_current_sixteen_byte_nonce_payload() {
        let key = "01234567890123456789012345678901";
        let iv = [9u8; 16];
        let cipher = AesGcm::<Aes256, U16>::new_from_slice(key.as_bytes()).unwrap();
        let combined = cipher
            .encrypt(Nonce::<U16>::from_slice(&iv), br#"["current"]"#.as_slice())
            .unwrap();
        let split = combined.len() - 16;
        let value = format!(
            "arr:{}:{}:{}",
            BASE64.encode(iv),
            BASE64.encode(&combined[split..]),
            BASE64.encode(&combined[..split])
        );
        assert_eq!(decrypt(&value, key).unwrap(), json!(["current"]));
    }
}
