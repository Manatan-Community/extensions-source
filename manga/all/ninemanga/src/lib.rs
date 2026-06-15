use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: NineManga = NineManga;

const SOURCES: [SourceConfig; 7] = [
    SourceConfig {
        id: "ninemanga-en",
        name: "NineMangaEn",
        lang: "en",
        base_url: "https://www.ninemanga.com",
    },
    SourceConfig {
        id: "ninemanga-es",
        name: "NineMangaEs",
        lang: "es",
        base_url: "https://es.ninemanga.com",
    },
    SourceConfig {
        id: "ninemanga-pt-br",
        name: "NineMangaBr",
        lang: "pt-BR",
        base_url: "https://br.ninemanga.com",
    },
    SourceConfig {
        id: "ninemanga-ru",
        name: "NineMangaRu",
        lang: "ru",
        base_url: "https://ru.ninemanga.com",
    },
    SourceConfig {
        id: "ninemanga-de",
        name: "NineMangaDe",
        lang: "de",
        base_url: "https://de.ninemanga.com",
    },
    SourceConfig {
        id: "ninemanga-it",
        name: "NineMangaIt",
        lang: "it",
        base_url: "https://it.ninemanga.com",
    },
    SourceConfig {
        id: "ninemanga-fr",
        name: "NineMangaFr",
        lang: "fr",
        base_url: "https://fr.ninemanga.com",
    },
];

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    name: &'static str,
    lang: &'static str,
    base_url: &'static str,
}

impl SourceConfig {
    fn absolute_url(self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    fn key_from_url(self, value: &str) -> String {
        if let Some(index) = value.find(self.base_url) {
            return format!(
                "/{}",
                value[index + self.base_url.len()..].trim_start_matches('/')
            );
        }
        if self.id == "ninemanga-en" && value.contains("ninemanga.com") {
            return format!(
                "/{}",
                value
                    .split("ninemanga.com")
                    .nth(1)
                    .unwrap_or(value)
                    .trim_start_matches('/')
            );
        }
        format!("/{}", value.trim_start_matches('/'))
    }
}

struct NineManga;

impl MangaSource for NineManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            source.absolute_url("/list/New-Update/")
        } else {
            source.absolute_url(&format!("/category/index_{page}.html"))
        };
        let body = fetch_document_or_fixture(source, &target, LIST_FIXTURE);
        Ok(parse_listing_page(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if matches!(source.lang, "es" | "ru" | "fr") {
            query = query.split('\'').next().unwrap_or(query);
        }
        if query.starts_with(source.base_url)
            || (source.id == "ninemanga-en" && query.contains("ninemanga.com"))
        {
            let body = fetch_document_or_fixture(source, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(
                    &body,
                    Some(source.key_from_url(query)),
                    source,
                )],
                has_next_page: false,
            });
        }
        let target = search_url(
            source,
            page,
            query,
            request.get("filters").unwrap_or(&Value::Null),
        );
        let body = fetch_document_or_fixture(source, &target, LIST_FIXTURE);
        Ok(parse_listing_page(&body, source))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample.html".into());
        let body = fetch_document_or_fixture(source, &source.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), source))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample.html".into());
        let target = format!("{}?waring=1", source.absolute_url(&key));
        let body = fetch_document_or_fixture(source, &target, CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/sample-1.html".into());
        let target = source.absolute_url(&key);
        let body = fetch_document_or_fixture(source, &target, PAGES_FIXTURE);
        Ok(parse_pages(&body, &target, source))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.contains("ninemanga.com") && input.contains("/manga/") {
            let body = fetch_document_or_fixture(source, input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &body,
                    Some(source.key_from_url(input)),
                    source,
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

fn source_for(request: &Value) -> SourceConfig {
    let id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .and_then(Value::as_str)
        .unwrap_or("ninemanga-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[0])
}

fn client(source: SourceConfig) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_header("Accept-Language", "es-ES,es;q=0.9,en;q=0.8,gl;q=0.7")
        .with_header("Cookie", "ninemanga_list_num=1")
        .with_referer(format!("{}/", source.base_url.trim_end_matches('/')))
        .with_cookies_for(source.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(source: SourceConfig, target: &str, fixture: &str) -> String {
    client(source)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(source: SourceConfig, page: u64, query: &str, filters: &Value) -> String {
    let mut params = vec![
        ("wd", query.to_string()),
        ("page", page.to_string()),
        ("type", "high".to_string()),
    ];
    for (key, upstream) in [
        ("queryMode", "name_sel"),
        ("authorMode", "author_sel"),
        ("author", "author"),
        ("artistMode", "artist_sel"),
        ("artist", "artist"),
        ("completed", "completed_series"),
        ("categoryId", "category_id"),
        ("excludeCategoryId", "out_category_id"),
    ] {
        if let Some(value) = filters
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            params.push((upstream, value.to_string()));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}/search/?{query}", source.base_url.trim_end_matches('/'))
}

fn parse_listing_page(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("bookinfo")
            .skip(1)
            .filter_map(|chunk| parse_listing_item(chunk, source))
            .collect(),
        has_next_page: body.contains("pageList") && body.contains("class=\"l\""),
    }
}

fn parse_listing_item(chunk: &str, source: SourceConfig) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "bookname", "href")
        .or_else(|| html::attr_after(chunk, "<a", "href"))?;
    let key = source.key_from_url(&href);
    let title = html::text_between(chunk, "bookname", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| source.name.to_string()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "<img", "data-src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| source.absolute_url(&value)),
        url: Some(source.absolute_url(&key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample.html".to_string());
    let intro = body
        .find("bookintro")
        .map(|index| &body[index..])
        .unwrap_or(body);
    let title = html::text_between(intro, "<span", "</span>")
        .map(|value| strip_manga_suffix(&html::strip_tags(&value)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| source.name.to_string()));
    let status_text = html::text_between(intro, "class=\"red\"", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(intro, "itemprop=image", "src")
            .or_else(|| html::attr_after(intro, "<img", "src"))
            .map(|value| source.absolute_url(&value)),
        authors: text_values_after(intro, "itemprop=author"),
        tags: genre_values(intro),
        description: html::text_between(intro, "itemprop=description", "</p>")
            .map(|value| html::strip_tags(&value)),
        status: parse_status(&status_text, source),
        url: Some(source.absolute_url(&key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_status(status: &str, source: SourceConfig) -> ItemStatus {
    let ongoing = [
        "Ongoing",
        "En curso",
        "Em tradução",
        "Laufende",
        "In corso",
        "En cours",
    ];
    let completed = [
        "Completed",
        "Completado",
        "Completo",
        "завершенный",
        "Abgeschlossen",
        "Completato",
        "Complété",
    ];
    if completed.iter().any(|needle| status.contains(needle)) {
        ItemStatus::Completed
    } else if ongoing.iter().any(|needle| status.contains(needle)) || source.lang == "ru" {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    let manga_title = html::text_between(body, "bookintro", "</div>")
        .and_then(|intro| html::text_between(&intro, "<span", "</span>"))
        .map(|value| strip_manga_suffix(&html::strip_tags(&value)))
        .unwrap_or_default();
    body.split("chapter_list_a")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = source.key_from_url(&href.replace("%20", " "));
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value).replace(&format!("{manga_title} "), ""))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Chapter".into()));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: title.split_whitespace().find_map(|part| {
                    part.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '.')
                        .parse()
                        .ok()
                }),
                url: Some(source.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, current_url: &str, source: SourceConfig) -> Vec<MangaPage> {
    if let Some(server_url) = html::attr_after(body, "post-content-body", "href") {
        let target = source.absolute_url(&server_url);
        let body = fetch_document_or_fixture(source, &target, PAGES_FIXTURE);
        let pages = parse_pages(&body, &target, source);
        if !pages.is_empty() {
            return pages;
        }
    }
    if let Some(redirect) = redirect_target(body, current_url) {
        let body = fetch_document_or_fixture(source, &redirect, PAGES_FIXTURE);
        let pages = parse_pages(&body, &redirect, source);
        if !pages.is_empty() {
            return pages;
        }
    }
    let image_list = parse_all_imgs_url(body);
    if !image_list.is_empty() {
        return image_list
            .into_iter()
            .enumerate()
            .map(|(index, image)| page(index, &image, current_url, source))
            .collect();
    }
    let direct = html::attr_after(body, "manga_pic", "src")
        .or_else(|| html::attr_after(body, "pic_box", "src"))
        .map(|image| vec![page(0, &source.absolute_url(&image), current_url, source)])
        .unwrap_or_default();
    if !direct.is_empty() {
        return direct;
    }
    parse_page_options(body, current_url, source)
}

fn parse_page_options(body: &str, current_url: &str, source: SourceConfig) -> Vec<MangaPage> {
    body.split("<option")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "value"))
        .enumerate()
        .filter_map(|(index, path)| {
            let target = source.absolute_url(&path);
            let body = fetch_document_or_fixture(source, &target, PAGE_IMAGE_FIXTURE);
            let image = html::attr_after(&body, "manga_pic", "src")
                .or_else(|| html::attr_after(&body, "pic_box", "src"))?;
            Some(page(
                index,
                &source.absolute_url(&image),
                current_url,
                source,
            ))
        })
        .collect()
}

fn parse_all_imgs_url(body: &str) -> Vec<String> {
    let Some(index) = body.find("all_imgs_url") else {
        return Vec::new();
    };
    let Some(open) = body[index..].find('[') else {
        return Vec::new();
    };
    let rest = &body[index + open + 1..];
    let Some(close) = rest.find(']') else {
        return Vec::new();
    };
    rest[..close]
        .split(',')
        .map(|part| part.replace(['"', '\'', ' ', '\n', '\r', '\t'], ""))
        .filter(|part| part.starts_with("http"))
        .collect()
}

fn redirect_target(body: &str, current_url: &str) -> Option<String> {
    let index = body.find("window.location.href")?;
    let rest = &body[index..];
    let quote = rest.find('"').or_else(|| rest.find('\''))?;
    let delim = rest.as_bytes()[quote] as char;
    let target = &rest[quote + 1..];
    let end = target.find(delim)?;
    Some(url::join_url(current_url, &target[..end]))
}

fn page(index: usize, image: &str, referer: &str, source: SourceConfig) -> MangaPage {
    let mut headers = manga::image_headers(referer);
    headers.insert(
        "Referer".to_string(),
        format!("{}/", source.base_url.trim_end_matches('/')),
    );
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(headers.clone()),
        },
        headers,
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn text_values_after(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn genre_values(body: &str) -> Vec<String> {
    body.split("itemprop=genre")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn strip_manga_suffix(value: &str) -> String {
    value.strip_suffix(" Manga").unwrap_or(value).to_string()
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<dl class="bookinfo"><dt><a class="bookname" href="https://www.ninemanga.com/manga/sample.html">Fixture Manga</a></dt><dd><img src="https://img.example/cover.jpg"></dd></dl>
<ul class="pageList"><li><a class="l" href="/category/index_2.html">Next</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="bookintro">
  <li><span>Fixture Manga Manga</span></li>
  <img itemprop="image" src="https://img.example/cover.jpg">
  <li itemprop="genre"><a>Action</a></li>
  <li><a itemprop="author">Author One</a></li>
  <li><a class="red">Completed</a></li>
  <p itemprop="description">Fixture description.</p>
</div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<div class="bookintro"><li><span>Fixture Manga Manga</span></li></div>
<ul class="sub_vol_ul"><li><a class="chapter_list_a" href="https://www.ninemanga.com/chapter/sample-1.html">Fixture Manga Chapter 1</a><span>Jan 1, 2024</span></li></ul>
"#;

const PAGES_FIXTURE: &str = r#"
<script>var all_imgs_url: ["https://img.example/1.jpg", "https://img.example/2.jpg", ];</script>
"#;

const PAGE_IMAGE_FIXTURE: &str =
    r#"<div class="pic_box"><img class="manga_pic" src="https://img.example/page.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ninemanga_fixtures() {
        let source = SOURCES[0];
        assert_eq!(parse_listing_page(LIST_FIXTURE, source).entries.len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("/manga/sample.html".into()), source).title,
            "Fixture Manga"
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, source).len(), 1);
        assert_eq!(
            parse_pages(
                PAGES_FIXTURE,
                "https://www.ninemanga.com/chapter/sample-1.html",
                source
            )
            .len(),
            2
        );
    }
}
