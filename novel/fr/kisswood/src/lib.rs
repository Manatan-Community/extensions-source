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
    id: "kisswood",
    name: "KissWood",
    lang: "fr",
    base_url: "https://kisswood.eu",
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
        let key =
            novel_sites::request_key(&request, "novel").unwrap_or_else(|| "sommaire".to_string());
        Ok(details_for_key(&novel_sites::key(SITE, &key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel_sites::request_key(&request, "novel").unwrap_or_else(|| "sommaire".to_string());
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
        let content =
            html::text_between(&body, "entry-content", "</div>").unwrap_or_else(|| body.clone());
        let elements = content.split("<hr").collect::<Vec<_>>();
        let chapter = if elements.len() > 2 {
            elements[1..elements.len() - 1].join("<hr")
        } else {
            content
                .split("https://fr.tipeee.com/kisswood/")
                .next()
                .unwrap_or_default()
                .split(">Sommaire</a>")
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let title =
            html::text_between(&body, "entry-title", "</h1>").map(|value| html::strip_tags(&value));
        Ok(novel_sites::text_from_html(SITE, &key, title, chapter))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Sommaires".to_string(),
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
    let body = novel_sites::fetch_document(SITE, SITE.base_url, LIST_FIXTURE);
    body.match_indices("Sommaire")
        .filter_map(|(index, _)| {
            let start = body[..index].rfind("<li").unwrap_or(0);
            let end = body[index..]
                .find("</li>")
                .map(|offset| index + offset + 5)
                .unwrap_or(body.len());
            let block = &body[start..end];
            let href = html::attr_after(block, "<a", "href")?;
            let title = title_before(&body[..start]).unwrap_or_else(|| {
                url::slug_from_url(&href).unwrap_or_else(|| SITE.name.to_string())
            });
            let cover = cover_from_info(&href);
            Some(novel_sites::catalog_item(
                SITE,
                novel_sites::key(SITE, &href),
                title,
                cover,
                false,
            ))
        })
        .collect()
}

fn details_for_key(key: &str) -> CatalogItem {
    let body = fetch_key(key, DETAILS_FIXTURE);
    let title = html::text_between(&body, "entry-title", "</h1>")
        .or_else(|| html::text_between(&body, "<title", "</title>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SITE.name.to_string()));
    let text = html::strip_tags(
        &html::text_between(&body, "entry-content", "</div>").unwrap_or_else(|| body.clone()),
    );
    let mut item = novel_sites::catalog_item(SITE, key.to_string(), title, find_cover(&body), true);
    item.description = Some(summary_from_text(&text));
    item.authors = ["Auteur :", "Auteur\u{00a0}:"]
        .iter()
        .find_map(|label| novel_sites::text_after_label(&text, label))
        .into_iter()
        .collect();
    item.status = ItemStatus::Ongoing;
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let content =
        html::text_between(body, "entry-content", "</div>").unwrap_or_else(|| body.to_string());
    let mut seen = BTreeSet::new();
    content
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?.replace("http://", "https://");
            if !href.contains(SITE.base_url)
                || href.contains("share=facebook")
                || href.contains("share=x")
                || href.contains("/category/traductions/")
                || href.contains("/category/tour-des-mondes/")
            {
                return None;
            }
            let key = novel_sites::key(SITE, &href);
            if !seen.insert(key.clone()) {
                return None;
            }
            let title = html::text_between(chunk, ">", "</a>")
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

fn title_before(prefix: &str) -> Option<String> {
    let start = prefix.rfind("<a")?;
    html::text_between(&prefix[start..], "<a", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "Sommaire")
}

fn cover_from_info(href: &str) -> Option<String> {
    let body = novel_sites::fetch_document(SITE, href, DETAILS_FIXTURE);
    find_cover(&body)
}

fn find_cover(body: &str) -> Option<String> {
    html::attr_after(body, "<div", "src")
        .or_else(|| html::attr_after(body, "<figure", "src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

fn summary_from_text(text: &str) -> String {
    let markers = [
        "Traducteur Anglais- Fran\u{00e7}ais",
        "Titre en fran\u{00e7}ais",
        "Titre :",
        "Lien vers le premier chapitre",
        "Auteur : ",
    ];
    let mut summary = text.to_string();
    for marker in markers {
        if let Some(index) = summary.find(marker) {
            summary.truncate(index);
        }
    }
    summary.replace("Synopsis :", "").trim().to_string()
}

fn fetch_key(key: &str, fixture: &str) -> String {
    novel_sites::fetch_document(SITE, &novel_sites::absolute_url(SITE, key), fixture)
}

const LIST_FIXTURE: &str = r#"<nav><ul><li><a href="https://kisswood.eu/info/sample">Sample Novel</a><ul><li><a href="https://kisswood.eu/sommaire/sample">Sommaire</a></li></ul></li></ul></nav>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="entry-title">Sample Novel</h1><div class="entry-content"><p><img src="/cover.jpg"></p><p>Synopsis : Sample summary</p><p>Auteur : Sample Author</p><ul><li><a href="https://kisswood.eu/2024/01/01/chapter-1">Chapter 1</a></li></ul></div>"#;
const TEXT_FIXTURE: &str = r#"<h1 class="entry-title">Chapter 1</h1><div class="entry-content"><hr><p>Sample chapter text.</p><hr></div>"#;

export_novel_source!(SOURCE);
