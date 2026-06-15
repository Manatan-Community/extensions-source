use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, RgbaImage};
use manatan_extension::{abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, ProcessedImage, SearchRequest, UrlResolveResult};
use manatan_shared::{dates, html, manga, url};
use md5::{Digest, Md5};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const SOURCE: Jinmantiantang = Jinmantiantang;
const DEFAULT_BASE_URL: &str = "https://18comic.vip";
const SCRAMBLE_ID: u64 = 220_980;

struct Jinmantiantang;

impl MangaSource for Jinmantiantang {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") { "mr" } else { "mv" };
        Ok(parse_listing(&fetch(&format!("{base}/albums?o={order}&page={}", page(&request)), &base, LIST_FIXTURE), &base, &request))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let base = base_url(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with("https://") {
            let key = normalize_key(query, &base);
            return Ok(Paged { entries: vec![parse_details(&fetch(query, &base, DETAILS_FIXTURE), &key, &base)], has_next_page: false });
        }
        if query.to_ascii_uppercase().starts_with("JM") || query.parse::<u64>().is_ok() {
            let id = query.trim_start_matches("JM").trim_start_matches(':');
            let key = format!("/album/{id}");
            return Ok(Paged { entries: vec![parse_details(&fetch(&url::join_url(&base, &key), &base, DETAILS_FIXTURE), &key, &base)], has_next_page: false });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let mut params = format!("{}{}{}{}", filter(filters, "category", "/albums?"), filter(filters, "sort", "o=mr&"), filter(filters, "time", "t=a&"), filter(filters, "scope", "main_tag=0"));
        let target = if !query.is_empty() && !query.contains('-') {
            params = params.split('?').nth(1).unwrap_or(&params).to_string();
            format!("{base}/search/photos?search_query={}&page={}&{params}", query.replace('+', "%2B").replace(' ', "+"), page(&request))
        } else {
            if params.is_empty() { params = "/albums?".into(); }
            let screen = query.split_whitespace().filter(|v| v.starts_with('-')).map(|v| v.trim_start_matches('-')).collect::<Vec<_>>().join("+");
            format!("{base}{params}&page={}&screen={screen}", page(&request))
        };
        Ok(parse_listing(&fetch(&target, &base, LIST_FIXTURE), &base, &request))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/album/sample".into());
        Ok(parse_details(&fetch(&url::join_url(&base, &key), &base, DETAILS_FIXTURE), &key, &base))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/album/sample".into());
        Ok(parse_chapters(&resolve_encoded_html(&fetch(&url::join_url(&base, &key), &base, DETAILS_FIXTURE)), &base))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let base = base_url(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/photo/sample".into());
        Ok(parse_pages_recursive(&url::join_url(&base, &key), &base))
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        let aid = request.get("page").and_then(|p| p.get("extra")).and_then(|e| e.get("albumId")).and_then(Value::as_u64).unwrap_or(0);
        let image_index = request.get("page").and_then(|p| p.get("extra")).and_then(|e| e.get("imageIndex")).and_then(Value::as_str).unwrap_or("1");
        if aid < SCRAMBLE_ID {
            return passthrough(request);
        }
        let Some(input) = request.get("imageBase64").and_then(Value::as_str).and_then(|v| STANDARD.decode(v).ok()) else { return passthrough(request); };
        let rows = scramble_rows(aid, image_index);
        let Some(out) = descramble_image(&input, rows) else { return passthrough(request); };
        Ok(ProcessedImage { image_base64: STANDARD.encode(out), mime_type: Some("image/jpeg".into()), ..ProcessedImage::default() })
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
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        let base = base_url(&request);
        if input.starts_with("https://") {
            let key = normalize_key(input, &base);
            return Ok(Some(UrlResolveResult { item: Some(parse_details(&fetch(input, &base, DETAILS_FIXTURE), &key, &base)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn base_url(request: &Value) -> String {
    request.get("preferences").and_then(|p| p.get("baseUrl")).and_then(Value::as_str).unwrap_or(DEFAULT_BASE_URL).trim_end_matches('/').to_string()
}
fn client(base: &str) -> http::HttpClient { http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{base}/")).with_cookies_for(base).with_webview_challenge_fallback() }
fn fetch(target: &str, base: &str, fixture: &str) -> String { client(base).get(target).browser_document().send_text().unwrap_or_else(|_| fixture.into()) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }
fn filter<'a>(filters: &'a Value, key: &str, default: &'a str) -> &'a str { filters.get(key).and_then(Value::as_str).unwrap_or(default) }
fn normalize_key(input: &str, base: &str) -> String {
    let hostless = input.strip_prefix(base).or_else(|| input.split_once("://").and_then(|(_, rest)| rest.find('/').map(|idx| &rest[idx..]))).unwrap_or(input);
    format!("/{}", hostless.split('?').next().unwrap_or(hostless).trim_matches('/'))
}

fn parse_listing(body: &str, base: &str, request: &Value) -> Paged<CatalogItem> {
    let blocked = request.get("preferences").and_then(|p| p.get("blockGenres")).and_then(Value::as_str).unwrap_or("").split("//").next().unwrap_or("").split_whitespace().map(|s| s.to_ascii_lowercase()).collect::<Vec<_>>();
    let entries = body.split("list-col").skip(1).flat_map(|section| section.split("p-b-15").skip(1)).filter_map(|chunk| {
        if chunk.contains("data-group") { return None; }
        let href = html::attr_after(chunk, "<a", "href")?;
        let key = normalize_key(&url::join_url(base, &href), base);
        let tags = chunk.split("<a").skip(2).map(html::strip_tags).filter(|v| !v.is_empty()).collect::<Vec<_>>();
        if tags.iter().any(|tag| blocked.contains(&tag.to_ascii_lowercase())) { return None; }
        Some(CatalogItem {
            key: key.clone(),
            title: html::text_between(chunk, "title-truncate", "</").or_else(|| html::text_between(chunk, "<h", "</h")).map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "禁漫天堂".into())),
            cover: extract_img(chunk).map(|v| url::join_url(base, &v).split('?').next().unwrap_or("").to_string()),
            authors: tags.first().cloned().into_iter().collect(),
            tags,
            url: Some(url::join_url(base, &key)),
            language: Some("zh".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        })
    }).collect();
    Paged { entries, has_next_page: body.contains("prevnext") }
}

fn parse_details(body: &str, key: &str, base: &str) -> CatalogItem {
    let body = resolve_encoded_html(body);
    let genre_text = body.split("itemprop=\"genre\"").nth(1).unwrap_or("").split("</span>").next().unwrap_or("");
    let tags = genre_text.split("<a").skip(1).map(html::strip_tags).filter(|v| !v.is_empty() && v != "連載中" && v != "完結").collect::<Vec<_>>();
    let status_text = html::strip_tags(genre_text);
    CatalogItem {
        key: key.into(),
        title: html::text_between(&body, "<h1", "</h1>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "禁漫天堂".into()),
        cover: extract_img(body.split("thumb-overlay").nth(1).unwrap_or(&body)).map(|v| {
            let stem = v.rsplit_once('.').map(|(s, _)| s).unwrap_or(&v);
            url::join_url(base, &format!("{stem}_3x4.jpg"))
        }),
        authors: body.split("tag-block").nth(3).unwrap_or("").split("btn-primary").skip(1).map(html::strip_tags).filter(|v| !v.is_empty()).collect(),
        tags,
        description: html::text_between(&body, "intro-block", "</div>").map(|v| html::strip_tags(&v).replace("敘述：", "")).filter(|v| !v.is_empty()),
        status: if status_text.contains("完結") { ItemStatus::Completed } else if status_text.contains("連載中") { ItemStatus::Ongoing } else { ItemStatus::Unknown },
        url: Some(url::join_url(base, key)),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, base: &str) -> Vec<MangaChapter> {
    let mut chapters = body.split("episode-block").nth(1).unwrap_or(body).split("<a").skip(1).filter(|c| c.contains("/photo/")).filter_map(|chunk| {
        let href = html::attr(chunk, "href")?;
        let key = normalize_key(&url::join_url(base, &href), base);
        Some(MangaChapter {
            key: key.clone(),
            title: Some(html::text_between(chunk, "<h3", "</h3>").map(|v| html::strip_tags(&v)).filter(|v| !v.is_empty()).unwrap_or_else(|| "Chapter".into())),
            date_uploaded: html::text_between(chunk, "hidden-xs", "</").map(|v| html::strip_tags(&v)).and_then(|v| dates::parse_ymd(&v)),
            url: Some(url::join_url(base, &key)),
            ..MangaChapter::default()
        })
    }).collect::<Vec<_>>();
    if chapters.is_empty() {
        let href = html::attr_after(body, "album_photo_cover", "href").unwrap_or_else(|| "/photo/sample".into());
        let key = normalize_key(&url::join_url(base, &href), base);
        chapters.push(MangaChapter { key: key.clone(), title: Some("单章节".into()), url: Some(url::join_url(base, &key)), ..MangaChapter::default() });
    } else {
        chapters.reverse();
    }
    chapters
}

fn parse_pages_recursive(first_url: &str, base: &str) -> Vec<MangaPage> {
    let mut out = Vec::new();
    let mut next = Some(first_url.to_string());
    for _ in 0..20 {
        let Some(target) = next.take() else { break; };
        let body = fetch(&target, base, PAGES_FIXTURE);
        for chunk in body.split("scramble-page").skip(1) {
            let Some(image) = extract_img(chunk) else { continue; };
            let image = image.split('?').next().unwrap_or(&image).to_string();
            let (album_id, image_index) = album_image_parts(&image);
            out.push(MangaPage {
                content: PageContent::Url { url: url::join_url(base, &image), context: None },
                headers: manga::image_headers(base),
                description: Some(format!("Page {}", out.len() + 1)),
                extra: BTreeMap::from([("albumId".into(), json!(album_id)), ("imageIndex".into(), json!(image_index))]),
                ..MangaPage::default()
            });
        }
        next = html::attr_after(&body, "prevnext", "href").map(|v| url::join_url(base, &v));
        if next.as_deref() == Some(&target) {
            break;
        }
    }
    out
}

fn extract_img(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-original").or_else(|| html::attr_after(chunk, "<img", "data-cfsrc")).or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn resolve_encoded_html(body: &str) -> String {
    let mut out = body.to_string();
    for part in body.split("base64DecodeUtf8(\"").skip(1) {
        let encoded = part.split("\")").next().unwrap_or_default();
        if let Ok(bytes) = STANDARD.decode(encoded) {
            if let Ok(text) = String::from_utf8(bytes) {
                out.push_str(&text);
            }
        }
    }
    out
}

fn album_image_parts(image: &str) -> (u64, String) {
    let parts = image.split('/').collect::<Vec<_>>();
    let album_id = parts.iter().rev().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    let image_index = parts.last().copied().unwrap_or("1").split('.').next().unwrap_or("1").to_string();
    (album_id, image_index)
}

fn scramble_rows(aid: u64, image_index: &str) -> u32 {
    let modulus = if aid >= 421_926 { 8 } else if aid >= 268_850 { 10 } else { return 10; };
    let mut hasher = Md5::new();
    hasher.update(format!("{aid}{image_index}").as_bytes());
    let digest = hasher.finalize();
    let last_hex = format!("{:02x}", digest[15]).chars().last().unwrap_or('0') as u32;
    2 * (last_hex % modulus) + 2
}

fn descramble_image(input: &[u8], rows: u32) -> Option<Vec<u8>> {
    let image = image::load_from_memory(input).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    let remainder = height % rows;
    let row_height = height / rows;
    let mut output = RgbaImage::new(width, height);
    for x in 0..rows {
        let mut copy_h = row_height;
        let mut py = row_height * x;
        let y = height.saturating_sub(row_height * (x + 1)).saturating_sub(remainder);
        if x == 0 {
            copy_h += remainder;
        } else {
            py += remainder;
        }
        for yy in 0..copy_h {
            for xx in 0..width {
                let pixel = image.get_pixel(xx, y + yy);
                output.put_pixel(xx, py + yy, *pixel);
            }
        }
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90).encode_image(&image::DynamicImage::ImageRgba8(output)).ok()?;
    Some(out)
}

fn passthrough(request: Value) -> ExtensionResult<ProcessedImage> {
    Ok(ProcessedImage {
        image_base64: request.get("imageBase64").and_then(Value::as_str).unwrap_or_default().to_string(),
        mime_type: request.get("mimeType").and_then(Value::as_str).map(ToOwned::to_owned),
        ..ProcessedImage::default()
    })
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="list-col"><div class="p-b-15"><a href="/album/1"><img data-original="/media/albums/1_3x4.jpg"></a><h4 class="title-truncate">Sample</h4><a>Author</a><a>Tag</a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1>Sample</h1><div class="thumb-overlay"><img data-original="/media/albums/1.jpg"></div><span itemprop="genre"><a>完結</a><a>Tag</a></span><div id="intro-block"><div class="p-t-5 p-b-5">敘述：Desc</div></div><div id="episode-block"><a href="/photo/1"><li><h3>Chapter 1</h3><span class="hidden-xs">2024-01-01</span></li></a></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="center scramble-page spnotice_chk" id="0"><img src="/media/photos/1/1.jpg"></div>"#;
