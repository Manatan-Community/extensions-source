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
    id: "novhell",
    name: "Novhell",
    lang: "fr",
    base_url: "https://novhell.org",
    popular_path: "",
    latest_path: "",
    search_path: "?s={query}&page={page}",
    content_rating: "suggestive",
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
            novel_sites::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        Ok(details_for_key(&novel_sites::key(SITE, &key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel_sites::request_key(&request, "novel").unwrap_or_else(|| "sample".to_string());
        let body = fetch_key(&novel_sites::key(SITE, &key), DETAILS_FIXTURE);
        let mut chapters = parse_chapters(&body);
        chapters.sort_by(|a, b| a.chapter_number.partial_cmp(&b.chapter_number).unwrap());
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
            .unwrap_or_else(|| "sample-chapter-1".to_string());
        let key = novel_sites::key(SITE, &key);
        let body = fetch_key(&key, TEXT_FIXTURE);
        let sections = body.split("<section").skip(1).collect::<Vec<_>>();
        let mut title = None;
        let mut chapter = String::new();
        for section in sections.iter().rev().take(5).rev() {
            if section.contains("<h4") {
                title = html::text_between(section, "<h4", "</h4>")
                    .map(|value| html::strip_tags(&value));
                chapter = section.to_string();
            }
        }
        if chapter.is_empty() {
            chapter = html::text_between(&body, "<main", "</main>").unwrap_or(body.clone());
        }
        Ok(novel_sites::text_from_html(SITE, &key, title, chapter))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Novels".to_string(),
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
    body.split("<figure")
        .skip(1)
        .filter_map(|figure| {
            let title = html::text_between(figure, "<figcaption", "</figcaption>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let href = html::attr_after(figure, "<a", "href")?;
            let cover = html::attr_after(figure, "<img", "src");
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
    let title = html::attr_after(&body, "property=\"og:title\"", "content")
        .map(|value| value.replace("- NovHell", "").trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SITE.name.to_string()));
    let text = html::strip_tags(&body);
    let mut item = novel_sites::catalog_item(
        SITE,
        key.to_string(),
        title,
        html::attr_after(&body, "<img", "src"),
        true,
    );
    item.authors = first_after(&text, &["Ecrit par ", "Auteur", "Ecrit par :"])
        .into_iter()
        .collect();
    item.tags = first_after(&text, &["Genre"]).into_iter().collect();
    item.description = first_after(&text, &["Synopsis"]);
    item.status = ItemStatus::Ongoing;
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains(SITE.base_url) {
                return None;
            }
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value).replace('\u{00a0}', " "))
                .filter(|value| !value.is_empty())?;
            let number = chapter_number_from_title(&title);
            Some(novel_sites::chapter_item(
                SITE,
                novel_sites::key(SITE, &href),
                Some(title),
                Some(number as f32),
            ))
        })
        .collect()
}

fn first_after(text: &str, labels: &[&str]) -> Option<String> {
    labels
        .iter()
        .find_map(|label| novel_sites::text_after_label(text, label))
}

fn chapter_number_from_title(title: &str) -> u32 {
    title
        .split("Chapitre ")
        .skip(1)
        .filter_map(|part| part.split_whitespace().next()?.parse::<u32>().ok())
        .sum()
}

fn fetch_key(key: &str, fixture: &str) -> String {
    novel_sites::fetch_document_deflate(SITE, &novel_sites::absolute_url(SITE, key), fixture)
}

const LIST_FIXTURE: &str = r#"<figure><a href="https://novhell.org/sample"><img src="/cover.jpg"></a><figcaption><span><strong>Sample Novel</strong></span></figcaption></figure>"#;
const DETAILS_FIXTURE: &str = r#"<meta property="og:title" content="Sample Novel - NovHell"><img src="/cover.jpg"><p>Auteur : Sample Author</p><p>Genre : Action</p><p>Synopsis : Sample summary</p><p><a href="https://novhell.org/sample-chapter-1">Chapitre 1</a></p>"#;
const TEXT_FIXTURE: &str = r#"<main><article><section><h4>Chapter 1</h4></section><section><p>Sample chapter text.</p></section></article></main>"#;

export_novel_source!(SOURCE);
