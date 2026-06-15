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
    id: "warriorlegendtrad",
    name: "Warrior Legend Trad",
    lang: "fr",
    base_url: "https://warriorlegendtrad.wordpress.com",
    popular_path: "light-novel",
    latest_path: "crea",
    search_path: "?s={query}&page={page}",
    content_rating: "safe",
};

const SOURCE: Source = Source;

struct Source;

impl NovelSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(Paged {
            entries: match page {
                1 => listing("light-novel"),
                2 => listing("crea"),
                _ => Vec::new(),
            },
            has_next_page: page == 1,
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
                novel_sites::filter_local_catalog(listing("light-novel"), query)
            } else {
                Vec::new()
            },
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "light-novel/sample".to_string());
        Ok(details_for_key(&novel_sites::key(SITE, &key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "light-novel/sample".to_string());
        let body = fetch_key(&novel_sites::key(SITE, &key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        chapters.sort_by(|a, b| {
            a.date_uploaded
                .cmp(&b.date_uploaded)
                .then_with(|| a.title.cmp(&b.title))
        });
        Ok(chapters)
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
            .replace("<hr/>", "");
        Ok(novel_sites::text_from_html(SITE, &key, title, content))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let mut second = request;
        if let Some(obj) = second.as_object_mut() {
            obj.insert("page".to_string(), Value::from(2));
        }
        let crea = self.list(second)?;
        Ok(vec![
            HomeSection {
                id: "light-novel".to_string(),
                title: "Light Novel".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: true,
                ..HomeSection::default()
            },
            HomeSection {
                id: "crea".to_string(),
                title: "Creations".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: crea.entries,
                has_more: false,
                ..HomeSection::default()
            },
        ])
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

fn listing(path: &str) -> Vec<CatalogItem> {
    let target = url::join_url(SITE.base_url, path);
    let body = novel_sites::fetch_document(SITE, &target, LIST_FIXTURE);
    body.split("<article")
        .skip(1)
        .filter_map(|article| {
            let header = article
                .split("entry-wrapper")
                .nth(1)
                .unwrap_or(article)
                .split("</h2>")
                .next()
                .unwrap_or(article);
            let href = html::attr_after(header, "<a", "href")?;
            let title = html::text_between(header, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let cover = html::attr_after(article, "<img", "src");
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
    let title = html::text_between(&body, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SITE.name.to_string()));
    let text = html::strip_tags(
        &html::text_between(&body, "entry-content", "</div>").unwrap_or_else(|| body.clone()),
    );
    let mut item = novel_sites::catalog_item(
        SITE,
        key.to_string(),
        title,
        html::attr_after(&body, "<figure", "src")
            .or_else(|| html::attr_after(&body, "<img", "src")),
        true,
    );
    item.authors = first_after(&text, &["Auteur\u{00a0}:", "Auteur :"])
        .into_iter()
        .collect();
    item.tags = first_after(&text, &["Genre :"]).into_iter().collect();
    item.description =
        novel_sites::text_between_markers(&text, "Synopsis\u{00a0}:", "index chapitre :")
            .or_else(|| novel_sites::text_between_markers(&text, "Synopsis :", "index chapitre :"));
    item.status = status(&text);
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
            Some(novel_sites::chapter_item(
                SITE,
                novel_sites::key(SITE, &href),
                Some(title),
                novel_sites::chapter_number(&href),
            ))
        })
        .collect()
}

fn first_after(text: &str, labels: &[&str]) -> Option<String> {
    labels
        .iter()
        .find_map(|label| novel_sites::text_after_label(text, label))
}

fn status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("en pause") {
        ItemStatus::Hiatus
    } else if lower.contains("termin") {
        ItemStatus::Completed
    } else if lower.contains("abandon") {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Ongoing
    }
}

fn fetch_key(key: &str, fixture: &str) -> String {
    novel_sites::fetch_document(SITE, &novel_sites::absolute_url(SITE, key), fixture)
}

const LIST_FIXTURE: &str = r#"<article><div class="entry-wrapper"><h2><a href="https://warriorlegendtrad.wordpress.com/light-novel/sample">Sample Novel</a></h2></div><figure><a><img src="/cover.jpg"></a></figure></article>"#;
const DETAILS_FIXTURE: &str = r#"<article><header><h1>Sample Novel</h1></header><figure><img src="/cover.jpg"></figure><div class="entry-content"><p>Auteur : Sample Author</p><p>Genre : Action</p><p>Synopsis : Sample summary index chapitre :</p><h2><a href="https://warriorlegendtrad.wordpress.com/2024/01/01/chapter-1">Chapter 1</a></h2></div></article>"#;
const TEXT_FIXTURE: &str = r#"<h1 class="entry-title">Chapter 1</h1><div class="entry-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
