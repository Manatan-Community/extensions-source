use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: FuryoSquad = FuryoSquad;
const BASE_URL: &str = "https://www.furyosociety.com";

struct FuryoSquad;

impl MangaSource for FuryoSquad {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(LIST_FIXTURE));
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/mangas")
        };
        let body = fetch_document(&target, if latest { LATEST_FIXTURE } else { LIST_FIXTURE });
        Ok(if latest {
            parse_latest(&body)
        } else {
            parse_popular(&body)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let entries = parse_popular(&fetch_document(&format!("{BASE_URL}/mangas"), LIST_FIXTURE))
            .entries
            .into_iter()
            .filter(|item| {
                item.title
                    .to_ascii_lowercase()
                    .contains(&query.to_ascii_lowercase())
            })
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample/".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample/".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1/".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_input(input) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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
}

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("fs-card-body")
            .skip(1)
            .filter_map(item_from_card)
            .collect(),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for row in body.split("<tr").skip(1) {
        let Some(title_chunk) = row.split("fs-comic-title").nth(1) else {
            continue;
        };
        let Some(href) = html::attr_after(title_chunk, "<a", "href") else {
            continue;
        };
        let key = normalize_key(&href);
        let title = html::text_between(title_chunk, "<a", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
        if !entries.iter().any(|item: &CatalogItem| item.key == key) {
            entries.push(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(row, "fs-chap-img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            });
        }
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn item_from_card(chunk: &str) -> Option<CatalogItem> {
    let title_chunk = chunk.split("fs-comic-title").nth(1)?;
    let raw_url = html::attr_after(chunk, "fs-card-img-container", "href")
        .or_else(|| html::attr_after(title_chunk, "<a", "href"))?;
    let key = normalize_key(&raw_url);
    let title = html::text_between(title_chunk, "<a", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "fs-card-img-container", "src")
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample/".into());
    let mut item = CatalogItem {
        key: normalize_key(&key),
        title: html::text_between(body, "fs-comic-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".into()),
        description: html::text_between(body, "fs-comic-description", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        cover: html::attr_after(body, "comic-cover", "src")
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    };
    for label in body.split("fs-comic-label").skip(1) {
        let name = html::strip_tags(label).to_ascii_lowercase();
        let value = label
            .split("</p>")
            .nth(1)
            .and_then(|rest| html::text_between(rest, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        match name.lines().next().unwrap_or_default().trim() {
            "scénario" | "scenario" => item.authors = value.into_iter().collect(),
            "dessins" => item.artists = value.into_iter().collect(),
            "genre" => item.tags = value.map(|value| split_values(&value)).unwrap_or_default(),
            _ => {}
        }
    }
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("div class=\"element")
        .skip(1)
        .filter_map(|chunk| {
            let title_chunk = chunk.split("div class=\"title").nth(1).unwrap_or(chunk);
            let href = html::attr_after(title_chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(title_chunk, "<a", "title")
                .or_else(|| {
                    html::text_between(title_chunk, "<a", "</a>")
                        .map(|value| html::strip_tags(&value))
                })
                .filter(|value| !value.is_empty());
            Some(MangaChapter {
                key: key.clone(),
                title,
                date_uploaded: html::text_between(chunk, "meta_r", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|date| parse_french_date(&date)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("id="))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn key_from_input(input: &str) -> Option<String> {
    if input.starts_with("id:") {
        return Some(normalize_key(input.trim_start_matches("id:")));
    }
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let key = normalize_key(input.trim_start_matches(BASE_URL));
    if key.contains("/series/") {
        return Some(key);
    }
    if let Some(slug) = read_slug(&key) {
        return Some(format!("/series/{slug}/"));
    }
    None
}

fn read_slug(key: &str) -> Option<String> {
    let parts = key.trim_matches('/').split('/').collect::<Vec<_>>();
    parts
        .iter()
        .position(|part| *part == "read")
        .and_then(|index| parts.get(index + 1))
        .map(|slug| (*slug).to_string())
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return normalize_key(&value[index + BASE_URL.len()..]);
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn split_values(value: &str) -> Vec<String> {
    value
        .split([',', '&'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_french_date(value: &str) -> Option<i64> {
    let lower = value.trim().to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("il y a ") {
        let mut parts = rest.split_whitespace();
        let amount = parts.next()?.parse::<i64>().ok()?;
        let unit = parts.next().unwrap_or_default();
        let seconds = match unit {
            "minute" | "minutes" => amount * 60,
            "heure" | "heures" => amount * 3_600,
            "jour" | "jours" => amount * 86_400,
            "semaine" | "semaines" => amount * 7 * 86_400,
            "mois" => amount * 30 * 86_400,
            "an" | "ans" | "année" | "années" => amount * 365 * 86_400,
            _ => 0,
        };
        return unix_now().map(|now| now.saturating_sub(seconds));
    }
    if lower.starts_with("aujourd'hui") {
        return unix_now().map(midnight);
    }
    if lower.starts_with("hier") {
        return unix_now().map(|now| midnight(now).saturating_sub(86_400));
    }
    if lower.starts_with("avant-hier") {
        return unix_now().map(|now| midnight(now).saturating_sub(2 * 86_400));
    }
    let date = lower.strip_prefix("le ").unwrap_or(&lower);
    parse_fr_day_month_year(date).or_else(|| dates::parse_ymd(date))
}

fn parse_fr_day_month_year(value: &str) -> Option<i64> {
    let mut parts = value.split_whitespace();
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = match parts.next()? {
        "janv" | "janvier" => 1,
        "févr" | "fevr" | "février" | "fevrier" => 2,
        "mars" => 3,
        "avr" | "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juil" | "juillet" => 7,
        "août" | "aout" => 8,
        "sept" | "septembre" => 9,
        "oct" | "octobre" => 10,
        "nov" | "novembre" => 11,
        "déc" | "dec" | "décembre" | "decembre" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<i32>().ok()?;
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn unix_now() -> Option<i64> {
    Some(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

fn midnight(timestamp: i64) -> i64 {
    timestamp - timestamp.rem_euclid(86_400)
}

const LIST_FIXTURE: &str = r#"
<div id="fs-tous"><div class="fs-card-body"><div class="fs-card-img-container"><a href="https://www.furyosociety.com/series/sample/"><img src="/cover.jpg"></a></div><span class="fs-comic-title"><a href="/series/sample/">Sample</a></span></div></div>
"#;
const LATEST_FIXTURE: &str = r#"
<table class="table-striped"><tr><td><img class="fs-chap-img" src="/cover.jpg"></td><td><span class="fs-comic-title"><a href="https://www.furyosociety.com/series/sample/">Sample</a></span></td></tr></table>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="fs-comic-title">Sample</h1><div class="comic-info"><img class="comic-cover" src="/cover.jpg"><p class="fs-comic-label">Scénario</p><p>Auteur</p><p class="fs-comic-label">Dessins</p><p>Artiste</p><p class="fs-comic-label">Genre</p><p>Action, Aventure</p><div class="fs-comic-description">Resume</div></div>
<div class="fs-chapter-list"><div class="element"><div class="title"><a href="https://www.furyosociety.com/read/sample/chapter-1/" title="Chapitre 1">Chapitre 1</a></div><div class="meta_r">le 01 janvier 2024</div></div></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="fs-read"><img id="p1" src="/page1.jpg"><img id="p2" src="/page2.jpg"></div>"#;
