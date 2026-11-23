use manatan_common::{absolute_url, selector};
use manatan_sdk::{
    client::{BrowserChallengePolicy, Client},
    html::{self, Html},
    model::{CatalogItem, NovelChapter, NovelText, Paged, UrlResolveResult},
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use url::Url;
use wordpress_novel::{chapter_text, image_with_cookies as image, rating};

const BASE: &str = "https://starcafe.me";
const CATALOG_URL: &str = "https://firestore.googleapis.com/v1/projects/starcafe-167bb/databases/(default)/documents/indexes/cnovels";

// Firestore REST wraps each field in a typed value. Decode the types in the
// public catalog, retaining numbers and timestamps for sorting.
fn firestore_value(field: &Value) -> Result<Value> {
    for kind in [
        "stringValue",
        "timestampValue",
        "booleanValue",
        "doubleValue",
        "nullValue",
    ] {
        if let Some(value) = field.get(kind) {
            return Ok(value.clone());
        }
    }
    if let Some(value) = field.get("integerValue") {
        return value
            .as_str()
            .and_then(|value| value.parse::<i64>().ok())
            .map(Value::from)
            .ok_or_else(|| Error::new("Invalid StarCafe catalog integer"));
    }
    if let Some(array) = field.get("arrayValue") {
        return array
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(firestore_value)
            .collect::<Result<Vec<_>>>()
            .map(Value::Array);
    }
    if let Some(map) = field.get("mapValue") {
        return map
            .get("fields")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(key, value)| Ok((key.clone(), firestore_value(value)?)))
            .collect::<Result<serde_json::Map<_, _>>>()
            .map(Value::Object);
    }
    Err(Error::new("Unknown StarCafe catalog field type"))
}

fn catalog_rows(document: &Value) -> Result<Vec<Value>> {
    let rows = document
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::new("StarCafe catalog document has no fields"))?;
    rows.values()
        .map(|row| {
            let fields = row
                .pointer("/mapValue/fields")
                .and_then(Value::as_object)
                .ok_or_else(|| Error::new("Invalid StarCafe catalog entry"))?;
            // Ignore fields added by the site that aren't part of our catalog.
            [
                "slug",
                "title",
                "favorites",
                "meta_updated_at",
                "novel_cover",
                "status",
                "tags",
            ]
            .into_iter()
            .filter_map(|key| fields.get(key).map(|value| (key, value)))
            .map(|(key, value)| Ok((key.to_owned(), firestore_value(value)?)))
            .collect::<Result<serde_json::Map<_, _>>>()
            .map(Value::Object)
        })
        .collect()
}

pub struct StarCafe {
    client: Client,
}
impl Default for StarCafe {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE),
        }
    }
}

/// Decode the site's public React Flight payload without executing page scripts.
/// Text records carry UTF-8 byte lengths, not line lengths; descriptions and
/// chapter text can span lines, chunks, and contain record-looking text.
fn flight(doc: &Html) -> Result<Vec<Value>> {
    let mut payload = String::new();
    for script in doc.select(&selector("script")?) {
        let text = script.text().collect::<String>();
        let mut rest = text.as_str();
        while let Some(pos) = rest.find("self.__next_f.push(") {
            rest = &rest[pos + "self.__next_f.push(".len()..];
            let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
            let value = stream
                .next()
                .transpose()
                .map_err(|_| Error::new("StarCafe page data is malformed"))?;
            if let Some(value) = value {
                if value.get(0).and_then(Value::as_u64) == Some(1) {
                    if let Some(chunk) = value.get(1).and_then(Value::as_str) {
                        payload.push_str(chunk);
                    }
                }
            }
            rest = &rest[stream.byte_offset()..];
        }
    }
    let mut records = BTreeMap::new();
    let mut rest = payload.as_str();
    while !rest.is_empty() {
        rest = rest.trim_start_matches(['\r', '\n']);
        let Some(colon) = rest.find(':') else { break };
        let id = &rest[..colon];
        // React's preload hint records intentionally have an empty record ID.
        if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::new("StarCafe page data has an invalid record"));
        }
        rest = &rest[colon + 1..];
        let value;
        if let Some(text) = rest.strip_prefix('T') {
            let comma = text
                .find(',')
                .ok_or_else(|| Error::new("StarCafe text record has no length"))?;
            let len = usize::from_str_radix(&text[..comma], 16)
                .map_err(|_| Error::new("StarCafe text length is invalid"))?;
            let text = &text[comma + 1..];
            value = Value::String(
                text.get(..len)
                    .ok_or_else(|| Error::new("StarCafe text record is truncated"))?
                    .into(),
            );
            rest = &text[len..];
        } else {
            let end = rest.find('\n').unwrap_or(rest.len());
            value = serde_json::from_str(&rest[..end]).unwrap_or(Value::Null);
            rest = &rest[end..];
        }
        if !id.is_empty() {
            records.insert(id.to_owned(), value);
        }
    }
    fn resolve(value: &Value, rows: &BTreeMap<String, Value>, depth: u8) -> Value {
        if depth >= 16 {
            return value.clone();
        }
        match value {
            Value::String(s) if s.starts_with('$') && rows.contains_key(&s[1..]) => {
                resolve(&rows[&s[1..]], rows, depth + 1)
            }
            Value::String(s) if s.starts_with('{') || s.starts_with('[') => {
                serde_json::from_str::<Value>(s)
                    .map(|v| resolve(&v, rows, depth + 1))
                    .unwrap_or_else(|_| value.clone())
            }
            Value::Array(a) => {
                Value::Array(a.iter().map(|v| resolve(v, rows, depth + 1)).collect())
            }
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), resolve(v, rows, depth + 1)))
                    .collect(),
            ),
            _ => value.clone(),
        }
    }
    if records.is_empty() {
        return Err(Error::new("StarCafe public page data is missing"));
    }
    Ok(records.values().map(|v| resolve(v, &records, 0)).collect())
}
fn find<'a>(value: &'a Value, member: &str) -> Option<&'a Value> {
    if value.get(member).is_some() {
        return Some(value);
    }
    match value {
        Value::Array(a) => a.iter().find_map(|v| find(v, member)),
        Value::Object(o) => o.values().find_map(|v| find(v, member)),
        _ => None,
    }
}
fn data<'a>(rows: &'a [Value], member: &str) -> Result<&'a Value> {
    rows.iter()
        .find_map(|v| find(v, member))
        .ok_or_else(|| Error::new(format!("StarCafe public {member} data is missing")))
}
fn required<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new(format!("StarCafe {key} is missing")))
}
fn item(row: &Value) -> Result<CatalogItem> {
    let slug = required(row, "slug")?;
    let url = format!("{BASE}/novels/{slug}");
    let mut item = CatalogItem::new(&url, required(row, "title")?);
    item.url = Some(url.clone());
    item.language = Some("en".into());
    item.authors = row
        .get("author")
        .and_then(Value::as_str)
        .map(|s| vec![s.into()])
        .unwrap_or_default();
    item.description = row
        .get("description")
        .and_then(Value::as_str)
        .map(|s| html::text(html::fragment(s).root_element()));
    item.tags = row
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    item.content_rating = Some(rating(&item.tags).into());
    item.status = Some(json!(match row
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "complete" | "completed" => "completed",
        "ongoing" => "ongoing",
        "dropped" => "cancelled",
        _ => "unknown",
    }));
    item.cover = row
        .get("novel_cover")
        .and_then(Value::as_str)
        .map(|s| absolute_url(BASE, s))
        .transpose()?
        .map(|s| image(s, &url));
    Ok(item)
}
impl StarCafe {
    fn page(&self, url: &str) -> Result<Vec<Value>> {
        let parsed = Url::parse(url).map_err(|_| Error::new("Invalid StarCafe URL"))?;
        if parsed.scheme() != "https" || parsed.host_str() != Some("starcafe.me") {
            return Err(Error::new("Not a StarCafe URL"));
        }
        let response = self
            .client
            .get(url)
            .send_with_challenge(&BrowserChallengePolicy::cloudflare(BASE).profile("starcafe"))?
            .error_for_status()?;
        flight(&html::document(response.text()?))
    }
    fn browse(&self, query: &str, page: u32, latest: bool) -> Result<Paged<CatalogItem>> {
        // The site loads this public index client-side. No site login or
        // cookies are sent to the catalog host.
        let document = Client::browser()
            .get(CATALOG_URL)
            .max_body_bytes(2_000_000)
            .send()?
            .error_for_status()?
            .json()?;
        let mut rows = catalog_rows(&document)?;
        if latest {
            rows.sort_by(|a, b| {
                b["meta_updated_at"]
                    .as_str()
                    .cmp(&a["meta_updated_at"].as_str())
            });
        } else {
            rows.sort_by_key(|v| std::cmp::Reverse(v["favorites"].as_u64().unwrap_or(0)));
        }
        let query = query.to_lowercase();
        let entries: Vec<_> = rows
            .iter()
            .filter(|r| {
                r["title"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query)
            })
            .map(item)
            .collect::<Result<_>>()?;
        let start = page.max(1).saturating_sub(1) as usize * 30;
        let next = entries.len() > start + 30;
        Ok(Paged::new(
            entries.into_iter().skip(start).take(30).collect(),
            next,
        ))
    }
    fn chapter_rows(rows: &[Value], url: &str) -> Result<Vec<NovelChapter>> {
        let index = data(rows, "toc").or_else(|_| data(rows, "latest_public_chapter"))?;
        let acronym = required(index, "acronym")?;
        let public = index["latest_public_chapter"]
            .as_f64()
            .ok_or_else(|| Error::new("StarCafe public chapter limit is missing"))?;
        let mut chapters = if let Some(toc) = index["toc"].as_object() {
            toc.iter()
                .map(|(key, row)| {
                    let number = key
                        .parse::<f64>()
                        .ok()
                        .filter(|number| number.is_finite() && *number >= 0.0)
                        .ok_or_else(|| Error::new("Invalid StarCafe chapter index key"))?;
                    Ok((
                        number,
                        row,
                        row["published_at"]
                            .as_str()
                            .is_some_and(|value| !value.is_empty()),
                    ))
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            index["chapters"]
                .as_array()
                .ok_or_else(|| Error::new("StarCafe chapter index is invalid"))?
                .iter()
                .map(|row| {
                    let number = row["chapter_num"]
                        .as_f64()
                        .ok_or_else(|| Error::new("StarCafe chapter number is missing"))?;
                    Ok((number, row, true))
                })
                .collect::<Result<Vec<_>>>()?
        };
        chapters.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut result = Vec::new();
        for (num, row, published) in chapters {
            let target = format!("{}/{acronym}-ch{}", url.trim_end_matches('/'), num);
            result.push(NovelChapter {
                key: target.clone(),
                url: Some(target),
                title: Some(format!(
                    "Chapter {num}: {}",
                    row["chapter_title"].as_str().unwrap_or("")
                )),
                chapter_number: Some(num as f32),
                language: Some("en".into()),
                is_locked: num > public || !published,
                ..NovelChapter::default()
            });
        }
        Ok(result)
    }
}
impl NovelSource for StarCafe {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", page, false)
    }
    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", page, true)
    }
    fn search(&mut self, q: &str, page: u32, _: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(q, page, false)
    }
    fn details(&mut self, i: CatalogItem) -> Result<CatalogItem> {
        let mut details = item(data(
            &self.page(i.url.as_deref().unwrap_or(&i.key))?,
            "novel_cover",
        )?)?;
        details.initialized = true;
        Ok(details)
    }
    fn chapters(&mut self, i: CatalogItem) -> Result<Vec<NovelChapter>> {
        let url = i.url.as_deref().unwrap_or(&i.key);
        Self::chapter_rows(&self.page(url)?, url)
    }
    fn text(&mut self, _: CatalogItem, ch: NovelChapter) -> Result<NovelText> {
        if ch.is_locked {
            return Err(Error::new(
                "This StarCafe chapter is locked. Open the website to sign in or unlock it.",
            ));
        }
        let url = ch.url.as_deref().unwrap_or(&ch.key);
        let payload = self.page(url)?;
        let row = data(&payload, "chapter_id")?;
        if row["is_published"].as_bool() != Some(true) {
            return Err(Error::new(
                "This StarCafe chapter is not publicly available. Open it on the website.",
            ));
        }
        let content = required(row, "content")?;
        let content = format!(
            "<main id=chapter>{}{}{}</main>",
            row["tn_top"].as_str().unwrap_or(""),
            content,
            row["tn_bot"].as_str().unwrap_or("")
        );
        chapter_text(&html::document(&content), "#chapter", url, ch.title)
    }
    fn handle_url(&mut self, url: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(p) = Url::parse(url) else {
            return Ok(None);
        };
        if p.scheme() != "https" || p.host_str() != Some("starcafe.me") {
            return Ok(None);
        }
        let parts: Vec<_> = p.path().trim_matches('/').split('/').collect();
        if parts.len() < 2 || parts[0] != "novels" {
            return Ok(None);
        }
        let target = format!("{BASE}/novels/{}", parts[1]);
        let mut i = CatalogItem::new(&target, "");
        i.url = Some(target);
        Ok(Some(UrlResolveResult {
            item: Some(i),
            ..UrlResolveResult::default()
        }))
    }
}
#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel("starcafe", StarCafe::default())
}
#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_the_public_catalog_document() {
        let rows = catalog_rows(&json!({"fields":{"EXAMPLE":{"mapValue":{"fields":{
            "slug":{"stringValue":"example"},
            "title":{"stringValue":"Example"},
            "favorites":{"integerValue":"12"},
            "meta_updated_at":{"timestampValue":"2026-11-11T00:00:00Z"},
            "tags":{"arrayValue":{"values":[{"stringValue":"Fantasy"}]}},
            "unused":{"geoPointValue":{"latitude":0,"longitude":0}}
        }}}}}))
        .unwrap();
        assert_eq!(rows[0]["favorites"], 12);
        let summary = item(&rows[0]).unwrap();
        assert_eq!(summary.tags, ["Fantasy"]);
        assert!(!summary.initialized);
        assert!(catalog_rows(&json!({"error":{"status":"PERMISSION_DENIED"}})).is_err());
    }

    #[test]
    fn keyed_toc_is_sorted_numerically_and_unpublished_entries_stay_locked() {
        let payload = vec![json!({"acronym":"TEST","latest_public_chapter":10,"toc":{
            "1":{"chapter_title":"First","published_at":"2026-11-01T00:00:00Z"},
            "2":{"chapter_title":"Unpublished"},
            "10":{"chapter_title":"Tenth","published_at":"2026-11-10T00:00:00Z"},
            "11":{"chapter_title":"Advance","published_at":"2026-11-11T00:00:00Z"}
        }})];
        let rows = StarCafe::chapter_rows(&payload, &format!("{BASE}/novels/test")).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.chapter_number.unwrap())
                .collect::<Vec<_>>(),
            [11., 10., 2., 1.]
        );
        assert!(rows[0].is_locked && !rows[1].is_locked && rows[2].is_locked && !rows[3].is_locked);
        assert!(rows[3].key.ends_with("/TEST-ch1"));
    }
    #[test]
    fn flight_reassembles_chunks_and_utf8_multiline_text() {
        let text = "<p>雪\n1:{not a record}</p>";
        let payload = format!(
            "1:T{:x},{text}2:{{\"chapter_id\":\"1\",\"content\":\"$1\"}}\n",
            text.len()
        );
        let (a, b) = payload.split_at(8);
        let doc = html::document(&format!(
            "<script>self.__next_f.push({})</script><script>self.__next_f.push({})</script>",
            json!([1, a]),
            json!([1, b])
        ));
        assert_eq!(
            data(&flight(&doc).unwrap(), "chapter_id").unwrap()["content"],
            text
        );
    }
    #[test]
    fn complete_index_not_just_visible_twenty_and_locks_preserved() {
        let chapters: Vec<_> = (1..=45)
            .map(|n| json!({"chapter_num":n,"chapter_title":"Test"}))
            .collect();
        let payload =
            vec![json!({"acronym":"TEST","latest_public_chapter":40,"chapters":chapters})];
        let result = StarCafe::chapter_rows(&payload, &format!("{BASE}/novels/test")).unwrap();
        assert_eq!(result.len(), 45);
        assert_eq!(result[0].chapter_number, Some(45.));
        assert!(result[0].is_locked);
        assert!(!result[5].is_locked);
        assert!(result[44].key.ends_with("/TEST-ch1"));
    }
    #[test]
    fn malformed_data_is_not_an_empty_catalog() {
        assert!(flight(&html::document("<p>Unavailable</p>")).is_err());
    }
    #[test]
    fn preload_hints_without_ids_do_not_break_the_catalog() {
        let payload = ":HL[\"/font.woff2\",\"font\"]\n1:{\"novels\":[]}\n";
        let doc = html::document(&format!(
            "<script>self.__next_f.push({})</script>",
            json!([1, payload])
        ));
        assert_eq!(
            data(&flight(&doc).unwrap(), "novels").unwrap()["novels"],
            json!([])
        );
    }
    #[test]
    fn blocked_chapter_is_rejected_before_network() {
        assert!(StarCafe::default()
            .text(
                CatalogItem::default(),
                NovelChapter {
                    is_locked: true,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .to_string()
            .contains("locked"));
    }
}
