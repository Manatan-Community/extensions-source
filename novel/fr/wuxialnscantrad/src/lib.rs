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
use std::collections::BTreeSet;

const SITE: NovelSite = NovelSite {
    id: "wuxialnscantrad",
    name: "WuxiaLnScantrad",
    lang: "fr",
    base_url: "https://wuxialnscantrad.wordpress.com",
    popular_path: "",
    latest_path: "",
    search_path: "?s={query}&page={page}",
    content_rating: "safe",
};

const SOURCE: Source = Source;

struct Source;

impl NovelSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(Paged {
            entries: if page == 1 { popular() } else { Vec::new() },
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
        let title =
            html::text_between(&body, "entry-title", "</h1>").map(|value| html::strip_tags(&value));
        let content = html::text_between(&body, "entry-content", "</div>")
            .unwrap_or_else(|| body.clone())
            .split("<script")
            .next()
            .unwrap_or_default()
            .replace("<hr>", "")
            .replace("<p>&nbsp;</p>", "");
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
    let body = novel_sites::fetch_document_deflate(SITE, SITE.base_url, LIST_FIXTURE);
    let menu = body.split("menu-item-2210").nth(1).unwrap_or(&body);
    menu.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(novel_sites::catalog_item(
                SITE,
                novel_sites::key(SITE, &href),
                title,
                None,
                false,
            ))
        })
        .collect()
}

fn details_for_key(key: &str) -> CatalogItem {
    let body = fetch_key(key, DETAILS_FIXTURE);
    let title = html::text_between(&body, "entry-title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SITE.name.to_string()));
    let content =
        html::text_between(&body, "entry-content", "</div>").unwrap_or_else(|| body.clone());
    let text = html::strip_tags(&content);
    let mut item = novel_sites::catalog_item(SITE, key.to_string(), title, cover(&content), true);
    item.authors = novel_sites::text_after_label(&text, "Auteur(s):")
        .into_iter()
        .collect();
    item.artists = novel_sites::text_between_markers(&text, "Artiste(s):", "Genres")
        .into_iter()
        .collect();
    item.tags = novel_sites::text_after_label(&text, "Genres:")
        .into_iter()
        .collect();
    item.description = [
        ("Synopsis :", "Chapitres disponibles"),
        ("Sypnopsis", "Sypnopsis officiel"),
        ("Synopsis", "Chapitres disponibles"),
    ]
    .iter()
    .find_map(|(start, end)| novel_sites::text_between_markers(&text, start, end));
    item.status = status(&text);
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let content =
        html::text_between(body, "entry-content", "</div>").unwrap_or_else(|| body.to_string());
    let list = html::text_between(&content, "<ul", "</ul>").unwrap_or(content);
    let mut seen = BTreeSet::new();
    list.split("<li")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains(SITE.base_url) {
                return None;
            }
            let key = novel_sites::key(SITE, &href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(novel_sites::chapter_item(
                SITE,
                key,
                Some(title),
                novel_sites::chapter_number(&href),
            ))
        })
        .collect()
}

fn cover(content: &str) -> Option<String> {
    html::attr_after(content, "<strong", "src").or_else(|| html::attr_after(content, "<img", "src"))
}

fn status(text: &str) -> ItemStatus {
    match novel_sites::text_after_label(text, "Statut:")
        .unwrap_or_default()
        .as_str()
    {
        "Arr\u{00ea}t\u{00e9}" => ItemStatus::Cancelled,
        "Termin\u{00e9}" => ItemStatus::Completed,
        _ => ItemStatus::Ongoing,
    }
}

fn fetch_key(key: &str, fixture: &str) -> String {
    novel_sites::fetch_document_deflate(SITE, &novel_sites::absolute_url(SITE, key), fixture)
}

const LIST_FIXTURE: &str = r#"<li id="menu-item-2210"><ul><li><a href="https://wuxialnscantrad.wordpress.com/projets/sample">Sample Novel</a></li></ul></li>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample Novel</h1><div class="entry-content"><p><img src="/cover.jpg"></p><p>Auteur(s): Sample Author</p><p>Genres: Action</p><p>Statut: En cours</p><p>Synopsis : Sample summary Chapitres disponibles</p><ul><li><a href="https://wuxialnscantrad.wordpress.com/2024/01/01/chapter-1">Chapter 1</a></li></ul></div>"#;
const TEXT_FIXTURE: &str = r#"<h1 class="entry-title">Chapter 1</h1><div class="entry-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
