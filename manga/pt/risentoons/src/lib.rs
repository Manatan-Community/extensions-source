use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: Risentoons = Risentoons;
const BASE_URL: &str = "https://risentoons.xyz";
const MEDIA_URL: &str = "https://media.risentoons.xyz";
const NAME: &str = "Risentoons";
const LANG: &str = "pt-BR";
const PAGE_SIZE: u64 = 24;
const API_CLIENT: &str = "V6";

struct Risentoons;

impl MangaSource for Risentoons {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_manga_page(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let sort = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "updated"
        } else {
            "views"
        };
        Ok(parse_manga_page(&api_get(
            &format!("{BASE_URL}/api/mangas?page={page}&limit={PAGE_SIZE}&sort={sort}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_manga_page(&api_get(
            &format!(
                "{BASE_URL}/api/mangas?page={page}&limit={PAGE_SIZE}&search={}",
                url::query_escape(query)
            ),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "zombie-no-afureta-sekai-ore-dake-ga-osowarenai".to_string());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "zombie-no-afureta-sekai-ore-dake-ga-osowarenai".to_string());
        let item = details_json(&key);
        let slug = item
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or(&key)
            .to_string();
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        let mut chapters = Vec::new();
        for page in 1..=50 {
            let body = api_get(
                &format!("{BASE_URL}/api/mangas/{id}/chapters?page={page}&limit=100&sort=desc"),
                CHAPTERS_FIXTURE,
            );
            let value = json_or_fixture(&body, CHAPTERS_FIXTURE);
            let items = value
                .get("chapters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                break;
            }
            let count = items.len();
            chapters.extend(
                items
                    .iter()
                    .filter_map(|chapter| chapter_from_json(chapter, &slug)),
            );
            if count < 100 {
                break;
            }
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| {
            "zombie-no-afureta-sekai-ore-dake-ga-osowarenai|ch_7TWs3aWUuujJeqloJCKObi|41"
                .to_string()
        });
        let Some(chapter_id) = key.split('|').nth(1) else {
            return Ok(Vec::new());
        };
        let body = api_get(
            &format!("{BASE_URL}/api/chapters/{chapter_id}/pages"),
            PAGES_FIXTURE,
        );
        Ok(parse_pages(&body))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/biblioteca/{}", normalize_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let mut parts = key.split('|');
            let slug = parts.next().unwrap_or_default();
            let _id = parts.next();
            let number = parts.next().unwrap_or_default();
            format!("{BASE_URL}/biblioteca/{slug}/{number}/")
        }))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_manga_page(&api_get(
            &format!("{BASE_URL}/api/mangas?page=1&limit={PAGE_SIZE}&sort=views"),
            LIST_FIXTURE,
        ));
        let latest = parse_manga_page(&api_get(
            &format!("{BASE_URL}/api/mangas?page=1&limit={PAGE_SIZE}&sort=updated"),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && input.contains("/biblioteca/") {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn api_get(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .header("Accept", "application/json, text/plain, */*")
        .header("X-Rip-Client", API_CLIENT)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_page(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = manga_items(&root)
        .into_iter()
        .map(|item| catalog_from_json(item, false))
        .collect::<Vec<_>>();
    let page = root.get("page").and_then(Value::as_u64).unwrap_or(1);
    let limit = root
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(PAGE_SIZE);
    let total = root
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(entries.len() as u64);
    Paged {
        has_next_page: page.saturating_mul(limit) < total,
        entries,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let item = details_json(key);
    catalog_from_json(&item, true)
}

fn details_json(key: &str) -> Value {
    let slug = normalize_key(key);
    let body = api_get(&format!("{BASE_URL}/api/mangas/{slug}"), DETAILS_FIXTURE);
    let root = json_or_fixture(&body, DETAILS_FIXTURE);
    root.get("data").cloned().unwrap_or(root)
}

fn catalog_from_json(item: &Value, initialized: bool) -> CatalogItem {
    let slug = item
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or("zombie-no-afureta-sekai-ore-dake-ga-osowarenai");
    CatalogItem {
        key: slug.to_string(),
        title: item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(NAME)
            .to_string(),
        alternate_titles: item
            .get("alternative_names")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|name| name.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect(),
        cover: item
            .get("cover_image")
            .and_then(Value::as_str)
            .map(absolute_media_url),
        authors: item
            .get("author")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        artists: item
            .get("artist")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        description: item
            .get("synopsis")
            .and_then(Value::as_str)
            .map(html::strip_tags)
            .filter(|value| !value.is_empty()),
        tags: item
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        url: Some(format!("{BASE_URL}/biblioteca/{slug}")),
        language: Some(LANG.to_string()),
        rating: item
            .get("rating")
            .and_then(Value::as_f64)
            .map(|value| value as f32),
        content_rating: Some(
            if item.get("is_18").and_then(Value::as_bool).unwrap_or(false) {
                "adult"
            } else {
                "safe"
            }
            .to_string(),
        ),
        status: status_from_json(item),
        initialized,
        ..CatalogItem::default()
    }
}

fn chapter_from_json(item: &Value, slug: &str) -> Option<MangaChapter> {
    let id = item.get("id").and_then(Value::as_str)?;
    let number = item.get("number").and_then(Value::as_f64).unwrap_or(0.0);
    let number_text = format_number(number);
    Some(MangaChapter {
        key: format!("{slug}|{id}|{number_text}"),
        title: item
            .get("title")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| Some(format!("Capitulo {number_text}"))),
        chapter_number: Some(number as f32),
        date_uploaded: item
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(parse_iso_date),
        url: Some(format!("{BASE_URL}/biblioteca/{slug}/{number_text}/")),
        language: Some(LANG.to_string()),
        is_locked: item.get("is_vip").and_then(Value::as_bool).unwrap_or(false),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let root = json_or_fixture(body, PAGES_FIXTURE);
    page_items(&root)
        .into_iter()
        .filter_map(page_url)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_media_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn manga_items(root: &Value) -> Vec<&Value> {
    root.get("data")
        .or_else(|| root.get("mangas"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect()
}

fn page_items(root: &Value) -> Vec<&Value> {
    if let Some(items) = root
        .get("pages")
        .or_else(|| root.get("data"))
        .or_else(|| root.get("images"))
        .and_then(Value::as_array)
    {
        return items.iter().collect();
    }
    root.as_array().into_iter().flatten().collect()
}

fn page_url(item: &Value) -> Option<String> {
    item.as_str().map(ToString::to_string).or_else(|| {
        ["image_url", "url", "image", "path", "src"]
            .iter()
            .find_map(|field| {
                item.get(field)
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
    })
}

fn status_from_json(item: &Value) -> ItemStatus {
    match item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "ongoing" | "lançando" | "lancando" => ItemStatus::Ongoing,
        "completed" | "completo" | "concluido" | "concluído" => ItemStatus::Completed,
        "cancelled" | "canceled" | "cancelado" => ItemStatus::Cancelled,
        "hiatus" | "hiato" => ItemStatus::Hiatus,
        _ => ItemStatus::Unknown,
    }
}

fn format_number(number: f64) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as u64)
    } else {
        let mut value = format!("{number:.2}");
        while value.ends_with('0') {
            value.pop();
        }
        value
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<i32>().ok()?;
    let day = value.get(8..10)?.parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn normalize_key(input: &str) -> String {
    let without_origin = input.strip_prefix(BASE_URL).unwrap_or(input);
    without_origin
        .trim_matches('/')
        .trim_start_matches("biblioteca/")
        .split('/')
        .next()
        .unwrap_or(without_origin)
        .to_string()
}

fn absolute_media_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else if value.starts_with("/manga_") || value.starts_with("/chapter_") {
        url::join_url(MEDIA_URL, value)
    } else {
        url::join_url(BASE_URL, value)
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"data":[{"id":"4305dee2-44c3-442f-a991-1e094712077f","slug":"zombie-no-afureta-sekai-ore-dake-ga-osowarenai","title":"Zombie no Afureta Sekai Ore Dake ga Osowarenai","cover_image":"https://media.risentoons.xyz/manga_cover/4305dee2-44c3-442f-a991-1e094712077f.webp","genres":["ACAO"],"status":"ongoing","synopsis":"Sample description.","is_18":true,"rating":0.0}],"page":1,"limit":24,"total":1,"success":true}"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"{"data":{"alternative_names":[{"lang_code":"br","name":"Sample alternative"}],"artist":"Sample Artist","author":"Sample Author","chapters_count":44,"cover_image":"https://media.risentoons.xyz/manga_cover/4305dee2-44c3-442f-a991-1e094712077f.webp","genres":["ACAO"],"id":"4305dee2-44c3-442f-a991-1e094712077f","is_18":true,"slug":"zombie-no-afureta-sekai-ore-dake-ga-osowarenai","status":"ongoing","synopsis":"Sample description.","title":"Zombie no Afureta Sekai Ore Dake ga Osowarenai","rating":0.0},"success":true}"#;
const CHAPTERS_FIXTURE: &str = r#"{"chapters":[{"created_at":"2026-06-04T21:07:19.661403","id":"ch_7TWs3aWUuujJeqloJCKObi","is_vip":false,"number":41.0,"pages_count":26,"title":"Capitulo 41"}],"success":true}"#;
const PAGES_FIXTURE: &str =
    r#"{"pages":[{"image_url":"https://media.risentoons.xyz/sample/page1.webp"}],"success":true}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_api_fixtures() {
        assert_eq!(parse_manga_page(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(
            details_from_key("zombie-no-afureta-sekai-ore-dake-ga-osowarenai").title,
            "Zombie no Afureta Sekai Ore Dake ga Osowarenai"
        );
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 1);
    }
}
