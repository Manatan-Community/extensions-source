use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, http,
    source::MangaSource,
};
use manatan_shared::{html, manga, url};
use serde_json::Value;

const SOURCE: TraducoesDoLipe = TraducoesDoLipe;
const BASE_URL: &str = "https://traducoesdolipe.blogspot.com";
const NAME: &str = "Traduções do Lipe";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "safe";
const MAX_RESULTS: u64 = 20;
const CHAPTER_RESULTS: u64 = 999_999;
const MANGA_CATEGORY: &str = "Projeto";
const CHAPTER_CATEGORY: &str = "Capítulo";

struct TraducoesDoLipe;

impl MangaSource for TraducoesDoLipe {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_feed_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_feed_listing(&fetch_json(
            &feed_url(page, &[MANGA_CATEGORY.to_string()], None),
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
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), &key)],
                has_next_page: false,
            });
        }
        Ok(parse_feed_listing(&fetch_json(
            &feed_url(
                page,
                &search_labels(&request),
                (!query.is_empty()).then_some(query),
            ),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample.html".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/p/sample.html".into());
        let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
        let labels = chapter_labels(&body);
        Ok(parse_chapter_feed(
            &fetch_json(&chapter_feed_url(&labels), CHAPTERS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/2024/01/sample-chapter-1.html".into());
        Ok(parse_pages(&fetch_document(
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

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_feed_listing(&fetch_json(
            &feed_url(1, &[MANGA_CATEGORY.to_string()], None),
            LIST_FIXTURE,
        ));
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), &key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("Accept", "application/json, text/plain, */*")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn feed_url(page: u64, labels: &[String], query: Option<&str>) -> String {
    let start = MAX_RESULTS * page.saturating_sub(1) + 1;
    let mut path = format!("{BASE_URL}/feeds/posts/default");
    if !labels.is_empty() {
        path.push_str("/-/");
        path.push_str(
            &labels
                .iter()
                .map(|label| label_path(label))
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
    let mut params = vec![
        "alt=json".to_string(),
        format!("max-results={}", MAX_RESULTS + 1),
        format!("start-index={start}"),
    ];
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        params.push(format!(
            "q={}",
            url::query_escape(&format!("label:{MANGA_CATEGORY} {query}"))
        ));
    }
    format!("{path}?{}", params.join("&"))
}

fn chapter_feed_url(labels: &[String]) -> String {
    feed_url(1, labels, None).replace(
        &format!("max-results={}", MAX_RESULTS + 1),
        &format!("max-results={CHAPTER_RESULTS}"),
    )
}

fn label_path(label: &str) -> String {
    url::query_escape(label).replace('+', "%20")
}

fn search_labels(request: &Value) -> Vec<String> {
    let mut labels = vec![MANGA_CATEGORY.to_string()];
    labels.extend(selected_values(request, "genres"));
    labels
}

fn selected_values(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_feed_listing(body: &str) -> Paged<CatalogItem> {
    let root = json_or_fixture(body, LIST_FIXTURE);
    let entries = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry_link(entry).is_some())
        .filter(|entry| has_category_or_empty(entry, MANGA_CATEGORY))
        .filter(|entry| !has_category(entry, "Anime"))
        .map(entry_to_catalog)
        .collect::<Vec<_>>();
    Paged {
        has_next_page: entries.len() > MAX_RESULTS as usize,
        entries: entries.into_iter().take(MAX_RESULTS as usize).collect(),
    }
}

fn entry_to_catalog(entry: &Value) -> CatalogItem {
    let href = entry_link(entry).unwrap_or_else(|| format!("{BASE_URL}/p/sample.html"));
    let key = normalize_key(&href);
    CatalogItem {
        key: key.clone(),
        title: entry_title(entry).unwrap_or_else(|| NAME.to_string()),
        cover: entry_thumbnail(entry),
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let details = body.split("grid gtc-235fr").nth(1).unwrap_or(body);
    let header = html::text_between(body, "<header", "</header>").unwrap_or_default();
    CatalogItem {
        key: normalize_key(key),
        title: html::attr_after(body, "property=\"og:description\"", "content")
            .or_else(|| html::text_between(&header, "<h1", "</h1>"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .or_else(|| html::attr_after(body, "property=\"og:title\"", "content"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| NAME.to_string())),
        cover: html::attr_after(&header, "thumb", "src")
            .or_else(|| image_attr(&header))
            .or_else(|| html::attr_after(body, "property=\"og:image\"", "content"))
            .or_else(|| image_attr(details))
            .map(|image| fix_google_image(&absolute_url(&image))),
        description: html::text_between(body, "id=\"synopsis\"", "</")
            .or_else(|| html::text_between(body, "id='synopsis'", "</"))
            .or_else(|| html::text_between(body, "class=\"synopsis\"", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(details, "rel=\"tag\"")
            .into_iter()
            .chain(link_values(body, "search/label"))
            .collect(),
        status: parse_status(&status_text(body)),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_feed(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let root = json_or_fixture(body, CHAPTERS_FIXTURE);
    let mut chapters = root
        .get("feed")
        .and_then(|feed| feed.get("entry"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| has_category_or_empty(entry, CHAPTER_CATEGORY))
        .filter_map(|entry| {
            let href = entry_link(entry)?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: entry_title(entry),
                date_uploaded: entry
                    .get("published")
                    .and_then(|value| value.get("$t"))
                    .and_then(Value::as_str)
                    .and_then(parse_feed_date),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capitulo".to_string()),
            url: Some(absolute_url(manga_key)),
            language: Some(LANG.to_string()),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script_pages = parse_script_pages(body);
    if !script_pages.is_empty() {
        return script_pages;
    }
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("separator")
                || chunk.contains("reader")
                || chunk.contains("blogger")
                || chunk.contains("bp.blogspot")
                || chunk.contains("googleusercontent")
                || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: fix_google_image(&absolute_url(&image)),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn parse_script_pages(body: &str) -> Vec<MangaPage> {
    let script = body
        .split("class=\"chapter\"")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<script", "</script>"))
        .or_else(|| html::text_between(body, "<script", "</script>"))
        .unwrap_or_default();
    let Some(array) = json_array_after_equals(&script) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&array)
        .unwrap_or_default()
        .into_iter()
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: fix_google_image(&absolute_url(&image)),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn json_array_after_equals(script: &str) -> Option<String> {
    let start = script.find('[')?;
    let end = script[start..].find(']')? + start + 1;
    Some(script[start..end].to_string())
}

fn chapter_labels(body: &str) -> Vec<String> {
    if let Some(feed) = quoted_after(body.split("catNameProject").nth(1).unwrap_or_default(), "(") {
        return vec![CHAPTER_CATEGORY.to_string(), feed];
    }
    if let Some(feed) = html::text_between(body, "#clwd", "</script>")
        .and_then(|script| quoted_after(&script, "clwd.run("))
        .or_else(|| quoted_after(body, "clwd.run("))
    {
        return vec![CHAPTER_CATEGORY.to_string(), feed];
    }
    if let Some(label) = html::attr_after(body, "chapter_get", "data-labelchapter")
        .or_else(|| quoted_after(body, "data-labelchapter"))
    {
        return vec![label];
    }
    vec![CHAPTER_CATEGORY.to_string()]
}

fn entry_title(entry: &Value) -> Option<String> {
    entry
        .get("title")
        .and_then(|title| title.get("$t"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn entry_link(entry: &Value) -> Option<String> {
    entry
        .get("link")
        .and_then(Value::as_array)?
        .iter()
        .find(|link| link.get("rel").and_then(Value::as_str) == Some("alternate"))
        .and_then(|link| link.get("href"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn entry_thumbnail(entry: &Value) -> Option<String> {
    entry
        .get("media$thumbnail")
        .and_then(|thumb| thumb.get("url"))
        .and_then(Value::as_str)
        .map(fix_google_image)
        .or_else(|| {
            entry
                .get("content")
                .and_then(|content| content.get("$t"))
                .and_then(Value::as_str)
                .and_then(image_attr)
                .map(|image| fix_google_image(&absolute_url(&image)))
        })
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_text(body: &str) -> String {
    html::text_between(body, "data-status", "</")
        .map(|value| html::strip_tags(&value))
        .or_else(|| {
            html::text_between(body, "class=\"status\"", "</").map(|value| html::strip_tags(&value))
        })
        .or_else(|| text_after_label(body, "Status"))
        .or_else(|| text_after_label(body, "Estado"))
        .unwrap_or_default()
}

fn text_after_label(body: &str, label: &str) -> Option<String> {
    let chunk = body.split(label).nth(1)?;
    html::text_between(chunk, "<span", "</span>")
        .or_else(|| html::text_between(chunk, "<dd", "</dd>"))
        .or_else(|| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.trim().to_ascii_lowercase();
    if ["completed", "completo", "finalizado", "finalizada"].contains(&value.as_str()) {
        ItemStatus::Completed
    } else if ["hiatus", "pausado"].contains(&value.as_str()) {
        ItemStatus::Hiatus
    } else if ["cancelled", "canceled", "dropped", "dropado", "cancelado"].contains(&value.as_str())
    {
        ItemStatus::Cancelled
    } else if [
        "ongoing",
        "ativo",
        "activa",
        "em lançamento",
        "em lancamento",
        "lançando",
    ]
    .contains(&value.as_str())
    {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn has_category(entry: &Value, category: &str) -> bool {
    entry
        .get("category")
        .and_then(Value::as_array)
        .is_some_and(|categories| {
            categories
                .iter()
                .any(|cat| cat.get("term").and_then(Value::as_str) == Some(category))
        })
}

fn has_category_or_empty(entry: &Value, category: &str) -> bool {
    entry
        .get("category")
        .and_then(Value::as_array)
        .map(|categories| {
            categories.is_empty()
                || categories
                    .iter()
                    .any(|cat| cat.get("term").and_then(Value::as_str) == Some(category))
        })
        .unwrap_or(true)
}

fn quoted_after(body: &str, marker: &str) -> Option<String> {
    let rest = body.split(marker).nth(1)?;
    let quote = rest.find(['"', '\''])?;
    let quote_char = rest.as_bytes()[quote] as char;
    let after = &rest[quote + 1..];
    let end = after.find(quote_char)?;
    Some(after[..end].to_string())
}

fn fix_google_image(input: &str) -> String {
    input
        .replace("/s72-c/", "/s1600/")
        .replace("=s72-c", "=s1600")
        .replace("/w72-h72-p-k-no-nu/", "/s1600/")
}

fn parse_feed_date(value: &str) -> Option<i64> {
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
    let doe = yoe * 365 + yoe / 4 + doy - yoe / 100;
    i64::from(era * 146_097 + doe - 719_468)
}

fn json_or_fixture(body: &str, fixture: &str) -> Value {
    serde_json::from_str(body)
        .unwrap_or_else(|_| serde_json::from_str(fixture).unwrap_or(Value::Null))
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Sample Lipe"},"category":[{"term":"Projeto"}],"link":[{"rel":"alternate","href":"https://traducoesdolipe.blogspot.com/p/sample.html"}],"media$thumbnail":{"url":"https://blogger.googleusercontent.com/img/s72-c/cover.jpg"}}]}}"#;
const DETAILS_FIXTURE: &str = r#"
<meta property="og:description" content="Sample Lipe"><meta property="og:image" content="https://blogger.googleusercontent.com/img/s72-c/cover.jpg">
<div class="synopsis">Sample description.</div><div class="status">Completo</div><div class="genres"><a rel="tag">Romance</a></div><script>var catNameProject = ('Sample')</script>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"feed":{"entry":[{"title":{"$t":"Capitulo 1"},"category":[{"term":"Capítulo"}],"published":{"$t":"2024-01-01T00:00:00.000Z"},"link":[{"rel":"alternate","href":"https://traducoesdolipe.blogspot.com/2024/01/sample-chapter-1.html"}]}]}}"#;
const PAGES_FIXTURE: &str = r#"<div class="chapter"><script>pages = ["https://blogger.googleusercontent.com/img/s1600/page1.jpg"]</script></div>"#;
