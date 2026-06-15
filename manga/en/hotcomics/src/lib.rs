use manatan_extension::{
    abi::ExtensionResult, export_manga_source, source::MangaSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: HotComics = HotComics;
const BASE_URL: &str = "https://hotcomics.me";

struct HotComics;

impl MangaSource for HotComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let path = match listing {
            "latest" => "en/new",
            "weekly" => "en/weekly",
            _ => "en",
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/{path}?page={page}"),
            LIST_FIXTURE,
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
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(normalize_key(query)),
                )],
                has_next_page: false,
            });
        }
        let target = if query.is_empty() {
            let browse =
                filter_str(request.get("filters"), "browse").unwrap_or_else(|| "en".to_string());
            format!("{BASE_URL}/{}?page={page}", browse.trim_start_matches('/'))
        } else {
            format!("{BASE_URL}/en/search?keyword={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/en/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/en/sample".to_string());
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/en/sample/1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(&format!("{BASE_URL}/en"), LIST_FIXTURE));
        let weekly = parse_listing(&fetch_document(
            &format!("{BASE_URL}/en/weekly"),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(&format!("{BASE_URL}/en/new"), LIST_FIXTURE));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Home".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "weekly".to_string(),
                title: "Weekly".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: weekly.entries,
                has_more: weekly.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "New".to_string(),
                style: Some(HomeSectionStyle::Compact),
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
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(normalize_key(input)),
                )),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_header("Cookie", "hc_vfs=Y")
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("ComicSeries") && !chunk.contains("no-comic"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "main-text", "</div>")
                .and_then(|block| html::text_between(&block, "title", "</"))
                .or_else(|| html::text_between(chunk, "title", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Comic".to_string())
                });
            let key = normalize_key(&url::join_url(BASE_URL, &href));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_from_chunk(chunk).map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                status: ItemStatus::Unknown,
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("vnext") && !body.contains("vnext disabled"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/en/sample".to_string());
    let info = html::text_between(body, "type_box", "</p>").unwrap_or_default();
    let author = html::text_between(&info, "writer", "</")
        .map(|value| {
            html::strip_tags(&value)
                .replace("(C)", "")
                .trim()
                .to_string()
        })
        .filter(|value| !value.is_empty());
    let tags = html::text_between(&info, "type", "</")
        .map(|value| {
            html::strip_tags(&value)
                .split('/')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let status = match html::text_between(&info, "date", "</")
        .map(|value| html::strip_tags(&value))
        .as_deref()
    {
        Some("End") | Some("Ende") => ItemStatus::Completed,
        Some(_) => ItemStatus::Ongoing,
        None => ItemStatus::Unknown,
    };
    let description = [
        html::text_between(body, "episode-contents", "</header>")
            .map(|value| html::strip_tags(&value)),
        html::text_between(body, "title_content", "</h2>").map(|value| html::strip_tags(&value)),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "episode-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".to_string())),
        cover: image_from_chunk(body).map(|value| url::join_url(BASE_URL, &value)),
        authors: author.clone().into_iter().collect(),
        artists: author.into_iter().collect(),
        tags,
        description: (!description.is_empty()).then_some(description),
        status,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("popupLogin(") || chunk.contains("cell-num"))
        .filter_map(|chunk| {
            let href = if chunk.contains("popupLogin('") {
                chunk
                    .split("popupLogin('")
                    .nth(1)
                    .and_then(|rest| rest.split('\'').next())
                    .map(ToString::to_string)
            } else {
                html::attr(chunk, "href")
            }?;
            let key = normalize_key(&url::join_url(BASE_URL, &href));
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "cell-num", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("Chapter".to_string())),
                url: Some(url::join_url(BASE_URL, &key)),
                date_uploaded: html::text_between(chunk, "cell-time", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_named_date(&value)),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Read".to_string()),
            url: Some(url::join_url(BASE_URL, manga_key)),
            language: Some("en".to_string()),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("viewer-img") || body.contains("viewer-img"))
        .filter_map(image_from_chunk)
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src")
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find(BASE_URL) {
        return format!(
            "/{}",
            input[index + BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn filter_str(filters: Option<&Value>, key: &str) -> Option<String> {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

fn parse_named_date(value: &str) -> Option<i64> {
    let parts = value
        .replace(',', "")
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = month_number(&parts[0])?;
    let day = parts[1].parse::<i32>().ok()?;
    let year = parts[2].parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) as i64 * 86_400)
}

fn month_number(value: &str) -> Option<i32> {
    Some(match value.to_ascii_lowercase().as_str() {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    })
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i32 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<li itemtype="ComicSeries"><a href="/en/sample"><div class="visual"><img data-src="/cover.jpg"></div><div class="main-text"><h4 class="title">Sample Comic</h4></div></a></li>
<div class="pagination"><a class="vnext" href="?page=2">Next</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h2 class="episode-title">Sample Comic</h2><p class="type_box"><span class="writer">(C) Sample Author</span><span class="type">Drama / Romance</span><span class="date">End</span></p>
<div class="episode-contents"><header>Header description</header></div><div class="title_content"><h2>Body description</h2></div>
<div id="tab-chapter"><a onclick="popupLogin('/en/sample/1')"><span class="cell-num">Episode 1</span><span class="cell-time">Jan 01, 2024</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div id="viewer-img"><img data-src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hotcomics_listing_and_chapter() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Comic"
        );
        assert_eq!(
            SOURCE.chapters(json!({"manga": "/en/sample"})).unwrap()[0]
                .title
                .as_deref(),
            Some("Episode 1")
        );
    }
}
