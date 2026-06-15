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
    id: "noveldeglace",
    name: "NovelDeGlace",
    lang: "fr",
    base_url: "https://noveldeglace.com/",
    popular_path: "roman/page/{page}",
    latest_path: "chapitre/page/{page}",
    search_path: "?s={query}&page={page}",
    content_rating: "suggestive",
};

const SOURCE: Source = Source;

struct Source;

impl NovelSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let latest = listing == "latest";
        let path = if latest {
            format!("chapitre/page/{page}")
        } else if let Some(filter) = filter_value(&request, "categorie_genre") {
            match filter.as_str() {
                "all" | "categorie_roman" | "genre" if page > 1 => {
                    return Ok(Paged {
                        entries: Vec::new(),
                        has_next_page: false,
                    });
                }
                "all" | "categorie_roman" | "genre" => format!("roman/page/{page}"),
                value if value.starts_with("c_") => {
                    format!("categorie_roman/{}/page/{page}", &value[2..])
                }
                value if value.starts_with("g_") => format!("genre/{}/page/{page}", &value[2..]),
                _ => format!("roman/page/{page}"),
            }
        } else if page > 1 {
            return Ok(Paged {
                entries: Vec::new(),
                has_next_page: false,
            });
        } else {
            format!("roman/page/{page}")
        };
        let body = novel_sites::fetch_document_deflate(
            SITE,
            &url::join_url(SITE.base_url, &path),
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body, latest);
        Ok(Paged {
            has_next_page: !entries.is_empty()
                && (latest || filter_value(&request, "categorie_genre").is_some()),
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
        let entries = if page == 1 {
            let body = novel_sites::fetch_document_deflate(
                SITE,
                &url::join_url(SITE.base_url, "roman"),
                LIST_FIXTURE,
            );
            novel_sites::filter_local_catalog(parse_listing(&body, false), query)
        } else {
            Vec::new()
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "roman/sample".to_string());
        Ok(details_for_key(&novel_sites::key(SITE, &key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel_sites::request_key(&request, "novel")
            .unwrap_or_else(|| "roman/sample".to_string());
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
            .unwrap_or_else(|| "chapitre/sample".to_string());
        let key = novel_sites::key(SITE, &key);
        let body = fetch_key(&key, TEXT_FIXTURE).replace("mistape_caption", "removed_caption");
        let title =
            html::text_between(&body, "entry-title", "</h1>").map(|value| html::strip_tags(&value));
        let content = html::text_between(&body, "chapter-content", "</div>")
            .or_else(|| html::text_between(&body, "entry-content", "</div>"))
            .unwrap_or_else(|| body.clone());
        Ok(novel_sites::text_from_html(SITE, &key, title, content))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Romans".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Chapitres".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with("https://noveldeglace.com") {
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

fn parse_listing(body: &str, latest: bool) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter_map(|article| {
            let title = html::text_between(article, "<h2", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let href = if latest {
                article
                    .split("Roman")
                    .nth(1)
                    .and_then(|block| html::attr_after(block, "<a", "href"))
            } else {
                html::attr_after(article, "<h2", "href")
                    .or_else(|| html::attr_after(article, "<a", "href"))
            }?;
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
    let title = html::text_between(&body, "span.current", "</span>")
        .or_else(|| html::text_between(&body, "current", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| SITE.name.to_string()));
    let mut item = novel_sites::catalog_item(
        SITE,
        key.to_string(),
        title,
        html::attr_after(&body, "<img", "src"),
        true,
    );
    let synopsis = html::text_between(&body, "data-title=Synopsis", "</div>")
        .or_else(|| html::text_between(&body, "data-title=\"Synopsis\"", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    let tomes = html::text_between(&body, "data-title=Tomes", "</div>")
        .or_else(|| html::text_between(&body, "data-title=\"Tomes\"", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    item.description = Some(format!("{synopsis}\n\n{tomes}").trim().to_string());
    let text = html::strip_tags(&body);
    item.authors = novel_sites::text_after_label(&text, "Auteur :")
        .into_iter()
        .collect();
    item.artists = novel_sites::text_after_label(&text, "Illustrateur :")
        .into_iter()
        .collect();
    item.tags = [
        text_after_strip(&text, "Cat\u{00e9}gorie :", "Autre"),
        novel_sites::text_after_label(&text, "Genre :"),
    ]
    .into_iter()
    .flatten()
    .collect();
    item.status = status(&body);
    item
}

fn parse_chapters(body: &str) -> Vec<NovelChapter> {
    let mut chapters = Vec::new();
    for (index, chunk) in body.split("chpt").skip(1).enumerate() {
        let prefix = if body.matches("data-title=Tomes").count() > 1 {
            format!("T.{} ", index + 1)
        } else {
            String::new()
        };
        let base_name = html::text_between(chunk, "<a", "</a>")
            .map(|value| format!("{prefix}{}", html::strip_tags(&value)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("{prefix}Chapitre {}", index + 1));
        let links = chunk.split("<a").skip(1).collect::<Vec<_>>();
        for (part_index, link) in links.iter().enumerate() {
            let Some(href) = html::attr(link, "href") else {
                continue;
            };
            let title = if links.len() > 1 {
                format!("{} ({})", base_name, part_index + 1)
            } else {
                base_name.clone()
            };
            let release = link
                .split("</a>")
                .nth(1)
                .and_then(|tail| novel_sites::text_between_markers(tail, "(", ")"));
            let mut chapter = novel_sites::chapter_item(
                SITE,
                novel_sites::key(SITE, &href),
                Some(title),
                Some(index as f32 + (part_index as f32 / 1000.0)),
            );
            chapter.summary = release;
            chapters.push(chapter);
        }
    }
    chapters
}

fn status(body: &str) -> ItemStatus {
    if body.contains("type etat4") {
        ItemStatus::Hiatus
    } else if body.contains("type etat5") {
        ItemStatus::Completed
    } else if body.contains("type etat6") {
        ItemStatus::Cancelled
    } else if body.contains("type etat0") || body.contains("type etat1") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn text_after_strip(text: &str, label: &str, excluded: &str) -> Option<String> {
    novel_sites::text_after_label(text, label).filter(|value| value != excluded)
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(|value| value.get("value").or(Some(value)))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

fn fetch_key(key: &str, fixture: &str) -> String {
    novel_sites::fetch_document_deflate(SITE, &novel_sites::absolute_url(SITE, key), fixture)
}

const LIST_FIXTURE: &str = r#"<article><h2><a href="https://noveldeglace.com/roman/sample">Sample Novel</a></h2><img src="/cover.jpg"><span class="Roman"><a href="https://noveldeglace.com/roman/sample">Roman</a></span></article>"#;
const DETAILS_FIXTURE: &str = r#"<span class="current">Sample Novel</span><img src="/cover.jpg"><div data-title="Synopsis">Sample summary.</div><div data-title="Tomes"><div class="chpt"><a href="https://noveldeglace.com/chapitre/sample-1">Chapter 1</a> (2024-01-01)</div></div><p class="type etat1"><strong>Statut :</strong></p><p><strong>Auteur :</strong> Sample Author</p><p class="categorie">Catégorie : Shonen</p><p class="genre">Genre : Action</p>"#;
const TEXT_FIXTURE: &str = r#"<h1 class="entry-title">Chapter 1</h1><div class="chapter-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
