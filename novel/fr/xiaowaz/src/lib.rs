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
    id: "xiaowaz",
    name: "Xiaowaz",
    lang: "fr",
    base_url: "https://xiaowaz.fr",
    popular_path: "",
    latest_path: "",
    search_path: "?s={query}&page={page}",
    content_rating: "safe",
};
const PAGE_SIZE: usize = 5;

const SOURCE: Source = Source;

struct Source;

impl NovelSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let all = all_novels();
        let start = PAGE_SIZE.saturating_mul(page.saturating_sub(1));
        let entries = all
            .into_iter()
            .skip(start)
            .take(PAGE_SIZE)
            .map(with_cover)
            .collect::<Vec<_>>();
        Ok(Paged {
            has_next_page: start + PAGE_SIZE < all_novels().len(),
            entries,
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
                novel_sites::filter_local_catalog(all_novels(), query)
                    .into_iter()
                    .map(with_cover)
                    .collect()
            } else {
                Vec::new()
            },
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "series/sample".to_string());
        Ok(details_for_key(&novel_sites::key(SITE, &key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "series/sample".to_string());
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
            .unwrap_or_else(|| "articles/chapter-1".to_string());
        let key = novel_sites::key(SITE, &key);
        let body = fetch_key(&key, TEXT_FIXTURE);
        let content = chapter_window(&body);
        let title =
            html::text_between(&body, "entry-title", "</h1>").map(|value| html::strip_tags(&value));
        Ok(novel_sites::text_from_html(SITE, &key, title, content))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Series".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
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

fn all_novels() -> Vec<CatalogItem> {
    let body = novel_sites::fetch_document(SITE, SITE.base_url, LIST_FIXTURE);
    let mut entries = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        if !href.contains(SITE.base_url)
            || !(href.contains("/series")
                || href.contains("/oeuvres-originales")
                || href.contains("/series-abandonnees"))
        {
            continue;
        }
        let title = html::text_between(chunk, ">", "</a>")
            .map(|value| {
                html::strip_tags(&value)
                    .replace('\u{2714}', "")
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty());
        let Some(title) = title else {
            continue;
        };
        if title == "Douluo Dalu" {
            continue;
        }
        entries.push(novel_sites::catalog_item(
            SITE,
            novel_sites::key(SITE, &href),
            title,
            None,
            false,
        ));
    }
    entries
}

fn with_cover(mut item: CatalogItem) -> CatalogItem {
    let body = fetch_key(&item.key, DETAILS_FIXTURE);
    item.cover = find_cover(&body).map(|cover| novel_sites::absolute_url(SITE, &cover));
    item
}

fn details_for_key(key: &str) -> CatalogItem {
    let body = fetch_key(key, DETAILS_FIXTURE);
    let mut title = html::text_between(&body, "card_title", "</")
        .or_else(|| html::text_between(&body, "entry-title", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SITE.name.to_string()));
    let mut status = ItemStatus::Ongoing;
    if title.ends_with('\u{2714}') {
        title.pop();
        title = title.trim().to_string();
        status = ItemStatus::Completed;
    } else if key.starts_with("series-abandonnees") {
        status = ItemStatus::Cancelled;
    }
    let text = html::strip_tags(
        &html::text_between(&body, "entry-content", "</div>").unwrap_or_else(|| body.clone()),
    );
    let mut item = novel_sites::catalog_item(SITE, key.to_string(), title, find_cover(&body), true);
    item.status = status;
    item.authors = author(&text).into_iter().collect();
    item.tags = genre(&text).into_iter().collect();
    if key.starts_with("oeuvres-originales") {
        item.tags.push("Oeuvre originale".to_string());
    }
    item.description = Some(summary(&body));
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let content =
        html::text_between(body, "entry-content", "</div>").unwrap_or_else(|| body.to_string());
    let block = html::text_between(&content, "<ul", "</ul>").unwrap_or_else(|| content.clone());
    let source = if block.contains("<a") { block } else { content };
    source
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
            Some(novel_sites::chapter_item(
                SITE,
                novel_sites::key(SITE, &href),
                Some(title),
                novel_sites::chapter_number(&href),
            ))
        })
        .collect()
}

fn author(text: &str) -> Option<String> {
    for label in [
        "\u{00c9}crit par",
        "Ecrit par",
        "Auteur original de l\u{2019}oeuvre\u{00a0}:",
        "Auteur\u{00a0}:",
        "Auteure\u{00a0}:\u{00a0}",
        "Auteur original\u{00a0}:",
    ] {
        if let Some(value) = novel_sites::text_after_label(text, label) {
            return Some(value.replace(". Traduction", "").trim().to_string());
        }
    }
    None
}

fn genre(text: &str) -> Option<String> {
    novel_sites::text_between_markers(text, "Genre", "Synopsis").map(|value| {
        value
            .replace('\u{00a0}', " ")
            .replace("s :", "")
            .replace(':', "")
            .trim()
            .trim_start_matches('s')
            .trim()
            .to_string()
    })
}

fn summary(body: &str) -> String {
    let content =
        html::text_between(body, "entry-content", "</div>").unwrap_or_else(|| body.to_string());
    let excludes = [
        "\u{00c9}crit par",
        "Ecrit par",
        "Sorties r\u{00e9}guli\u{00e8}res",
        "Auteur\u{00a0}:",
        "Statut VO\u{00a0}:",
        "Nom utilis\u{00e9}\u{00a0}:",
        "Auteur original\u{00a0}:",
        "Index\u{00a0}:",
    ];
    content
        .split("<p")
        .skip(1)
        .filter_map(|chunk| {
            if chunk.contains("xiaowaz.fr/articles") {
                return None;
            }
            let text = html::text_between(chunk, ">", "</p>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            if excludes.iter().any(|needle| text.contains(needle))
                || text.contains("Genre")
                || text.contains("Synopsis")
            {
                return None;
            }
            Some(text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn chapter_window(body: &str) -> String {
    let start = body.find("wp-post-navigation").unwrap_or(0);
    let rest = &body[start..];
    let end = rest
        .find("abh_box abh_box_down abh_box_business")
        .or_else(|| rest.find("https://ko-fi.com/wazouille"))
        .unwrap_or(rest.len());
    rest[..end].replace("<p>&nbsp;</p>", "<p/>")
}

fn find_cover(body: &str) -> Option<String> {
    html::attr_after(body, "fetchpriority", "src")
        .or_else(|| html::attr_after(body, "aligncenter", "src"))
        .or_else(|| html::attr_after(body, "<img", "src"))
}

fn fetch_key(key: &str, fixture: &str) -> String {
    novel_sites::fetch_document(SITE, &novel_sites::absolute_url(SITE, key), fixture)
}

const LIST_FIXTURE: &str =
    r#"<li class="page_item"><a href="https://xiaowaz.fr/series/sample">Sample Novel</a></li>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="card_title">Sample Novel</h1><div class="entry-content"><p><img class="aligncenter" src="/cover.jpg"></p><p>Auteur&nbsp;: Sample Author</p><p>Genre : Action Synopsis</p><p>Sample summary.</p><ul><li><a href="https://xiaowaz.fr/articles/chapter-1">Chapter 1</a></li></ul></div>"#;
const TEXT_FIXTURE: &str = r#"<div class="wp-post-navigation"></div><p>Sample chapter text.</p><div class="abh_box abh_box_down abh_box_business"></div>"#;

export_novel_source!(SOURCE);
