use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ReadOnePieceMangaOnline = ReadOnePieceMangaOnline;
const BASE_URL: &str = "https://ww12.readonepiece.com";
const OPM_LAYOUT: bool = false;
const SOURCE_LIST: &[(&str, &str)] = &[
    ("One Piece", "/manga/one-piece/"),
    ("Colored", "/manga/one-piece-digital-colored-comics/"),
    ("Soma x Sanji", "/manga/shokugeki-no-sanji-one-shot/"),
    ("OP x Toriko", "/manga/one-piece-x-toriko/"),
    ("Party", "/manga/one-piece-party/"),
    ("DB x OP", "/manga/dragon-ball-x-one-piece/"),
    ("Wanted!", "/manga/wanted-one-piece/"),
    ("Ace's Story", "/manga/one-piece-ace-s-story/"),
    ("Omake", "/manga/one-piece-omake/"),
    ("Vivre Card", "/manga/vivre-card-databook/"),
    ("Pirate Recipes", "/manga/one-piece-pirate-recipes/"),
    ("Databook", "/manga/one-piece-databook/"),
    ("Ace's Story Manga", "/manga/one-piece-ace-story-manga/"),
    ("OP Academy", "/manga/one-piece-academy/"),
    ("MONSTERS", "/manga/monsters/"),
    ("Zoro Novel", "/manga/one-piece-novel-zoro/"),
    ("OP in Love", "/manga/one-piece-in-love/"),
    ("Heroines", "/manga/one-piece-novel-heroines/"),
];

struct ReadOnePieceMangaOnline;

impl MangaSource for ReadOnePieceMangaOnline {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: SOURCE_LIST
                .iter()
                .map(|(title, path)| catalog_item(title, path, false))
                .collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let needle = query.to_ascii_lowercase();
        Ok(Paged {
            entries: SOURCE_LIST
                .iter()
                .filter(|(title, _)| title.to_ascii_lowercase().contains(&needle))
                .map(|(title, path)| catalog_item(title, path, false))
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| SOURCE_LIST[0].1.to_string());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| SOURCE_LIST[0].1.to_string());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/chapter/one-piece-chapter-1/".to_string());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: input.contains("/manga/").then(|| {
                    parse_details(
                        &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                        Some(key),
                    )
                }),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalog_item(title: &str, path: &str, initialized: bool) -> CatalogItem {
    let key = normalize_key(path);
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        url: Some(absolute_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| SOURCE_LIST[0].1.to_string());
    let mut item = catalog_item(
        &url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()),
        &key,
        true,
    );
    if OPM_LAYOUT {
        if let Some(title) = html::text_between(body, "<h2", "</h2>")
            .map(|value| html::strip_tags(&value).replace("Manga:", ""))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            item.title = title;
        }
        item.cover = body
            .split("<img")
            .skip(1)
            .find(|chunk| chunk.contains("card-img-right"))
            .and_then(image_attr)
            .map(|image| url::join_url(BASE_URL, &image));
        item.description = body
            .split("card-body")
            .nth(1)
            .and_then(|chunk| html::text_between(chunk, "<p", "</p>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        return item;
    }
    if let Some(title) = h1_texts(body).last().filter(|value| !value.is_empty()) {
        item.title = title.to_string();
    }
    item.cover = first_image(body).map(|image| url::join_url(BASE_URL, &image));
    item.description = description(body);
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    if OPM_LAYOUT {
        return body
            .split("<tr")
            .skip(1)
            .filter(|chunk| chunk.contains("/chapter/"))
            .filter_map(parse_opm_chapter)
            .fold(Vec::new(), push_unique_chapter);
    }
    let mut chapters = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("col-span-4") && chunk.contains("/chapter/"))
        .filter_map(parse_chapter_chunk)
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() {
        chapters = body
            .split("<option")
            .skip(1)
            .filter(|chunk| chunk.contains("/chapter/"))
            .filter_map(parse_option_chapter)
            .fold(Vec::new(), push_unique_chapter);
    }
    chapters
}

fn parse_opm_chapter(chunk: &str) -> Option<MangaChapter> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "<td", "</td>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Chapter".to_string()));
    let date_text = chunk
        .split("<td")
        .nth(2)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    Some(chapter(&href, &title, date_text))
}

fn parse_chapter_chunk(chunk: &str) -> Option<MangaChapter> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "<a", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&href).unwrap_or_else(|| "Chapter".to_string()));
    Some(chapter(&href, &title, None))
}

fn parse_option_chapter(chunk: &str) -> Option<MangaChapter> {
    let href = html::attr(chunk, "value")?;
    let title = html::strip_tags(chunk);
    Some(chapter(&href, &title, None))
}

fn chapter(href: &str, title: &str, date_text: Option<String>) -> MangaChapter {
    let key = normalize_key(href);
    MangaChapter {
        key: key.clone(),
        title: Some(title.trim().to_string()),
        url: Some(absolute_url(&key)),
        date_uploaded: date_text
            .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("js-page")
                || chunk.contains("data-src")
                || chunk.contains("wp-manga-chapter-img")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.contains("/images/404."))
        .fold(Vec::<String>::new(), |mut acc, image| {
            let image = url::join_url(BASE_URL, &image);
            if !acc.contains(&image) {
                acc.push(image);
            }
            acc
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn h1_texts(body: &str) -> Vec<String> {
    body.split("<h1")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</h1>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn description(body: &str) -> Option<String> {
    body.split("Description")
        .nth(1)
        .and_then(|chunk| html::text_between(chunk, "<div", "</div>"))
        .or_else(|| html::text_between(body, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn first_image(body: &str) -> Option<String> {
    body.split("<img")
        .skip(1)
        .filter_map(image_attr)
        .find(|image| !image.starts_with("data:") && !image.contains("favicon"))
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim();
    let path = format!("/{}", path.trim_matches('/'));
    if path == "/" {
        SOURCE_LIST[0].1.trim_end_matches('/').to_string()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

fn absolute_url(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn push_unique_chapter(mut acc: Vec<MangaChapter>, chapter: MangaChapter) -> Vec<MangaChapter> {
    if !acc.iter().any(|item| item.key == chapter.key) {
        acc.push(chapter);
    }
    acc
}

export_manga_source!(SOURCE);

const DETAILS_FIXTURE: &str = r#"
<div class="container"><h1>One Piece</h1></div>
<div class="flex"><img src="https://i.imgur.com/NKmkkq1.png"></div>
<div>Description</div><div>Monkey D. Luffy sails toward the Grand Line.</div>
<div class="col-span-4"><a href="https://ww12.readonepiece.com/chapter/one-piece-chapter-1/">One Piece Chapter 1</a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="js-pages-container">
<img data-src="https://cdn.readonepiece.com/file/mangap/sample/1.jpeg" class="js-page">
<img data-src="https://cdn.readonepiece.com/file/mangap/sample/2.jpeg" class="js-page">
</div>
"#;
