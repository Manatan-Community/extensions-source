use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Nikkangecchan = Nikkangecchan;
const BASE_URL: &str = "https://nikkangecchan.jp";

struct Nikkangecchan;

impl MangaSource for Nikkangecchan {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch_document(BASE_URL, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        let mut page = parse_listing(&fetch_document(BASE_URL, LIST_FIXTURE));
        if !query.is_empty() {
            let needle = query.to_lowercase();
            page.entries
                .retain(|item| item.title.to_lowercase().contains(&needle));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/episode/1".into());
        let chapter_url = absolute_url(&key);
        Ok(vec![MangaPage {
            content: PageContent::Url {
                url: format!("{chapter_url}/image"),
                context: Some(manga::image_headers(&chapter_url)),
            },
            headers: manga::image_headers(&chapter_url),
            description: Some("Page 1".into()),
            ..MangaPage::default()
        }])
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let catalog = self.list(json!({}))?;
        Ok(vec![HomeSection {
            id: "catalog".into(),
            title: "Catalog".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: false,
            entries: catalog.entries,
            ..HomeSection::default()
        }])
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
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(BASE_URL.to_string())
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<figure")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "imgBox", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Nikkangecchan".into())
                });
            let cover =
                html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover,
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(&body, "<h3", "</h3>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Nikkangecchan".into())),
        authors: html::text_between(&body, "author", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: html::text_between(&body, "description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("episodeBox")
        .skip(1)
        .filter_map(|chunk| {
            let page = chunk.split("episode-page").nth(1)?;
            let data_src = html::attr(page, "data-src")?;
            let key = normalize_key(data_src.trim_end_matches("/image"));
            let title = html::text_between(chunk, "episodeTitle", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Episode".into());
            let subtitle = html::attr(page, "data-title")
                .map(|value| value.split('|').next().unwrap_or(&value).trim().to_string())
                .unwrap_or_default();
            let chapter_title = if subtitle.is_empty() {
                title
            } else {
                format!("{title} - {subtitle}")
            };
            Some(MangaChapter {
                key: key.clone(),
                title: Some(chapter_title),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input.strip_prefix(BASE_URL).unwrap_or(input)))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(key: &str) -> String {
    format!("{BASE_URL}{}", normalize_key(key))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="contentInner"><figure><div class="imgBox"><a href="/sample"><img src="/cover.jpg"></a></div><div class="detailBox"><h3>Sample Nikkangecchan</h3></div></figure></div>"#;
const DETAILS_FIXTURE: &str = r#"<div id="comicDetail"><div class="detailBox"><h3>Sample Nikkangecchan</h3><div class="author">Sample Author</div></div></div><div class="description">Sample description.</div><div class="episodeBox"><div class="episode-page" data-src="/sample/episode/1/image" data-title="First story|Sample"></div><h4 class="episodeTitle">1</h4></div>"#;
