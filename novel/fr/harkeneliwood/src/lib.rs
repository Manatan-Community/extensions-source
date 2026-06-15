use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{
    html,
    novel_sites::{self, NovelSite},
    url,
};
use serde_json::Value;

const SITE: NovelSite = NovelSite {
    id: "harkeneliwood",
    name: "HarkenEliwood",
    lang: "fr",
    base_url: "https://harkeneliwood.wordpress.com",
    popular_path: "projets/",
    latest_path: "projets/",
    search_path: "?s={query}&page={page}",
    content_rating: "safe",
};

const SOURCE: Source = Source;

struct Source;

impl NovelSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let entries = if page > 1 { Vec::new() } else { popular() };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(SITE.base_url) {
            let key = novel_sites::key(SITE, query);
            return Ok(Paged {
                entries: vec![details_for_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(Paged {
            entries: if page == 1 {
                novel_sites::filter_local_catalog(popular(), query)
            } else {
                Vec::new()
            },
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "projets/sample".to_string());
        Ok(details_for_key(&novel_sites::key(SITE, &key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "projets/sample".to_string());
        let body = fetch_key(&novel_sites::key(SITE, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel_sites::request_key(&request, "chapter")
            .unwrap_or_else(|| "2024/01/01/chapter-1".to_string());
        let key = novel_sites::key(SITE, &key);
        let body = fetch_key(&key, TEXT_FIXTURE);
        let title = html::text_between(&body, "h1.entry-title", "</h1>")
            .or_else(|| html::text_between(&body, "entry-title", "</h1>"))
            .map(|value| html::strip_tags(&value));
        let chapter = html::text_between(&body, "div.entry-content", "</div>")
            .or_else(|| html::text_between(&body, "entry-content", "</div>"))
            .unwrap_or_else(|| body.clone());
        let content = title
            .as_ref()
            .map(|value| format!("<h1>{value}</h1>{chapter}"))
            .unwrap_or(chapter);
        Ok(novel_sites::text_from_html(SITE, &key, title, content))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Projets".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(SITE.base_url) {
            let key = novel_sites::key(SITE, input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(None)
    }
}

fn popular() -> Vec<CatalogItem> {
    let body = novel_sites::fetch_document_deflate(
        SITE,
        &url::join_url(SITE.base_url, "projets/"),
        LIST_FIXTURE,
    );
    let content = body.split("entry-content").nth(1).unwrap_or(&body);
    content
        .split("<a")
        .skip(1)
        .filter(|chunk| !chunk.contains("nofollow noopener noreferrer"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let key = novel_sites::key(SITE, &href);
            Some(novel_sites::catalog_item(SITE, key, title, None, false))
        })
        .collect()
}

fn details_for_key(key: &str) -> CatalogItem {
    let body = fetch_key(key, DETAILS_FIXTURE);
    let text = html::strip_tags(
        &html::text_between(&body, "entry-content", "</div>").unwrap_or_else(|| body.clone()),
    );
    let title = html::text_between(&body, "h1.entry-title", "</h1>")
        .or_else(|| html::text_between(&body, "entry-title", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SITE.name.to_string()));
    let mut item = novel_sites::catalog_item(SITE, key.to_string(), title, cover(&body), true);
    item.description = Some(summary(&text));
    item.authors = novel_sites::text_after_label(&text, "Auteur")
        .into_iter()
        .collect();
    item.status = ItemStatus::Ongoing;
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("entry-content")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains(SITE.base_url) {
                return None;
            }
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let key = novel_sites::key(SITE, &href);
            Some(novel_sites::chapter_item(
                SITE,
                key,
                Some(title),
                novel_sites::chapter_number(&href),
            ))
        })
        .collect()
}

fn summary(text: &str) -> String {
    let mut value = [
        ("Synopsis :", "Traduction anglaise"),
        ("Synopsis :", "Raw :"),
        ("Synopsis :", "Prelude"),
        ("Synopsis :", "Pr\u{00e9}lude"),
        ("Synospis :", "Original "),
    ]
    .iter()
    .find_map(|(start, end)| novel_sites::text_between_markers(text, start, end))
    .unwrap_or_else(|| text.to_string());
    if let Some(trimmed) = novel_sites::text_between_markers(&value, "", "Raw :") {
        value = trimmed;
    }
    value.trim().to_string()
}

fn cover(body: &str) -> Option<String> {
    let content = body.split("entry-content").nth(1).unwrap_or(body);
    html::attr_after(content, "<img", "src")
}

fn fetch_key(key: &str, fixture: &str) -> String {
    novel_sites::fetch_document_deflate(SITE, &novel_sites::absolute_url(SITE, key), fixture)
}

const LIST_FIXTURE: &str = r#"<div class="entry-content"><a href="https://harkeneliwood.wordpress.com/projets/sample">Sample Novel</a></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample Novel</h1><div class="entry-content"><p><img src="/cover.jpg"></p><p>Auteur : Sample Author</p><p>Synopsis : Sample summary Raw :</p><p><a href="https://harkeneliwood.wordpress.com/2024/01/01/chapter-1">Chapter 1</a></p></div>"#;
const TEXT_FIXTURE: &str = r#"<h1 class="entry-title">Chapter 1</h1><div class="entry-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
