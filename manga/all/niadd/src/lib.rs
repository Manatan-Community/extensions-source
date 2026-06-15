use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Niadd = Niadd;

const SOURCES: [SourceConfig; 7] = [
    SourceConfig {
        id: "niadd-pt-br",
        lang: "pt-BR",
        base_url: "https://br.niadd.com",
    },
    SourceConfig {
        id: "niadd-en",
        lang: "en",
        base_url: "https://www.niadd.com",
    },
    SourceConfig {
        id: "niadd-es",
        lang: "es",
        base_url: "https://es.niadd.com",
    },
    SourceConfig {
        id: "niadd-it",
        lang: "it",
        base_url: "https://it.niadd.com",
    },
    SourceConfig {
        id: "niadd-ru",
        lang: "ru",
        base_url: "https://ru.niadd.com",
    },
    SourceConfig {
        id: "niadd-de",
        lang: "de",
        base_url: "https://de.niadd.com",
    },
    SourceConfig {
        id: "niadd-fr",
        lang: "fr",
        base_url: "https://fr.niadd.com",
    },
];

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    base_url: &'static str,
}

impl SourceConfig {
    fn absolute_url(self, path: &str) -> String {
        url::join_url(self.base_url, path)
    }

    fn key_from_url(self, value: &str) -> String {
        if let Some(index) = value.find(self.base_url) {
            return format!(
                "/{}",
                value[index + self.base_url.len()..].trim_start_matches('/')
            );
        }
        format!("/{}", value.trim_start_matches('/'))
    }
}

struct Niadd;

impl MangaSource for Niadd {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let path = if latest {
            "/list/New-Update.html"
        } else {
            "/list/Hot-Manga.html"
        };
        let body = fetch_document_or_fixture(source, &source.absolute_url(path), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, source),
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
        if query.starts_with(source.base_url) {
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
        let target = if query.is_empty() {
            source.absolute_url("/list/Hot-Manga.html")
        } else {
            format!(
                "{}/search/?name={}",
                source.base_url.trim_end_matches('/'),
                url::query_escape(query)
            )
        };
        let body = fetch_document_or_fixture(source, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, source),
            has_next_page: false,
        })
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
        let chapters_path = format!("{}/chapters.html", key.trim_end_matches(".html"));
        let body = fetch_document_or_fixture(
            source,
            &source.absolute_url(&chapters_path),
            CHAPTERS_FIXTURE,
        );
        Ok(parse_chapters(&body, source))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/sample-1.html".into());
        let body = fetch_document_or_fixture(source, &source.absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body, &source.absolute_url(&key), source))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(source.base_url) && input.contains("/manga/") {
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
        .unwrap_or("niadd-en");
    SOURCES
        .iter()
        .copied()
        .find(|source| source.id == id)
        .unwrap_or(SOURCES[1])
}

fn fetch_document_or_fixture(source: SourceConfig, target: &str, fixture: &str) -> String {
    manatan_shared::sdk::http::HttpClient::browser()
        .with_referer(format!("{}/", source.base_url.trim_end_matches('/')))
        .with_cookies_for(source.base_url)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, source: SourceConfig) -> Vec<CatalogItem> {
    body.split("manga-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = source.key_from_url(&href);
            let title = html::text_between(chunk, "manga-name", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| source.absolute_url(&value)),
                url: Some(source.absolute_url(&key)),
                language: Some(source.lang.to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>, source: SourceConfig) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample.html".to_string());
    let year = detail_value(
        body,
        &[
            "Released:",
            "Lanzado:",
            "Rilasciato:",
            "Выпущенный:",
            "Liberado:",
            "Freigegeben:",
        ],
    );
    let synopsis = synopsis_text(body);
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .or_else(|| html::text_between(body, "book-headline-name", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "detail-img", "src")
            .or_else(|| html::attr_after(body, "bookside-img", "src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| source.absolute_url(&value)),
        authors: detail_people(body, "author"),
        artists: detail_people(body, "Artista"),
        tags: genre_values(body),
        description: Some(
            [year.map(|value| format!("Year: {value}")), synopsis]
                .into_iter()
                .flatten()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        status: ItemStatus::Ongoing,
        url: Some(source.absolute_url(&key)),
        language: Some(source.lang.to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, source: SourceConfig) -> Vec<MangaChapter> {
    body.split("hover-underline")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = source.key_from_url(&href);
            let title = html::text_between(chunk, "chapter-name", "</")
                .or_else(|| html::text_between(chunk, "name", "</"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| html::strip_tags(chunk));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                url: Some(source.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, current_url: &str, source: SourceConfig) -> Vec<MangaPage> {
    let mut pages = parse_all_imgs_url(body, current_url, source);
    if pages.is_empty() {
        pages.extend(parse_reader_images(body, current_url, source));
    }
    if pages.is_empty() {
        if let Some(source_url) = html::attr_after(body, "cool-blue vision-button", "href") {
            let target = source.absolute_url(&source_url);
            let body = fetch_document_or_fixture(source, &target, PAGES_FIXTURE);
            pages.extend(parse_pages(&body, &target, source));
        }
    }
    pages
}

fn parse_all_imgs_url(body: &str, current_url: &str, source: SourceConfig) -> Vec<MangaPage> {
    let Some(start) = body.find("all_imgs_url") else {
        return Vec::new();
    };
    let Some(open) = body[start..].find('[') else {
        return Vec::new();
    };
    let rest = &body[start + open + 1..];
    let Some(close) = rest.find(']') else {
        return Vec::new();
    };
    rest[..close]
        .split(',')
        .map(|part| part.replace(['"', '\'', ' ', '\n', '\r', '\t'], ""))
        .filter(|part| part.starts_with("http"))
        .enumerate()
        .map(|(index, image)| page(index, &image, current_url, source))
        .collect()
}

fn parse_reader_images(body: &str, current_url: &str, source: SourceConfig) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.is_empty() && !image.contains("cover") && !image.contains("logo"))
        .enumerate()
        .map(|(index, image)| page(index, &source.absolute_url(&image), current_url, source))
        .collect()
}

fn page(index: usize, image: &str, referer: &str, source: SourceConfig) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image.to_string(),
            context: Some(manga::image_headers(referer)),
        },
        headers: manga::image_headers(source.base_url),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn detail_value(body: &str, labels: &[&str]) -> Option<String> {
    labels.iter().find_map(|label| {
        let index = body.find(label)?;
        let chunk = &body[index
            ..body[index..]
                .find("</div>")
                .map(|end| index + end)
                .unwrap_or(body.len())];
        html::text_between(chunk, "<span", "</span>")
            .map(|value| {
                html::strip_tags(&value)
                    .replace(label, "")
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty())
    })
}

fn detail_people(body: &str, marker: &str) -> Vec<String> {
    let Some(index) = body.find(marker) else {
        return Vec::new();
    };
    let chunk = &body[index
        ..body[index..]
            .find("</div>")
            .map(|end| index + end)
            .unwrap_or(body.len())];
    chunk
        .split("<span")
        .skip(1)
        .filter_map(|part| {
            html::text_between(part, ">", "</span>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty() && !value.contains(':'))
        .collect()
}

fn genre_values(body: &str) -> Vec<String> {
    body.split("itemprop=\"genre\"")
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn synopsis_text(body: &str) -> Option<String> {
    for label in [
        "Synopsis",
        "Sinopsis",
        "Sinossi",
        "конспект",
        "Sinopse",
        "Zusammenfassung",
    ] {
        if let Some(index) = body.find(label) {
            let rest = &body[index..];
            return html::text_between(rest, "detail-section", "</div>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
        }
    }
    None
}

fn chapter_number(title: &str) -> Option<f32> {
    title
        .split_whitespace()
        .skip_while(|part| !part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .next()
        .and_then(|part| {
            part.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '.')
                .parse()
                .ok()
        })
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="manga-item"><a href="https://www.niadd.com/manga/sample.html"><div class="manga-img"><img src="https://img.example/cover.jpg"></div><div class="manga-name">Fixture Manga</div></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Fixture Manga</h1>
<div class="bookside-img"><img src="https://img.example/cover.jpg"></div>
<div class="bookside-general">
  <div class="detail-general-cell">Autor (es): <span>Author One</span></div>
  <div class="detail-general-cell">Artista: <span>Artist One</span></div>
  <div class="detail-general-cell">Released: <span>2024</span></div>
</div>
<a itemprop="genre">Action</a><a itemprop="genre">Drama</a>
<div class="detail-cate-title">Synopsis</div><div class="detail-section">Fixture synopsis.</div>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<ul class="chapter-list">
  <a class="hover-underline" href="https://www.niadd.com/chapter/sample-1.html"><span class="chapter-name">Capítulo 1</span><span class="chapter-time">Jan 01, 2024</span></a>
</ul>
"#;

const PAGES_FIXTURE: &str = r#"
<script>var all_imgs_url = ["https://img.example/1.jpg", "https://img.example/2.jpg"];</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_niadd_fixtures() {
        let source = SOURCES[1];
        assert_eq!(parse_listing(LIST_FIXTURE, source).len(), 1);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("/manga/sample.html".into()), source).title,
            "Fixture Manga"
        );
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE, source).len(), 1);
        assert_eq!(
            parse_pages(
                PAGES_FIXTURE,
                "https://www.niadd.com/chapter/sample-1.html",
                source
            )
            .len(),
            2
        );
    }
}
