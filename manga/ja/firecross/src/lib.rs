use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, GenericImage, GenericImageView, ImageFormat};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::io::Cursor;

const SOURCE: FireCross = FireCross;
const BASE_URL: &str = "https://firecross.jp";

struct FireCross;

impl MangaSource for FireCross {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, "seriesList_item"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_listing(
            &fetch_document(
                &format!("{BASE_URL}/ebook/comics?sort=1&page={page}"),
                LIST_FIXTURE,
            ),
            "seriesList_item",
        ))
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
                    &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut target = format!(
            "{BASE_URL}/search?q={}&t=1&distribution_episode=1&page={page}",
            url::query_escape(query)
        );
        if let Some(labels) = request
            .get("filters")
            .and_then(|filters| filters.get("labels"))
            .and_then(Value::as_str)
        {
            for label in labels
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                target.push_str("&label%5B%5D=");
                target.push_str(&url::query_escape(label));
            }
        }
        Ok(parse_listing(
            &fetch_document(&target, SEARCH_FIXTURE),
            "seriesList_item",
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/ebook/series/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/ebook/series/sample".into());
        let hide_locked = request
            .get("preferences")
            .and_then(|prefs| prefs.get("hide_locked"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut chapters = Vec::new();
        let mut page = 1;
        loop {
            let body = fetch_document(
                &format!("{}?sort=latest&page={page}", url::join_url(BASE_URL, &key)),
                DETAILS_FIXTURE,
            );
            chapters.extend(parse_chapters(&body, hide_locked));
            if !body.contains("ebookSeries_paginationLink active ~")
                && !body.contains("pagination-btn--next")
                || page > 20
            {
                break;
            }
            page += 1;
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| r#"{"token":"fixture","id":"sample"}"#.to_string());
        if !key.starts_with('{') {
            return Ok(vec![manga::text_page(
                "Log in via WebView and purchase this chapter to read.",
            )]);
        }
        let chapter_id = serde_json::from_str::<ChapterId>(&key).unwrap_or(ChapterId {
            token: "fixture".into(),
            id: "sample".into(),
        });
        let body = client()
            .post(format!("{BASE_URL}/api/reader"))
            .form(&[("_token", &chapter_id.token), ("ebook_id", &chapter_id.id)])
            .header("X-Requested-With", "XMLHttpRequest")
            .send_text()
            .unwrap_or_else(|_| API_FIXTURE.to_string());
        let redirect = serde_json::from_str::<ApiResponse>(&body)
            .map(|response| response.redirect)
            .unwrap_or_else(|_| format!("{BASE_URL}/viewer?param=fixture&cgi=/viewer/cgi"));
        Ok(clipstudio_pages(
            &fetch_document(&redirect, VIEWER_FIXTURE),
            &redirect,
        ))
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let key = lazy_key(&request);
        if key.contains("mode=8") || key.contains("file=000") {
            let body = fetch_document(&key, PAGE_XML_FIXTURE);
            return Ok(MangaPageImage {
                url: image_url_from_page_xml(&key, &body),
                headers: manga::image_headers(BASE_URL),
                context: Some(manga::image_headers(BASE_URL)),
                ..MangaPageImage::default()
            });
        }
        Ok(MangaPageImage {
            url: key,
            headers: manga::image_headers(BASE_URL),
            context: Some(manga::image_headers(BASE_URL)),
            ..MangaPageImage::default()
        })
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let image_base64 = request
            .get("imageBase64")
            .or_else(|| request.get("image_base64"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("url"))
            .and_then(|url| url.get("url").or(Some(url)))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let processed = if url.contains("key=") {
            deobfuscate_base64(image_base64, url).unwrap_or_else(|| image_base64.to_string())
        } else if url.contains("size=") {
            unscramble_base64(image_base64, url).unwrap_or_else(|| image_base64.to_string())
        } else {
            image_base64.to_string()
        };
        Ok(ProcessedImage {
            image_base64: processed,
            mime_type: Some("image/jpeg".into()),
            ..ProcessedImage::default()
        })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            if key.starts_with('{') {
                BASE_URL.to_string()
            } else {
                url::join_url(BASE_URL, &key)
            }
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn parse_listing(body: &str, class_name: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .filter(|chunk| chunk.contains(class_name))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "seriesList_itemTitle", "href")
                .or_else(|| html::attr_after(chunk, "btn-search-result", "href"))
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "seriesList_itemTitle", "</a>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "series-list-img", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: body.contains("pagination-btn--next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/ebook/series/sample".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "ebook-series-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "series-list-img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "ebook-series-synopsis", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: text_values(body, "ebook-series-author"),
        tags: text_values(body, "book-genre"),
        status: ItemStatus::Unknown,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    body.split("shop-item--episode")
        .skip(1)
        .filter_map(|chunk| {
            let name = html::text_between(chunk, "shop-item-info-name", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".into());
            let date_uploaded = html::text_between(chunk, "shop-item-info-release", "</")
                .map(|value| html::strip_tags(&value).replace("公開：", ""))
                .and_then(|value| dates::parse_ymd(&value));
            if let Some(form) = html::text_between(chunk, "<form", "</form>") {
                let token = html::attr_after(&form, "name=\"_token\"", "value").unwrap_or_default();
                let id = html::attr_after(&form, "name=\"ebook_id\"", "value").unwrap_or_default();
                let key = serde_json::json!({ "token": token, "id": id }).to_string();
                return Some(MangaChapter {
                    key,
                    title: Some(name),
                    date_uploaded,
                    ..MangaChapter::default()
                });
            }
            if hide_locked {
                return None;
            }
            let rental_id = html::attr(chunk, "data-id").unwrap_or_default();
            Some(MangaChapter {
                key: format!("rental/{rental_id}"),
                title: Some(format!("Locked: {name}")),
                date_uploaded,
                is_locked: true,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn clipstudio_pages(body: &str, viewer_url: &str) -> Vec<MangaPage> {
    if let Some(content_id) = query_param(viewer_url, "c") {
        let token_body = client()
            .get(format!(
                "{BASE_URL}/api/tokens/viewer?content_id={content_id}"
            ))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| TOKEN_FIXTURE.to_string());
        let token = serde_json::from_str::<TokenResponse>(&token_body)
            .map(|response| response.token)
            .unwrap_or_else(|_| "fixture".into());
        let meta_body = client()
            .get(format!("{BASE_URL}/api/contents/{content_id}/meta"))
            .header("Authorization", format!("Bearer {token}"))
            .xhr()
            .send_text()
            .unwrap_or_else(|_| META_FIXTURE.to_string());
        let base = serde_json::from_str::<MetaResponse>(&meta_body)
            .map(|response| response.content.base_url)
            .unwrap_or_else(|_| "https://firecross.jp/content/sample/".into());
        return epub_pages(&base);
    }

    let authkey = query_param(viewer_url, "param")
        .or_else(|| html::attr_after(body, "name=\"param\"", "value"))
        .unwrap_or_else(|| "fixture".into());
    let endpoint = query_param(viewer_url, "cgi")
        .or_else(|| html::attr_after(body, "name=\"cgi\"", "value"))
        .unwrap_or_else(|| "/viewer/cgi".into());
    let base = url::join_url(BASE_URL, &endpoint);
    let face_url = format!(
        "{base}?mode=7&reqtype=0&vm=4&file=face.xml&param={}",
        url::query_escape(&authkey)
    );
    let face = fetch_document(&face_url, FACE_FIXTURE);
    let total = xml_value(&face, "TotalPage")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let grid_w = xml_value(&face, "Width").unwrap_or_else(|| "4".into());
    let grid_h = xml_value(&face, "Height").unwrap_or_else(|| "4".into());
    (0..total)
        .map(|index| {
            let file = format!("{index:04}.xml");
            let page_url = format!(
                "{base}?mode=8&reqtype=0&vm=4&file={file}&param={}#{grid_w}/{grid_h}",
                url::query_escape(&authkey)
            );
            MangaPage {
                content: PageContent::Lazy {
                    key: page_url,
                    url: None,
                    page_url: Some(viewer_url.to_string()),
                    context: None,
                },
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn epub_pages(content_base_url: &str) -> Vec<MangaPage> {
    let content_base = format!("{}/", content_base_url.trim_end_matches('/'));
    let prep = client()
        .get(format!("{content_base}preprocess-settings.json"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| PREPROCESS_FIXTURE.to_string());
    let key = serde_json::from_str::<PreprocessSettings>(&prep)
        .map(|value| value.obfuscate_image_key)
        .unwrap_or(0);
    let container = client()
        .get(format!("{content_base}META-INF/container.xml"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| CONTAINER_FIXTURE.to_string());
    let opf_path =
        html::attr_after(&container, "rootfile", "full-path").unwrap_or_else(|| "book.opf".into());
    let opf_url = url::join_url(&content_base, &opf_path);
    let opf_base = opf_url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/"))
        .unwrap_or_else(|| content_base.clone());
    let opf = client()
        .get(&opf_url)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| OPF_FIXTURE.to_string());
    let mut images = opf
        .split("<item")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("media-type=\"image/") || chunk.contains("media-type='image/")
        })
        .filter_map(|chunk| html::attr(chunk, "href"))
        .collect::<Vec<_>>();
    images.sort();
    if images.is_empty() {
        images.push("page1.jpg".into());
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, href)| MangaPage {
            content: PageContent::Url {
                url: format!("{}#key={key}", url::join_url(&opf_base, &href)),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn image_url_from_page_xml(page_xml_url: &str, body: &str) -> String {
    let endpoint = page_xml_url.split('?').next().unwrap_or(page_xml_url);
    let authkey = query_param(page_xml_url, "param").unwrap_or_else(|| "fixture".into());
    let page_index = xml_value(body, "PageNo").unwrap_or_else(|| "0".into());
    let scramble = xml_value(body, "Scramble");
    let number = body
        .split("<Kind")
        .nth(1)
        .and_then(|chunk| html::attr(chunk, "No"))
        .unwrap_or_else(|| "0".into());
    let kind = xml_value(body, "Kind").unwrap_or_else(|| "1".into());
    let file = format!(
        "{:04}_{:04}.bin",
        page_index.parse::<u32>().unwrap_or(0),
        number.parse::<u32>().unwrap_or(0)
    );
    let mut image = format!(
        "{endpoint}?mode={kind}&file={file}&reqtype=0&param={}",
        url::query_escape(&authkey)
    );
    if let Some(scramble) = scramble {
        if let Some(fragment) = page_xml_url.split('#').nth(1) {
            image.push_str("#size=");
            image.push_str(&scramble);
            image.push('/');
            image.push_str(fragment);
        }
    }
    image
}

fn deobfuscate_base64(input: &str, image_url: &str) -> Option<String> {
    let key = image_url
        .split("key=")
        .nth(1)?
        .split('&')
        .next()?
        .parse::<u8>()
        .ok()?;
    let mut bytes = STANDARD.decode(input).ok()?;
    for byte in bytes.iter_mut().take(1024) {
        *byte ^= key;
    }
    Some(STANDARD.encode(bytes))
}

fn unscramble_base64(input: &str, image_url: &str) -> Option<String> {
    let fragment = image_url.split("size=").nth(1)?;
    let mut parts = fragment.split('/');
    let mapping = parts
        .next()?
        .split(',')
        .filter_map(|value| value.parse::<u32>().ok())
        .collect::<Vec<_>>();
    let grid_w = parts.next()?.parse::<u32>().ok()?;
    let grid_h = parts.next()?.parse::<u32>().ok()?;
    let bytes = STANDARD.decode(input).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let result = unscramble_image(image, &mapping, grid_w, grid_h)?;
    let mut out = Vec::new();
    result
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
        .ok()?;
    Some(STANDARD.encode(out))
}

fn unscramble_image(
    image: DynamicImage,
    mapping: &[u32],
    grid_w: u32,
    grid_h: u32,
) -> Option<DynamicImage> {
    let (width, height) = image.dimensions();
    if mapping.len() < (grid_w * grid_h) as usize || width < 8 * grid_w || height < 8 * grid_h {
        return Some(image);
    }
    let piece_w = (width / grid_w) / 8 * 8;
    let piece_h = (height / grid_h) / 8 * 8;
    let mut result = DynamicImage::new_rgba8(width, height);
    for (dest, source) in mapping.iter().enumerate() {
        let dx = (dest as u32 % grid_w) * piece_w;
        let dy = (dest as u32 / grid_w) * piece_h;
        let sx = (source % grid_w) * piece_w;
        let sy = (source / grid_w) * piece_h;
        let tile = image.crop_imm(sx, sy, piece_w, piece_h);
        result.copy_from(&tile, dx, dy).ok()?;
    }
    Some(result)
}

fn lazy_key(request: &Value) -> String {
    request
        .get("page")
        .and_then(|page| page.get("content"))
        .and_then(|content| content.get("lazy"))
        .and_then(|lazy| lazy.get("key"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn text_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, "<a", "</a>")
                .or_else(|| html::text_between(chunk, "<li", "</li>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn query_param(input: &str, name: &str) -> Option<String> {
    for part in input.split('?').nth(1)?.split('#').next()?.split('&') {
        let (key, value) = part.split_once('=')?;
        if key == name {
            return Some(value.replace("%2B", "+"));
        }
    }
    None
}

fn xml_value(input: &str, name: &str) -> Option<String> {
    html::text_between(input, &format!("<{name}"), &format!("</{name}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!(
        "/{}",
        path.split('?')
            .next()
            .unwrap_or(path)
            .trim_start_matches('/')
            .trim_end_matches('/')
    )
}

#[derive(Deserialize)]
struct ChapterId {
    token: String,
    id: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    redirect: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
}

#[derive(Deserialize)]
struct MetaResponse {
    content: MetaContent,
}

#[derive(Deserialize)]
struct MetaContent {
    #[serde(rename = "baseUrl")]
    base_url: String,
}

#[derive(Deserialize)]
struct PreprocessSettings {
    #[serde(rename = "obfuscateImageKey")]
    obfuscate_image_key: u8,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<ul class="seriesList"><li class="seriesList_item"><a class="seriesList_itemTitle" href="/ebook/series/sample">Sample FireCross</a><img class="series-list-img" src="/cover.jpg"></li></ul>"#;
const SEARCH_FIXTURE: &str = LIST_FIXTURE;
const DETAILS_FIXTURE: &str = r#"<h1 class="ebook-series-title">Sample FireCross</h1><p class="ebook-series-synopsis">Fixture description.</p><div class="shop-item--episode"><span class="shop-item-info-name">Chapter 1</span><span class="shop-item-info-release">公開：2024/1/1</span><form data-api="reader"><input name="_token" value="fixture"><input name="ebook_id" value="sample"></form></div>"#;
const API_FIXTURE: &str =
    r#"{"redirect":"https://firecross.jp/viewer?param=fixture&cgi=/viewer/cgi"}"#;
const VIEWER_FIXTURE: &str = r#"<div id="meta"><input name="param" value="fixture"><input name="cgi" value="/viewer/cgi"></div>"#;
const FACE_FIXTURE: &str =
    r#"<TotalPage>1</TotalPage><Scramble><Width>4</Width><Height>4</Height></Scramble>"#;
const PAGE_XML_FIXTURE: &str =
    r#"<PageNo>0</PageNo><Scramble>0,1,2,3</Scramble><Kind No="0" scramble="0">1</Kind>"#;
const TOKEN_FIXTURE: &str = r#"{"token":"fixture"}"#;
const META_FIXTURE: &str = r#"{"content":{"baseUrl":"https://firecross.jp/content/sample/"}}"#;
const PREPROCESS_FIXTURE: &str = r#"{"obfuscateImageKey":0}"#;
const CONTAINER_FIXTURE: &str =
    r#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#;
const OPF_FIXTURE: &str =
    r#"<package><manifest><item href="page1.jpg" media-type="image/jpeg"/></manifest></package>"#;
