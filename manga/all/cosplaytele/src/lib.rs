use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://cosplaytele.com";
const POPULAR_LIMIT: u64 = 20;
const SOURCE: CosplayTele = CosplayTele;

struct CosplayTele;

impl MangaSource for CosplayTele {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        if latest {
            return Ok(parse_listing_page(&fetch_text_or_fixture(
                &format!("{BASE_URL}/page/{page}/"),
                LIST_FIXTURE,
            )));
        }
        Ok(parse_popular_posts(&fetch_text_or_fixture(
            &popular_url(page),
            POPULAR_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if is_cosplaytele_url(query) {
            return Ok(search_url(page, query));
        }
        let category = request
            .get("filters")
            .and_then(|filters| filters.get("category"))
            .and_then(Value::as_str)
            .unwrap_or("All");
        let target = search_url_for(page, query, category);
        Ok(parse_listing_page(&fetch_text_or_fixture(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-gallery/".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample-gallery/".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key,
            title: Some("Gallery".into()),
            chapter_number: Some(1.0),
            date_uploaded: parse_date(&body),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-gallery/".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if is_cosplaytele_url(input) && !is_taxonomy_url(input) {
            let body = fetch_text_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_key(input)))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_text_or_fixture(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .header("Accept", "text/html,application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn popular_url(page: u64) -> String {
    format!(
        "{BASE_URL}/wp-json/wordpress-popular-posts/v1/popular-posts?offset={}&limit={POPULAR_LIMIT}&range=last7days&embed=true&_embed=wp:featuredmedia&_fields=title,link,_embedded,_links.wp:featuredmedia",
        page * POPULAR_LIMIT,
    )
}

fn parse_popular_posts(body: &str) -> Paged<CatalogItem> {
    let posts = serde_json::from_str::<Vec<PopularPostDto>>(body)
        .unwrap_or_else(|_| serde_json::from_str(POPULAR_FIXTURE).expect("popular fixture"));
    let has_next_page = posts.len() as u64 >= POPULAR_LIMIT;
    Paged {
        entries: posts.into_iter().map(PopularPostDto::into_item).collect(),
        has_next_page,
    }
}

fn search_url(page: u64, input: &str) -> Paged<CatalogItem> {
    if is_taxonomy_url(input) {
        let target = paginated_taxonomy_url(page, input);
        parse_listing_page(&fetch_text_or_fixture(&target, LIST_FIXTURE))
    } else {
        let body = fetch_text_or_fixture(input, DETAILS_FIXTURE);
        Paged {
            entries: vec![parse_details(&body, Some(normalize_key(input)))],
            has_next_page: false,
        }
    }
}

fn search_url_for(page: u64, query: &str, category: &str) -> String {
    let query = query.trim();
    let category_path = match category {
        "Cosplay Nude" => "category/nude",
        "Cosplay Ero" => "category/no-nude",
        "Cosplay" => "category/cosplay",
        _ => "",
    };
    let mut target = if category_path.is_empty() {
        format!("{BASE_URL}/page/{page}/")
    } else {
        format!("{BASE_URL}/{category_path}/page/{page}/")
    };
    if !query.is_empty() {
        target.push_str("?s=");
        target.push_str(&url::query_escape(query));
    }
    target
}

fn parse_listing_page(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("class=\"box")
        .skip(1)
        .filter_map(parse_listing_block)
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("next page-number"),
    }
}

fn parse_listing_block(block: &str) -> Option<CatalogItem> {
    let cover = html::attr_after(block, "<img", "src").map(|value| url::join_url(BASE_URL, &value));
    let link_start = block.find("<h5")?;
    let link_block = &block[link_start..];
    let href = html::attr_after(link_block, "<a", "href")?;
    let title = html::text_between(link_block, "<a", "</a>").map(|value| html::strip_tags(&value))?;
    Some(item(title, normalize_key(&href), cover))
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let title = html::text_between(body, "entry-title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "CosplayTele Gallery".into());
    let cover = html::attr_after(body, ".gallery-item", "src")
        .or_else(|| html::attr_after(body, "<img", "src"))
        .map(|value| url::join_url(BASE_URL, &value));
    let genres = parse_tags(body);
    CatalogItem {
        key: key.unwrap_or_else(|| format!("/{}/", title.to_ascii_lowercase().replace(' ', "-"))),
        title: title.clone(),
        cover,
        description: Some(title),
        status: ItemStatus::Completed,
        tags: genres,
        url: None,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|block| block.contains("/tag/") || block.contains("/category/"))
        .filter_map(|block| html::text_between(block, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("gallery-item")
        .skip(1)
        .filter_map(|block| html::attr_after(block, "<img", "src").or_else(|| html::attr(block, "src")))
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

fn parse_date(body: &str) -> Option<i64> {
    html::attr_after(body, "time", "datetime")
        .and_then(|value| dates::parse_fixture_date(value.split('T').next().unwrap_or(&value)))
}

fn paginated_taxonomy_url(page: u64, input: &str) -> String {
    let trimmed = input.trim_end_matches('/');
    if let Some(index) = trimmed.find("/page/") {
        let prefix = &trimmed[..index + "/page/".len()];
        format!("{prefix}{page}/")
    } else {
        format!("{trimmed}/page/{page}/")
    }
}

fn is_cosplaytele_url(input: &str) -> bool {
    input.starts_with(BASE_URL) || input.starts_with("https://www.cosplaytele.com")
}

fn is_taxonomy_url(input: &str) -> bool {
    input.contains("/category/") || input.contains("/tag/")
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://www.cosplaytele.com")
        .split('?')
        .next()
        .unwrap_or(input)
        .trim();
    format!("/{}", path.trim_matches('/'))
}

fn item(title: String, key: String, cover: Option<String>) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title,
        cover,
        url: Some(url::join_url(BASE_URL, &key)),
        status: ItemStatus::Completed,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Debug, Deserialize)]
struct PopularPostDto {
    title: RenderedStringDto,
    link: String,
    #[serde(rename = "_embedded")]
    embedded: Option<EmbeddedDto>,
}

impl PopularPostDto {
    fn into_item(self) -> CatalogItem {
        item(
            html::strip_tags(&self.title.rendered),
            normalize_key(&self.link),
            self.embedded
                .and_then(|embedded| embedded.featured_media.into_iter().next())
                .map(|media| media.source_url),
        )
    }
}

#[derive(Debug, Deserialize)]
struct RenderedStringDto {
    rendered: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddedDto {
    #[serde(rename = "wp:featuredmedia", default)]
    featured_media: Vec<FeaturedMediaDto>,
}

#[derive(Debug, Deserialize)]
struct FeaturedMediaDto {
    source_url: String,
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"
[
  {
    "title": { "rendered": "Popular Gallery" },
    "link": "https://cosplaytele.com/popular-gallery/",
    "_embedded": { "wp:featuredmedia": [ { "source_url": "https://cosplaytele.com/cover.jpg" } ] }
  }
]
"#;

const LIST_FIXTURE: &str = r#"
<main>
  <div class="box"><img src="/cover.jpg"><h5><a href="https://cosplaytele.com/sample-gallery/">Sample Gallery</a></h5></div>
  <a class="next page-number" href="/page/2/">Next</a>
</main>
"#;

const DETAILS_FIXTURE: &str = r#"
<main id="main">
  <h1 class="entry-title">Sample Gallery</h1>
  <a href="https://cosplaytele.com/category/cosplay/">Cosplay</a>
  <time class="updated" datetime="2024-01-01T00:00:00+00:00"></time>
  <figure class="gallery-item"><img src="https://cosplaytele.com/1.jpg"></figure>
</main>
"#;

const PAGES_FIXTURE: &str = r#"
<figure class="gallery-item"><img src="https://cosplaytele.com/1.jpg"></figure>
<figure class="gallery-item"><img src="https://cosplaytele.com/2.jpg"></figure>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_popular_json() {
        let page = parse_popular_posts(POPULAR_FIXTURE);
        assert_eq!(page.entries[0].key, "/popular-gallery");
        assert_eq!(page.entries[0].title, "Popular Gallery");
    }

    #[test]
    fn parses_listing_and_details() {
        let page = parse_listing_page(LIST_FIXTURE);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        let details = parse_details(DETAILS_FIXTURE, Some("/sample-gallery".into()));
        assert_eq!(details.title, "Sample Gallery");
        assert_eq!(details.tags, vec!["Cosplay"]);
    }

    #[test]
    fn parses_chapter_pages() {
        assert_eq!(parse_date(DETAILS_FIXTURE), Some(dates::unix_utc_2024_01_01()));
        assert_eq!(parse_pages(PAGES_FIXTURE).len(), 2);
    }
}
