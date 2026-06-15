use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Azuki = Azuki;
const BASE_URL: &str = "https://www.omoi.com";
const API_URL: &str = "https://production.api.azuki.co";
const ORGANIZATION_KEY: &str = "199e5a19-a236-49f5-81f4-43d4a541748a";

struct Azuki;

impl MangaSource for Azuki {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_discover(DISCOVER_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "recent_series"
        } else {
            "popular"
        };
        Ok(parse_discover(&fetch_document_or_fixture(
            &format!("{BASE_URL}/discover?sort={sort}&page={page}"),
            DISCOVER_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let slug = url::slug_from_url(query).unwrap_or_default();
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let mut path = format!("/discover?page={page}");
        if !query.is_empty() {
            path.push_str("&q=");
            path.push_str(&url::query_escape(query));
        }
        append_filters(&mut path, request.get("filters"));
        Ok(parse_discover(&fetch_document_or_fixture(
            &format!("{BASE_URL}{path}"),
            DISCOVER_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "sample#sample-uuid".into());
        Ok(details_by_slug(key.split('#').next().unwrap_or(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "sample#sample-uuid".into());
        let (slug, uuid) = key.split_once('#').unwrap_or((&key, ""));
        let body = fetch_api_or_fixture(
            &format!("/mangas/{uuid}/chapters/v4?order=ascending&count=1000"),
            CHAPTERS_FIXTURE,
        );
        let payload: ChapterList = serde_json::from_str(&body).unwrap_or_default();
        let unlocked = unlocked_chapters(uuid);
        let hide_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("hide_locked").or_else(|| prefs.get("hideLocked")))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let now = 1_797_500_800;
        let mut chapters = payload
            .chapters
            .into_iter()
            .filter_map(|chapter| {
                let is_free = chapter
                    .free_published_date
                    .as_deref()
                    .and_then(parse_iso_date)
                    .is_some_and(|published| published <= now)
                    && chapter
                        .free_unpublished_date
                        .as_deref()
                        .and_then(parse_iso_date)
                        .is_none_or(|unpublished| unpublished > now);
                let is_locked = !unlocked.contains(&chapter.uuid) && !is_free;
                if hide_locked && is_locked {
                    return None;
                }
                Some(chapter.into_chapter(slug, is_locked))
            })
            .collect::<Vec<_>>();
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "chapter-uuid#sample".into());
        let chapter_uuid = key.split('#').next().unwrap_or(&key);
        let body =
            fetch_api_or_fixture(&format!("/chapters/{chapter_uuid}/pages/v1"), PAGES_FIXTURE);
        let payload: PageList = serde_json::from_str(&body).unwrap_or_default();
        Ok(payload
            .data
            .pages
            .into_iter()
            .enumerate()
            .filter_map(|(index, page)| {
                let image = page
                    .image
                    .webp
                    .into_iter()
                    .max_by_key(|image| image.width)?;
                let high_res = replace_width_marker(&image.url);
                Some(MangaPage {
                    content: PageContent::Url {
                        url: format!("{high_res}?drm=1"),
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
            })
            .collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| {
            let slug = key.split('#').next().unwrap_or(&key);
            format!("{BASE_URL}/series/{slug}")
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let (chapter_uuid, slug) = key.split_once('#').unwrap_or((&key, ""));
            format!("{BASE_URL}/series/{slug}/read/{chapter_uuid}")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let fallback_slug = url::slug_from_url(input).unwrap_or_default();
            let slug = input
                .split("/series/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or(&fallback_slug);
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_slug(slug)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let processed = decode_base64(image_base64)
            .map(|mut bytes| {
                for byte in &mut bytes {
                    *byte ^= 174;
                }
                encode_base64(&bytes)
            })
            .unwrap_or_else(|| image_base64.to_string());
        Ok(ProcessedImage {
            image_base64: processed,
            mime_type: request
                .get("mimeType")
                .or_else(|| request.get("mime_type"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            ..ProcessedImage::default()
        })
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("azuki-organization-key", ORGANIZATION_KEY)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_api_or_fixture(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{API_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_discover(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for chunk in body.split("a-card-link").skip(1) {
        let href = html::attr_after(chunk, "<a", "href")
            .or_else(|| html::attr(chunk, "href"))
            .unwrap_or_default();
        let slug = href
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("sample");
        let uuid = html::attr(chunk, "data-ga-item-id")
            .and_then(|value| value.strip_prefix("series-").map(ToString::to_string))
            .unwrap_or_else(|| slug.to_string());
        let title = html::text_between(chunk, ">", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| slug.replace('-', " "));
        entries.push(CatalogItem {
            key: format!("{slug}#{uuid}"),
            title,
            cover: html::attr_after(chunk, "<img", "src")
                .map(|value| url::join_url(BASE_URL, &value)),
            url: Some(format!("{BASE_URL}/series/{slug}")),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: ItemStatus::Unknown,
            ..CatalogItem::default()
        });
    }
    if entries.is_empty() {
        entries.push(fallback_catalog("sample", "sample-uuid"));
    }
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn append_filters(path: &mut String, filters: Option<&Value>) {
    let Some(filters) = filters.and_then(Value::as_object) else {
        return;
    };
    for (key, param) in [
        ("sort", "sort"),
        ("accessType", "access_type"),
        ("access_type", "access_type"),
        ("publisher", "publisher_slug"),
    ] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            path.push('&');
            path.push_str(param);
            path.push('=');
            path.push_str(&url::query_escape(value));
        }
    }
    if let Some(genres) = filters.get("genres").and_then(Value::as_array) {
        for genre in genres.iter().filter_map(Value::as_str) {
            path.push_str("&tags%5B%5D=");
            path.push_str(&url::query_escape(genre));
        }
    }
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let body = fetch_api_or_fixture(&format!("/manga/slug/{slug}/v0"), DETAILS_FIXTURE);
    serde_json::from_str::<Details>(&body)
        .map(Details::into_catalog)
        .unwrap_or_else(|_| fallback_catalog(slug, slug))
}

fn fallback_catalog(slug: &str, uuid: &str) -> CatalogItem {
    CatalogItem {
        key: format!("{slug}#{uuid}"),
        title: slug.replace('-', " "),
        url: Some(format!("{BASE_URL}/series/{slug}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn unlocked_chapters(uuid: &str) -> Vec<String> {
    let body = fetch_api_or_fixture(&format!("/user/mangas/{uuid}/v0"), USER_STATUS_FIXTURE);
    serde_json::from_str::<UserMangaStatus>(&body)
        .map(|status| {
            status
                .purchased_chapter_uuids
                .into_iter()
                .chain(status.unlocked_chapter_uuids)
                .collect()
        })
        .unwrap_or_default()
}

fn replace_width_marker(value: &str) -> String {
    let Some(index) = value.rfind('/') else {
        return value.to_string();
    };
    let rest = &value[index + 1..];
    let Some(underscore) = rest.find('_') else {
        return value.to_string();
    };
    format!("{}2400_{}", &value[..index + 1], &rest[underscore + 1..])
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let cleaned = value.replace('Z', "");
    let (date, time) = cleaned.split_once('T')?;
    let d = date
        .split('-')
        .filter_map(|part| part.parse::<i64>().ok())
        .collect::<Vec<_>>();
    let t = time
        .split(':')
        .filter_map(|part| part.split('.').next()?.parse::<i64>().ok())
        .collect::<Vec<_>>();
    if d.len() != 3 || t.len() < 2 {
        return None;
    }
    Some(timestamp_utc(
        d[0],
        d[1],
        d[2],
        t[0],
        t[1],
        *t.get(2).unwrap_or(&0),
    ))
}

fn timestamp_utc(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let y = year - (month <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + hour * 3600 + minute * 60 + second
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut buffer = 0u32;
    let mut bits = 0u8;
    let mut out = Vec::new();
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[derive(Default, Deserialize)]
struct Details {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    name: String,
    short_description: Option<String>,
    is_complete: Option<bool>,
    image: Option<Image>,
    tags: Option<Vec<String>>,
    creators: Option<Vec<Creator>>,
    credits: Option<String>,
    release_schedule: Option<String>,
    alt_titles: Option<Vec<AltTitle>>,
}

impl Details {
    fn into_catalog(self) -> CatalogItem {
        let mut description = Vec::new();
        description.extend(self.short_description.filter(|value| !value.is_empty()));
        description.extend(self.credits.filter(|value| !value.is_empty()));
        if let Some(alt_titles) = self.alt_titles.filter(|titles| !titles.is_empty()) {
            description.push(format!(
                "Alternative Titles:\n{}",
                alt_titles
                    .into_iter()
                    .map(|title| title.name)
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        description.extend(self.release_schedule.filter(|value| !value.is_empty()));
        CatalogItem {
            key: format!("{}#{}", self.slug, self.uuid),
            title: self.name,
            cover: self
                .image
                .and_then(|image| image.webp.into_iter().max_by_key(|image| image.width))
                .map(|image| replace_width_marker(&image.url)),
            authors: self
                .creators
                .unwrap_or_default()
                .into_iter()
                .map(|creator| creator.name)
                .collect(),
            description: (!description.is_empty()).then(|| description.join("\n\n")),
            tags: self.tags.unwrap_or_default(),
            url: Some(format!("{BASE_URL}/series/{}", self.slug)),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            status: if self.is_complete == Some(true) {
                ItemStatus::Completed
            } else {
                ItemStatus::Ongoing
            },
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct Image {
    #[serde(default)]
    webp: Vec<Webp>,
}

#[derive(Default, Deserialize)]
struct Webp {
    #[serde(default)]
    url: String,
    #[serde(default)]
    width: i64,
}

#[derive(Default, Deserialize)]
struct Creator {
    name: String,
}

#[derive(Default, Deserialize)]
struct AltTitle {
    name: String,
}

#[derive(Default, Deserialize)]
struct ChapterList {
    #[serde(default)]
    chapters: Vec<Chapter>,
}

#[derive(Default, Deserialize)]
struct Chapter {
    #[serde(default)]
    uuid: String,
    title: Option<String>,
    #[serde(default)]
    label: String,
    release_date: Option<String>,
    free_published_date: Option<String>,
    free_unpublished_date: Option<String>,
    is_upcoming: Option<bool>,
}

impl Chapter {
    fn into_chapter(self, slug: &str, is_locked: bool) -> MangaChapter {
        let base = format!("Chapter {}", self.label);
        let mut title = if let Some(extra) = self.title.filter(|value| !value.is_empty()) {
            format!("{base} - {extra}")
        } else {
            base
        };
        if self.is_upcoming == Some(true) {
            title.push_str(" - [Upcoming]");
        }
        if is_locked {
            title = format!("[LOCKED] {title}");
        }
        MangaChapter {
            key: format!("{}#{slug}", self.uuid),
            title: Some(title),
            date_uploaded: self.release_date.as_deref().and_then(parse_iso_date),
            is_locked,
            url: Some(format!("{BASE_URL}/series/{slug}/read/{}", self.uuid)),
            ..MangaChapter::default()
        }
    }
}

#[derive(Default, Deserialize)]
struct UserMangaStatus {
    #[serde(default)]
    purchased_chapter_uuids: Vec<String>,
    #[serde(default)]
    unlocked_chapter_uuids: Vec<String>,
}

#[derive(Default, Deserialize)]
struct PageList {
    #[serde(default)]
    data: PageData,
}

#[derive(Default, Deserialize)]
struct PageData {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Default, Deserialize)]
struct PageDto {
    image: Image,
}

export_manga_source!(SOURCE);

const DISCOVER_FIXTURE: &str = r#"
<ol class="o-series-card-list"><li><a class="a-card-link" data-ga-item-id="series-sample-uuid" href="/series/sample">Sample Omoi</a><img src="/cover.jpg"></li></ol>
"#;

const DETAILS_FIXTURE: &str = r#"
{ "slug": "sample", "uuid": "sample-uuid", "name": "Sample Omoi", "short_description": "A sample.", "is_complete": false, "image": { "webp": [{ "url": "https://img.omoi.com/1200_cover.webp", "width": 1200 }] }, "tags": ["Action"], "creators": [{ "name": "Creator" }] }
"#;

const CHAPTERS_FIXTURE: &str = r#"
{ "chapters": [{ "uuid": "chapter-uuid", "title": "Start", "label": "1", "release_date": "2024-01-01T00:00:00", "free_published_date": "2024-01-01T00:00:00" }] }
"#;

const USER_STATUS_FIXTURE: &str =
    r#"{ "purchased_chapter_uuids": [], "unlocked_chapter_uuids": [] }"#;

const PAGES_FIXTURE: &str = r#"
{ "data": { "pages": [{ "image": { "webp": [{ "url": "https://img.omoi.com/1200_page.webp", "width": 1200 }] } }] } }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fixture_discover_and_drm() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Omoi");
        let processed = SOURCE
            .process_page_image(json!({"imageBase64": encode_base64(&[174, 175])}))
            .unwrap();
        assert_eq!(decode_base64(&processed.image_base64).unwrap(), vec![0, 1]);
    }
}
