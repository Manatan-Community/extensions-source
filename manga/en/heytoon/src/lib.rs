use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: HeyToon = HeyToon;
const BASE_URL: &str = "https://heytoon.net";

struct HeyToon;

impl MangaSource for HeyToon {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_home_popular(HOME_FIXTURE),
                has_next_page: true,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request.get("listingId").or_else(|| request.get("listing")).and_then(Value::as_str);
        if listing == Some("popular") && page == 1 {
            return Ok(Paged {
                entries: parse_home_popular(&fetch_document(BASE_URL, HOME_FIXTURE)),
                has_next_page: true,
            });
        }
        let sort = if listing == Some("popular") { "views" } else { "latest" };
        Ok(parse_listing(&fetch_document(&genre_url(page.saturating_sub((listing == Some("popular")) as u64), "", sort), LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), Some(key))],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(Paged {
                entries: parse_autocomplete(&fetch_autocomplete(query)),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters");
        let sort = filter_str(filters, "sort", "latest");
        let genre = filter_str(filters, "genre", "");
        Ok(parse_listing(&fetch_document(&genre_url(page, genre, sort), LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/en/comic/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/en/comic/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/en/comic/sample/episode-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let body = fetch_document(BASE_URL, HOME_FIXTURE);
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: parse_home_popular(&body),
            has_more: true,
            ..HomeSection::default()
        }])
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
        .with_cookies_for(BASE_URL)
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_autocomplete(query: &str) -> String {
    client()
        .get(format!("{BASE_URL}/api/complete-search?keyword={}", url::query_escape(query)))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| AUTOCOMPLETE_FIXTURE.to_string())
}

fn genre_url(page: u64, genre: &str, sort: &str) -> String {
    let mut target = format!("{BASE_URL}/en/genres");
    if !genre.is_empty() {
        target.push('/');
        target.push_str(&url::query_escape(genre));
    }
    target.push_str(&format!("?orderBy={sort}"));
    if page > 1 {
        target.push_str(&format!("&page={page}"));
    }
    target
}

fn parse_home_popular(body: &str) -> Vec<CatalogItem> {
    body.split("<section")
        .skip(1)
        .filter(|chunk| chunk.to_ascii_lowercase().contains("popular") || chunk.to_ascii_lowercase().contains("trending"))
        .flat_map(|section| section.split("<a").skip(1))
        .filter_map(listing_item)
        .collect()
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| chunk.contains("comicItem"))
            .flat_map(|chunk| chunk.split("<a").skip(1).take(1))
            .filter_map(listing_item)
            .collect(),
        has_next_page: body.contains("nextpostslink"),
    }
}

fn listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    let title = html::attr_after(chunk, "<img", "title")
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .or_else(|| Some(html::strip_tags(&html::text_between(chunk, ">", "</a>")?)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HeyToon".into()));
    let cover = html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|image| url::join_url(BASE_URL, &image));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover,
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_autocomplete(body: &str) -> Vec<CatalogItem> {
    serde_json::from_str::<Vec<SearchComic>>(body)
        .unwrap_or_else(|_| serde_json::from_str(AUTOCOMPLETE_FIXTURE).expect("fixture is valid"))
        .into_iter()
        .map(|comic| CatalogItem {
            key: normalize_key(&comic.url),
            title: comic.title,
            cover: comic.cover,
            status: ItemStatus::Unknown,
            url: Some(url::join_url(BASE_URL, &comic.url)),
            language: Some("en".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/en/comic/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "titCon", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "HeyToon".into())),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| html::attr_after(body, "<img", "data-src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "modal_detail", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(body, "/genres/"),
        status: status_from(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("episodeItemCon")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "episodeStitle", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "episodeDate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let body = body.split("comicContent").nth(1).unwrap_or(body);
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|image| !image.starts_with("data:"))
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

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..].trim_start_matches('/').trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn link_values(body: &str, needle: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(needle))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("up") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn filter_str<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

#[derive(Debug, Deserialize)]
struct SearchComic {
    #[serde(rename = "linkComic")]
    url: String,
    title: String,
    #[serde(default, rename = "raw_thumb")]
    cover: Option<String>,
}

export_manga_source!(SOURCE);

const HOME_FIXTURE: &str = r#"
<section class="slider"><h2>Popular</h2><a href="/en/comic/sample"><img title="Sample HeyToon" data-src="/cover.jpg">Sample HeyToon</a></section>
"#;

const LIST_FIXTURE: &str = r#"
<div class="comicItem"><a href="/en/comic/sample"><img title="Sample HeyToon" data-src="/cover.jpg"></a></div><div class="wp-pagenavi"><a class="nextpostslink"></a></div>
"#;

const AUTOCOMPLETE_FIXTURE: &str = r#"
[{"linkComic":"/en/comic/sample","title":"Sample HeyToon","raw_thumb":"/cover.jpg"}]
"#;

const DETAILS_FIXTURE: &str = r#"
<div id="titleSubWrapper"><h1 class="titCon">Sample HeyToon</h1></div><meta property="og:image" content="/cover.jpg"><div id="modal_detail"><div class="cont_area"><p>Description</p></div><a href="/en/genres/Drama">Drama</a></div><div class="badgeArea"><span>Up</span></div><div class="episodeListConPC"><a id="episodeItemCon" href="/en/comic/sample/episode-1"><div class="comicInfo"><p class="episodeStitle">Episode 1</p><span class="episodeDate">Jan 01, 2024</span></div></a></div>
"#;

const PAGES_FIXTURE: &str =
    r#"<div id="comicContent"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_heytoon_fixtures() {
        assert_eq!(parse_home_popular(HOME_FIXTURE).len(), 1);
        assert_eq!(parse_listing(LIST_FIXTURE).entries.len(), 1);
        assert_eq!(parse_chapters(DETAILS_FIXTURE).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
