use aes::Aes128;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cbc::{
    cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit},
    Decryptor,
};
use manatan_extension::{
    abi, abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem,
    ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage, SearchRequest,
    UrlResolveResult,
};
use manatan_shared::{html, manga, url};
use serde_json::{json, Value};
use std::collections::BTreeMap;

type Aes128CbcDec = Decryptor<Aes128>;

const SOURCE: FavComic = FavComic;
const DEFAULT_BASE_URL: &str = "https://www.favcomic.com";
const IMAGE_KEY_B64: &str = "NlgrYjYuRT5ic1hifSs9Tg==";

struct FavComic;

impl MangaSource for FavComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let body = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let manga_type = preference(&request, "mangaType", "boy-1")
                .split('-')
                .next()
                .unwrap_or("boy")
                .to_string();
            fetch(
                &format!("{base}/{manga_type}?page={}", page(&request)),
                &base,
                LIST_FIXTURE,
            )
        } else {
            let comic_type = preference(&request, "mangaType", "boy-1")
                .split('-')
                .nth(1)
                .unwrap_or("1")
                .to_string();
            fetch(
                &format!(
                    "{base}/rank?range={}&comicType={comic_type}&vip=0",
                    preference(&request, "rankType", "1")
                ),
                &base,
                RANK_FIXTURE,
            )
        };
        Ok(parse_listing(&body, &base))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(&base) {
            let key = normalize_key(query, &base);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(query, &base, DETAILS_FIXTURE),
                    &key,
                    &base,
                )],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let manga_type = filters
            .get("mangaType")
            .and_then(Value::as_str)
            .unwrap_or("search");
        let target =
            format!(
            "{base}/{manga_type}?keyword={}&origin={}&finished={}&free={}&sort={}&page={}&tag={}",
            url::query_escape(query),
            filters.get("origin").and_then(Value::as_str).unwrap_or("0"),
            filters.get("finished").and_then(Value::as_str).unwrap_or("0"),
            filters.get("free").and_then(Value::as_str).unwrap_or("0"),
            filters.get("sort").and_then(Value::as_str).unwrap_or("1"),
            page(&request),
            filters.get("tag").and_then(Value::as_str).unwrap_or("")
        );
        Ok(parse_listing(&fetch(&target, &base, LIST_FIXTURE), &base))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_details(
            &fetch(&url::join_url(&base, &key), &base, DETAILS_FIXTURE),
            &key,
            &base,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_chapters(
            &fetch(&url::join_url(&base, &key), &base, DETAILS_FIXTURE),
            &base,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/1".to_string());
        let target = url::join_url(&base, &key);
        let body = fetch(&target, &base, PAGES_FIXTURE);
        match chapter_error(&body) {
            Some(message) => Err(abi::ExtensionError { message }),
            None => Ok(parse_pages(&body, &target, &base)),
        }
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        if !request
            .get("page")
            .and_then(|p| p.get("extra"))
            .and_then(|e| e.get("favEncrypted"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return passthrough(request);
        }
        let input = request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(bytes) = STANDARD.decode(input).ok().filter(|v| v.len() > 16) else {
            return passthrough(request);
        };
        let Some(key) = STANDARD.decode(IMAGE_KEY_B64).ok() else {
            return passthrough(request);
        };
        let iv = &bytes[..16];
        let cipher = &bytes[16..];
        let Some(plain) = Aes128CbcDec::new_from_slices(&key, iv)
            .ok()
            .and_then(|d| d.decrypt_padded_vec_mut::<Pkcs7>(cipher).ok())
        else {
            return passthrough(request);
        };
        Ok(ProcessedImage {
            image_base64: STANDARD.encode(plain),
            mime_type: request
                .get("mimeType")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            ..ProcessedImage::default()
        })
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(&base, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let base = base_url(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(&base, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        let base = base_url(&request);
        if input.starts_with(&base) {
            let key = normalize_key(input, &base);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch(input, &base, DETAILS_FIXTURE),
                    &key,
                    &base,
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

fn client(base: &str) -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{base}/"))
        .with_cookies_for(base)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, base: &str, fixture: &str) -> String {
    client(base)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_url(request: &Value) -> String {
    preference(request, "baseUrl", DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn preference(request: &Value, key: &str, default: &str) -> String {
    request
        .get("preferences")
        .and_then(|p| p.get(key))
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn normalize_key(input: &str, base: &str) -> String {
    format!(
        "/{}",
        input
            .strip_prefix(base)
            .unwrap_or(input)
            .split('?')
            .next()
            .unwrap_or(input)
            .trim_matches('/')
    )
}

fn parse_listing(body: &str, base: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|c| c.contains("cover") || c.contains("rank_item"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if href == "#" || href.starts_with("javascript") {
                return None;
            }
            let key = normalize_key(&href, base);
            let img = chunk.split("<img").nth(1).unwrap_or(chunk);
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<a", "title")
                    .or_else(|| html::attr_after(img, "<img", "alt"))
                    .or_else(|| {
                        html::text_between(chunk, "title", "</").map(|v| html::strip_tags(&v))
                    })
                    .unwrap_or_else(|| "喜漫漫画".to_string()),
                cover: html::attr_after(img, "<img", "data-src")
                    .or_else(|| html::attr_after(img, "<img", "src"))
                    .map(|i| clean_image_url(&url::join_url(base, &i)).0),
                authors: html::text_between(chunk, "author", "</")
                    .map(|v| vec![html::strip_tags(&v)])
                    .unwrap_or_default(),
                description: html::text_between(chunk, "brief", "</").map(|v| html::strip_tags(&v)),
                url: Some(url::join_url(base, &key)),
                language: Some("zh".to_string()),
                content_rating: Some("adult".to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination_box") && !body.contains("active\">下一"),
    }
}

fn parse_details(body: &str, key: &str, base: &str) -> CatalogItem {
    let img = body
        .split("comic_cover_box")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .nth(1)
        .unwrap_or(body);
    let (cover, _) = html::attr_after(img, "<img", "data-src")
        .or_else(|| html::attr_after(img, "<img", "src"))
        .map(|i| clean_image_url(&url::join_url(base, &i)))
        .unwrap_or((String::new(), false));
    CatalogItem {
        key: normalize_key(key, base),
        title: html::text_between(body, "comic_title", "</")
            .map(|v| html::strip_tags(&v))
            .unwrap_or_else(|| "喜漫漫画".to_string()),
        cover: if cover.is_empty() { None } else { Some(cover) },
        authors: html::text_between(body, "author", "</")
            .map(|v| vec![html::strip_tags(&v)])
            .unwrap_or_default(),
        artists: html::text_between(body, "author", "</")
            .map(|v| vec![html::strip_tags(&v)])
            .unwrap_or_default(),
        tags: body
            .split("tag_box")
            .nth(1)
            .unwrap_or(body)
            .split("<a")
            .skip(1)
            .filter_map(|p| html::text_between(p, ">", "</a>"))
            .map(|v| html::strip_tags(&v))
            .filter(|v| !v.is_empty())
            .collect(),
        description: html::text_between(body, "intro_box", "</div>")
            .map(|v| html::strip_tags(&v).replace("作品介绍：", "")),
        status: if body.contains("完结") {
            ItemStatus::Completed
        } else if body.contains("连载中") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(base, key)),
        language: Some("zh".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("catalog_box")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href, base);
            let note = html::text_between(chunk, "span:last-child", "</span>")
                .and_then(|v| html::strip_tags(&v).parse::<f32>().ok());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "title", "</")
                        .map(|v| html::strip_tags(&v))
                        .unwrap_or_else(|| html::strip_tags(chunk)),
                ),
                scanlators: note.map(|value| format!("￥{value}")).into_iter().collect(),
                url: Some(url::join_url(base, &key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, referer: &str, base: &str) -> Vec<MangaPage> {
    body.split("#content")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(|chunk| {
            let image = html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src"))?;
            let encrypted = chunk.contains("encrypted-image") || image.ends_with("#true");
            let (clean, hash_encrypted) = clean_image_url(&url::join_url(base, &image));
            Some((clean, encrypted || hash_encrypted))
        })
        .enumerate()
        .map(|(index, (image, encrypted))| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            extra: BTreeMap::from([("favEncrypted".to_string(), json!(encrypted))]),
            ..MangaPage::default()
        })
        .collect()
}

fn chapter_error(body: &str) -> Option<String> {
    let code = html::attr_after(body, "comic_chapter_box", "code")?;
    match code.as_str() {
        "1" => Some("此话需在 WebView 中登录才能看".to_string()),
        "3" => Some("金币不足，请充值".to_string()),
        "4" => Some("请在 WebView 中付费解锁此话".to_string()),
        "444" => Some("免费额度已用完，明天零点重置".to_string()),
        _ => None,
    }
}

fn clean_image_url(input: &str) -> (String, bool) {
    let encrypted = input.rsplit('#').next() == Some("true");
    (
        input.split('#').next().unwrap_or(input).to_string(),
        encrypted,
    )
}

fn passthrough(request: Value) -> ExtensionResult<ProcessedImage> {
    Ok(ProcessedImage {
        image_base64: request
            .get("imageBase64")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        mime_type: request
            .get("mimeType")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        ..ProcessedImage::default()
    })
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

const RANK_FIXTURE: &str = r#"<div class="rank_item"><a href="/comic/sample"><div class="cover"><img data-src="/cover.jpg" alt="Sample FavComic"></div><span class="author">Author</span><span class="brief">Summary</span></a></div>"#;
const LIST_FIXTURE: &str = r#"<div class="cover_box"><a href="/comic/sample" title="Sample FavComic"><img class="cover" data-src="/cover.jpg"></a></div>"#;
const DETAILS_FIXTURE: &str = r#"<div class="comic_cover_box"><div class="flex_box"><img data-src="/cover.jpg"></div></div><h1 class="comic_title">Sample FavComic</h1><span class="author">Author</span><div class="state_box"><span></span><span>连载中</span></div><div class="tag_box"><a>Action</a></div><div class="intro_box"><div class="txt">作品介绍：Summary</div></div><div class="catalog_box"><a href="/comic/sample/1"><span class="title">Chapter 1</span></a></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="comic_chapter_box" code="0"></div><div id="content"><img data-src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
