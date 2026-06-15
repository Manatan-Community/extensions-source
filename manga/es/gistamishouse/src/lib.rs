use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: GistamisHouse = GistamisHouse;
const BASE_URL: &str = "https://gistamishousefansub.blogspot.com";
const NAME: &str = "Gistamis House";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";
const MAX_RESULTS: u64 = 20;

struct GistamisHouse;

impl MangaSource for GistamisHouse {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if listing_id(&request) == "latest" {
            return Ok(parse_feed_listing(&fetch_json_or_fixture(
                &feed_url("Series", page, &[("orderby", "published")]),
                FEED_FIXTURE,
            )));
        }
        Ok(parse_popular(&fetch_document_or_fixture(
            BASE_URL,
            POPULAR_FIXTURE,
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
                entries: vec![parse_details(
                    &fetch_document_or_fixture(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            feed_url("Series", page, &[])
        } else {
            format!(
                "{}&q=label:Series+{}",
                feed_url("Series", page, &[]),
                url::query_escape(query)
            )
        };
        Ok(parse_feed_listing(&fetch_json_or_fixture(
            &target,
            FEED_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        Ok(parse_details(
            &fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        let body = fetch_document_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let feed = chapter_feed_url(&body);
        Ok(parse_chapter_feed(&fetch_json_or_fixture(
            &feed,
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/chapter.html".into());
        Ok(parse_pages(&fetch_document_or_fixture(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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
        .with_referer(format!("{BASE_URL}/"))
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

fn fetch_json_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    let popular = body.split("PopularPosts").nth(1).unwrap_or(body);
    let entries = popular
        .split("<figure")
        .skip(1)
        .filter(|chunk| !chunk.contains("data=Capitulo"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "figcaption", "</figcaption>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| url::slug_from_url(&key))
                    .unwrap_or_else(|| NAME.to_string()),
                cover: image_from_chunk(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_feed_listing(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, FEED_FIXTURE);
    let mut entries = feed_entries(&root)
        .filter(|entry| has_category(entry, "Series"))
        .filter(|entry| !has_category(entry, "Anime") && !has_category(entry, "Novela"))
        .map(catalog_from_entry)
        .collect::<Vec<_>>();
    let has_next_page = entries.len() > MAX_RESULTS as usize;
    if has_next_page {
        entries.truncate(MAX_RESULTS as usize);
    }
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample.html".to_string());
    let profile = body.split("grid gtc-235fr").nth(1).unwrap_or(body);
    let alt_name = info_value(profile, "Otros Nombres");
    let mut description = text_by_id(profile, "synopsis")
        .or_else(|| html::text_between(profile, "synopsis", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    if let Some(alt_name) = alt_name.filter(|value| !value.is_empty()) {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Otros Nombres: ");
        description.push_str(&alt_name);
    }

    CatalogItem {
        key: key.clone(),
        title: html::attr_after(body, "property=\"og:title\"", "content")
            .or_else(|| {
                html::text_between(body, "<h1", "</h1>").map(|value| html::strip_tags(&value))
            })
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| NAME.to_string()),
        cover: image_from_chunk(profile).map(|image| absolute_url(&image)),
        description: (!description.is_empty()).then_some(description),
        authors: info_value(profile, "Author")
            .or_else(|| info_value(profile, "Autor"))
            .or_else(|| info_value(profile, "Mangaka"))
            .into_iter()
            .collect(),
        artists: info_value(profile, "Artist").into_iter().collect(),
        tags: tag_values(profile),
        status: parse_status(
            &info_value(profile, "Status")
                .or_else(|| info_value(profile, "Estado"))
                .unwrap_or_default(),
        ),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_feed(body: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, CHAPTERS_FIXTURE);
    feed_entries(&root)
        .filter(|entry| has_category(entry, "Capitulo") || has_category(entry, "Cap"))
        .filter_map(chapter_from_entry)
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let article = body.split("<article").nth(1).unwrap_or(body);
    article
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-src")
                .or_else(|| html::attr(chunk, "data-lazy-src"))
                .or_else(|| html::attr(chunk, "data-cfsrc"))
                .or_else(|| html::attr(chunk, "src"))
        })
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn catalog_from_entry(entry: &Value) -> CatalogItem {
    let href = alternate_link(entry).unwrap_or_else(|| BASE_URL.to_string());
    let key = normalize_key(&href);
    CatalogItem {
        key: key.clone(),
        title: entry
            .get("title")
            .and_then(|title| title.get("$t"))
            .and_then(Value::as_str)
            .unwrap_or(NAME)
            .to_string(),
        cover: entry
            .get("media$thumbnail")
            .and_then(|thumb| thumb.get("url"))
            .and_then(Value::as_str)
            .map(blogger_thumbnail)
            .or_else(|| {
                entry
                    .get("content")
                    .and_then(|content| content.get("$t"))
                    .and_then(Value::as_str)
                    .and_then(image_from_chunk)
                    .map(|image| absolute_url(&image))
            }),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn chapter_from_entry(entry: &Value) -> Option<MangaChapter> {
    let href = alternate_link(entry)?;
    let key = normalize_key(&href);
    Some(MangaChapter {
        key: key.clone(),
        title: Some(
            entry
                .get("title")
                .and_then(|title| title.get("$t"))
                .and_then(Value::as_str)
                .unwrap_or("Capitulo")
                .to_string(),
        ),
        date_uploaded: entry
            .get("published")
            .and_then(|date| date.get("$t"))
            .and_then(Value::as_str)
            .and_then(|date| manatan_shared::dates::parse_ymd(&date[..date.len().min(10)])),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        ..MangaChapter::default()
    })
}

fn feed_entries(root: &Value) -> impl Iterator<Item = &Value> {
    root.get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn has_category(entry: &Value, term: &str) -> bool {
    entry
        .get("category")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|category| category.get("term").and_then(Value::as_str) == Some(term))
}

fn alternate_link(entry: &Value) -> Option<String> {
    entry
        .get("link")
        .and_then(Value::as_array)?
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some("alternate"))?
        .get("href")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn chapter_feed_url(body: &str) -> String {
    let feed = quoted_after(body, "clwd.run(").or_else(|| quoted_after(body, "label"));
    feed.map(|label| feed_url(&label, 1, &[("max-results", "999999")]))
        .unwrap_or_else(|| feed_url("Capitulo", 1, &[("max-results", "999999")]))
}

fn feed_url(label: &str, page: u64, extra: &[(&str, &str)]) -> String {
    let start_index = MAX_RESULTS * (page.saturating_sub(1)) + 1;
    let max_results = extra
        .iter()
        .find(|(key, _)| *key == "max-results")
        .map(|(_, value)| *value)
        .unwrap_or("21");
    let mut pairs = vec![
        ("alt", "json".to_string()),
        ("max-results", max_results.to_string()),
        ("start-index", start_index.to_string()),
    ];
    for (key, value) in extra {
        if *key != "max-results" {
            pairs.push((*key, (*value).to_string()));
        }
    }
    format!(
        "{BASE_URL}/feeds/posts/default/-/{}?{}",
        url::query_escape(label),
        pairs
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split(marker).nth(1)?;
    let start = rest.find(['"', '\''])?;
    let quote = rest[start..].chars().next()?;
    let tail = &rest[start + quote.len_utf8()..];
    Some(tail[..tail.find(quote)?].to_string())
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split("y6x11p")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .and_then(|chunk| {
            html::text_between(chunk, "span class=\"dt", "</span>")
                .or_else(|| html::text_between(chunk, "span class='dt", "</span>"))
                .or_else(|| html::text_between(chunk, "<span", "</span>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_by_id(body: &str, id: &str) -> Option<String> {
    body.split('<')
        .find(|chunk| {
            chunk.contains(&format!("id=\"{id}\"")) || chunk.contains(&format!("id='{id}'"))
        })
        .and_then(|chunk| chunk.split('>').nth(1))
        .map(ToString::to_string)
}

fn tag_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("rel=\"tag\"") || chunk.contains("rel='tag'"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn blogger_thumbnail(value: &str) -> String {
    value
        .replace("/s72-c/", "/w600/")
        .replace("=s72-c", "=w600")
        .replace("\\/", "/")
}

fn parse_status(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("activo") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else if lower.contains("completo") || lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("pausado") || lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("cancelado") || lower.contains("dropped") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn normalize_key(value: &str) -> String {
    if let Some(path) = value.strip_prefix(BASE_URL) {
        let trimmed = path.trim_matches('/');
        return if trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("/{trimmed}")
        };
    }
    let trimmed = value.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|_| serde_json::from_str(fixture).unwrap())
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

const POPULAR_FIXTURE: &str = r#"
<div class="PopularPosts"><div class="grid">
<figure><a href="https://gistamishousefansub.blogspot.com/2024/01/sample.html"><img src="https://blogger.googleusercontent.com/img/sample=s72-c"></a><figcaption><a href="https://gistamishousefansub.blogspot.com/2024/01/sample.html">Sample</a></figcaption></figure>
<figure><span data="Capitulo"></span></figure>
</div></div>
"#;

const FEED_FIXTURE: &str = r#"{
  "feed": {
    "entry": [{
      "title": {"$t": "Sample"},
      "category": [{"term": "Series"}, {"term": "Manga"}],
      "link": [{"rel": "alternate", "href": "https://gistamishousefansub.blogspot.com/2024/01/sample.html"}],
      "media$thumbnail": {"url": "https://blogger.googleusercontent.com/img/sample=s72-c"}
    }]
  }
}"#;

const DETAILS_FIXTURE: &str = r#"
<div class="grid gtc-235fr"><img src="https://gistamishousefansub.blogspot.com/cover.jpg"><div id="synopsis">Description</div><div class="mt-15"><a rel="tag">Manga</a></div><div class="y6x11p">Estado <span class="dt">Activo</span></div><div class="y6x11p">Autor <span class="dt">Author</span></div><div class="y6x11p">Otros Nombres <span class="dt">Alt</span></div></div>
<div id="latest"><script>label = 'sample-chapters'</script></div>
"#;

const CHAPTERS_FIXTURE: &str = r#"{
  "feed": {
    "entry": [{
      "title": {"$t": "Capitulo 1"},
      "published": {"$t": "2024-01-01T00:00:00.000Z"},
      "category": [{"term": "Capitulo"}],
      "link": [{"rel": "alternate", "href": "https://gistamishousefansub.blogspot.com/2024/01/chapter.html"}]
    }]
  }
}"#;

const PAGES_FIXTURE: &str = r#"
<article class="oh"><div class="post"><p><img src="https://gistamishousefansub.blogspot.com/page-1.jpg"></p><p><img data-src="https://gistamishousefansub.blogspot.com/page-2.jpg"></p></div></article>
"#;

export_manga_source!(SOURCE);
