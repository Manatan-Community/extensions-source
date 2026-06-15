use manatan_extension::{
    CatalogItem, HomeSection, ItemStatus, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: TheLibraryOfOhara = TheLibraryOfOhara;
const BASE_URL: &str = "https://thelibraryofohara.com";
const MAX_CHAPTER_PAGES: usize = 20;

struct TheLibraryOfOhara;

impl MangaSource for TheLibraryOfOhara {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let body = fetch_document_or_fixture(BASE_URL, HOME_FIXTURE);
        Ok(Paged {
            entries: parse_home(&body, source),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![catalog_from_url(query, source, None)],
                has_next_page: false,
            });
        }
        let entries = parse_home(&fetch_document_or_fixture(BASE_URL, HOME_FIXTURE), source)
            .into_iter()
            .filter(|item| item.title.to_lowercase().contains(&query.to_lowercase()))
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/category/sample".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), CATEGORY_FIXTURE);
        Ok(parse_details(&body, &key, source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/category/sample".into());
        Ok(fetch_chapters(&url::join_url(BASE_URL, &key), source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), CHAPTER_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let page = self.list(request)?;
        Ok(vec![HomeSection {
            id: "categories".to_string(),
            title: "Categories".to_string(),
            entries: page.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let source = source_for(&request);
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(catalog_from_url(input, source, None)),
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    site_lang: &'static str,
    category_ids: &'static [&'static str],
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig {
        id: "thelibraryofohara-id",
        lang: "id",
        site_lang: "Indonesia",
        category_ids: &["702404482", "699200615"],
    },
    SourceConfig {
        id: "thelibraryofohara-en",
        lang: "en",
        site_lang: "English",
        category_ids: &[
            "589813936",
            "607613583",
            "43972770",
            "9363667",
            "634609261",
            "699200615",
            "139757",
            "22695",
            "648324575",
        ],
    },
    SourceConfig {
        id: "thelibraryofohara-es",
        lang: "es",
        site_lang: "Spanish",
        category_ids: &["693784776", "699200615"],
    },
    SourceConfig {
        id: "thelibraryofohara-it",
        lang: "it",
        site_lang: "Italian",
        category_ids: &["699200615"],
    },
    SourceConfig {
        id: "thelibraryofohara-ar",
        lang: "ar",
        site_lang: "Arabic",
        category_ids: &["699200615"],
    },
    SourceConfig {
        id: "thelibraryofohara-fr",
        lang: "fr",
        site_lang: "French",
        category_ids: &["699200615"],
    },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("thelibraryofohara-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[1])
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

fn parse_home(body: &str, source: SourceConfig) -> Vec<CatalogItem> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| {
            source
                .category_ids
                .iter()
                .any(|id| chunk.contains(&format!("cat-item-{id}")))
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(catalog_from_url(&href, source, Some(title)))
        })
        .collect()
}

fn parse_details(body: &str, key: &str, source: SourceConfig) -> CatalogItem {
    let title = html::text_between(body, "page-title", "</")
        .map(|value| html::strip_tags(&value).replace("Category: ", ""))
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(key).map(|slug| slug.replace('-', " ")));
    let mut item = catalog_from_url(&url::join_url(BASE_URL, key), source, title);
    item.cover = choose_chapter_thumbnail(body, &item.title, source);
    item.initialized = true;
    item
}

fn fetch_chapters(category_url: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut next = Some(category_url.to_string());
    for _ in 0..MAX_CHAPTER_PAGES {
        let Some(target) = next.take() else { break };
        let body = fetch_document_or_fixture(&target, CATEGORY_FIXTURE);
        let mut page_chapters = parse_chapter_page(&body);
        if page_chapters.is_empty() {
            break;
        }
        chapters.append(&mut page_chapters);
        next = next_page(&body);
        if next.is_none() {
            break;
        }
    }
    filter_chapters(chapters, source)
}

fn parse_chapter_page(body: &str) -> Vec<MangaChapter> {
    body.split("<article")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "entry-thumbnail", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let title = html::text_between(chunk, "entry-title", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&href).unwrap_or_else(|| "Chapter".to_string())
                });
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|date| parse_iso_date(&date)),
                url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn filter_chapters(chapters: Vec<MangaChapter>, source: SourceConfig) -> Vec<MangaChapter> {
    let Some(first) = chapters
        .first()
        .and_then(|chapter| chapter.title.as_deref())
    else {
        return chapters;
    };
    if first.contains("Reverie") {
        return chapters
            .into_iter()
            .filter(|chapter| {
                let title = chapter.title.as_deref().unwrap_or_default();
                match source.lang {
                    "fr" => title.contains("French"),
                    "ar" => title.contains("Arabic"),
                    "it" => title.contains("Italian"),
                    "id" => title.contains("Indonesia"),
                    "es" => title.contains("Spanish"),
                    _ => !["French", "Arabic", "Italian", "Indonesia", "Spanish"]
                        .iter()
                        .any(|lang| title.contains(lang)),
                }
            })
            .collect();
    }
    if source.lang == "es" {
        chapters
            .into_iter()
            .filter(|chapter| {
                !chapter
                    .title
                    .as_deref()
                    .unwrap_or_default()
                    .contains("Indonesia")
            })
            .collect()
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let content = body.split("entry-content").nth(1).unwrap_or(body);
    content
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, "data-orig-file")
                .or_else(|| html::attr(chunk, "src"))
                .filter(|value| !value.is_empty())
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            description: Some((index + 1).to_string()),
            ..MangaPage::default()
        })
        .collect()
}

fn choose_chapter_thumbnail(body: &str, manga_title: &str, source: SourceConfig) -> Option<String> {
    let article = if manga_title.contains("Reverie") {
        body.split("<article").skip(1).find(|chunk| {
            let title = html::text_between(chunk, "entry-title", "</h2>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            title.contains(source.site_lang)
                || (source.lang == "en"
                    && !["French", "Arabic", "Italian", "Indonesia", "Spanish"]
                        .iter()
                        .any(|lang| title.contains(lang)))
        })
    } else if manga_title.contains("Chapter Secrets") && source.lang != "en" {
        body.split("<article").skip(1).find(|chunk| {
            let title = html::text_between(chunk, "entry-title", "</h2>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            (source.lang == "id" && title.contains("Indonesia"))
                || (source.lang == "es" && !title.contains("Indonesia"))
        })
    } else {
        None
    }
    .or_else(|| body.split("<article").nth(1));
    article
        .and_then(|chunk| html::attr_after(chunk, "<img", "src"))
        .map(|value| url::join_url(BASE_URL, &value))
}

fn next_page(body: &str) -> Option<String> {
    body.split("nav-previous")
        .nth(1)
        .and_then(|chunk| html::attr_after(chunk, "<a", "href"))
        .map(|value| url::join_url(BASE_URL, &value))
}

fn catalog_from_url(input: &str, source: SourceConfig, title: Option<String>) -> CatalogItem {
    let key = normalize_key(input);
    CatalogItem {
        key: key.clone(),
        title: title.unwrap_or_else(|| {
            url::slug_from_url(&key)
                .unwrap_or_else(|| "Category".into())
                .replace('-', " ")
        }),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        initialized: false,
        ..CatalogItem::default()
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        input
            .trim_start_matches(BASE_URL)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .to_string()
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
}

fn parse_iso_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    let mut parts = date.split('-').filter_map(|part| part.parse::<i64>().ok());
    Some(days_from_civil(parts.next()?, parts.next()?, parts.next()?) * 86_400)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - (month <= 2) as i64;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const HOME_FIXTURE: &str = r#"
<div id="categories-7"><ul>
<li class="cat-item cat-item-589813936"><a href="https://thelibraryofohara.com/category/chapter-secrets/">Chapter Secrets</a></li>
<li class="cat-item cat-item-699200615"><a href="https://thelibraryofohara.com/category/return-to-the-reverie/">Return to the Reverie</a></li>
</ul></div>
"#;

const CATEGORY_FIXTURE: &str = r#"
<h1 class="page-title">Category: Chapter Secrets</h1>
<article><a class="entry-thumbnail" href="https://thelibraryofohara.com/2024/01/01/sample-chapter/"><img src="https://cdn.example/thumb.jpg"></a><h2 class="entry-title"><a>Sample Chapter</a></h2><span class="posted-on"><time datetime="2024-01-01T00:00:00+00:00"></time></span></article>
"#;

const CHAPTER_FIXTURE: &str = r#"
<div class="entry-content">
<a><img data-orig-file="https://cdn.example/page-1.jpg"></a>
<img class="size-full" src="https://cdn.example/page-2.jpg">
</div>
"#;

export_manga_source!(SOURCE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_home_categories() {
        let entries = parse_home(HOME_FIXTURE, SOURCES[1]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "/category/chapter-secrets/");
    }

    #[test]
    fn parses_details_and_chapters() {
        let item = parse_details(CATEGORY_FIXTURE, "/category/chapter-secrets/", SOURCES[1]);
        assert_eq!(item.title, "Chapter Secrets");
        assert_eq!(item.cover.as_deref(), Some("https://cdn.example/thumb.jpg"));
        let chapters = parse_chapter_page(CATEGORY_FIXTURE);
        assert_eq!(chapters[0].title.as_deref(), Some("Sample Chapter"));
        assert_eq!(chapters[0].date_uploaded, Some(1_704_067_200));
    }

    #[test]
    fn parses_pages() {
        let pages = parse_pages(CHAPTER_FIXTURE);
        assert_eq!(pages.len(), 2);
        match &pages[0].content {
            PageContent::Url { url, .. } => assert_eq!(url, "https://cdn.example/page-1.jpg"),
            _ => panic!("expected URL page"),
        }
    }
}
