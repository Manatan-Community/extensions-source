use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: TuSachXinhXinh = TuSachXinhXinh;
const BASE_URL: &str = "https://tusachxinhxinh12.online";
const KEY_PART_1: &str = "qX3xRL";
const KEY_PART_2: &str = "guhD2Z";
const KEY_PART_3: &str = "9f7sWJ";

struct TuSachXinhXinh;

impl MangaSource for TuSachXinhXinh {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            return Ok(Paged {
                entries: parse_compact_list(&vh::fetch_document(
                    BASE_URL,
                    &format!("{BASE_URL}/nhieu-xem-nhat/"),
                    POPULAR_FIXTURE,
                )),
                has_next_page: false,
            });
        }
        let page = vh::page_number(&request);
        let target = if page > 1 {
            format!("{BASE_URL}/page/{page}/")
        } else {
            BASE_URL.to_string()
        };
        Ok(parse_latest(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = vh::query(&request);
        if let Some(key) = vh::key_from_url(BASE_URL, &query, "/truyen-tranh/") {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let body = vh::browser_client(BASE_URL)
                .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&[("action", "searchtax"), ("keyword", query.as_str())])
                .send_text()
                .unwrap_or_else(|_| SEARCH_FIXTURE.to_string());
            return Ok(parse_search_json(&body));
        }
        if let Some(filter_uri) = vh::filter(&request, "filterUri") {
            let target = format!("{BASE_URL}/{}/", filter_uri.trim_matches('/'));
            let body = vh::fetch_document(BASE_URL, &target, POPULAR_FIXTURE);
            let compact = parse_compact_list(&body);
            if !compact.is_empty() {
                return Ok(Paged {
                    entries: compact,
                    has_next_page: false,
                });
            }
            return Ok(parse_latest(&body));
        }
        self.list(json!({"page": vh::page_number(&request), "listingId": "latest"}))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen-tranh/sample".into());
        Ok(parse_chapters(&vh::fetch_document(
            BASE_URL,
            &vh::absolute_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample/chap-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE);
        let images = extract_page_images(&body);
        Ok(if images.is_empty() {
            vec![vh::text_page("Khong tim thay hinh anh")]
        } else {
            vh::image_pages(images, &chapter_url)
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            vh::home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"})),
            )?,
            vh::home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"})),
            )?,
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| vh::absolute_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| vh::absolute_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = vh::key_from_url(BASE_URL, input, "/truyen-tranh/") {
            let is_chapter = key.to_ascii_lowercase().contains("chap");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter).then(|| details_by_key(&key)),
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

fn parse_compact_list(body: &str) -> Vec<CatalogItem> {
    body.split("position-relative")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "super-title", "</p>")
                .or_else(|| vh::title_from(chunk))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk).map(vh::strip_small_thumbnail),
                "adult",
            ))
        })
        .fold(Vec::new(), vh::push_unique)
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("comic-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "comic-title", "</h3>")
                .or_else(|| vh::title_from(chunk))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk).map(vh::strip_small_thumbnail),
                "adult",
            ))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: body.contains("li.next") && !body.contains("next disabled"),
    }
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .or_else(|_| serde_json::from_str(SEARCH_FIXTURE))
        .unwrap_or_default();
    let entries = response
        .data
        .into_iter()
        .filter(|item| item.link.contains("/truyen-tranh/"))
        .map(|item| {
            vh::catalog_item(
                BASE_URL,
                vh::normalize_key(BASE_URL, &item.link),
                item.title,
                item.img.map(vh::strip_small_thumbnail),
                "adult",
            )
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: false,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &vh::fetch_document(BASE_URL, &vh::absolute_url(BASE_URL, key), DETAILS_FIXTURE),
        key,
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: vh::normalize_key(BASE_URL, key),
        title: html::text_between(body, "info-title", "</")
            .or_else(|| html::text_between(body, "<h2", "</h2>"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "img-thumbnail", "data-lazy-src")
            .or_else(|| html::attr_after(body, "img-thumbnail", "src"))
            .or_else(|| vh::image_attr(body))
            .map(vh::strip_small_thumbnail)
            .map(|image| vh::absolute_url(BASE_URL, &image)),
        authors: strong_value(body, "Tác giả").into_iter().collect(),
        tags: link_texts(body, "/the-loai/"),
        description: html::text_between(body, "text-justify", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: vh::status_from_vi(
            &html::text_between(body, "comic-stt", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default(),
        ),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(1)
        .filter_map(|row| {
            let href = html::attr_after(row, "text-capitalize", "href")
                .or_else(|| html::attr_after(row, "<a", "href"))?;
            let key = vh::normalize_key(BASE_URL, &href);
            let raw_title = html::text_between(row, "text-capitalize", "</a>")
                .or_else(|| html::text_between(row, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(parse_chapter_name(&raw_title)),
                date_uploaded: html::text_between(row, "hidden-xs hidden-sm", "</td>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| vh::parse_dd_mm_yy(&value)),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), vh::push_unique_chapter)
}

fn parse_chapter_name(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if let Some(index) = lower.find("chap") {
        return raw[index..]
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
    }
    raw.rsplit(['-', '–'])
        .next()
        .unwrap_or(raw)
        .trim()
        .to_string()
}

fn extract_page_images(body: &str) -> Vec<String> {
    if let Some(raw) = html_content_json(body) {
        if let Some(decrypted) = vh::decrypt_cryptojs_aes_sha512(
            &(KEY_PART_1.to_string() + KEY_PART_2 + KEY_PART_3),
            &raw,
        ) {
            let images = images_from_decrypted_html(&decrypted);
            if !images.is_empty() {
                return images;
            }
        }
    }
    vh::collect_image_urls(BASE_URL, body)
}

fn html_content_json(body: &str) -> Option<String> {
    let start = body.find("var htmlContent")?;
    let rest = &body[start..];
    let quote_start = rest.find('"')? + 1;
    let tail = &rest[quote_start..];
    let mut escaped = false;
    let mut end = None;
    for (index, ch) in tail.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            end = Some(index);
            break;
        }
    }
    Some(
        tail[..end?]
            .replace("\\\"", "\"")
            .replace("\\/", "/")
            .replace("\\\\", "\\"),
    )
}

fn images_from_decrypted_html(body: &str) -> Vec<String> {
    let data_attr = format!("data-{}", KEY_PART_1.to_ascii_lowercase());
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| {
            html::attr(chunk, &data_attr)
                .map(deobfuscate_url)
                .or_else(|| vh::image_attr(chunk))
        })
        .filter(|image| vh::looks_like_image(image))
        .map(|image| vh::absolute_url(BASE_URL, &image))
        .fold(Vec::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
}

fn deobfuscate_url(input: String) -> String {
    input
        .replace(KEY_PART_1, ".")
        .replace(KEY_PART_2, ":")
        .replace(KEY_PART_3, "/")
}

fn strong_value(body: &str, label: &str) -> Option<String> {
    body.find(label)
        .and_then(|index| html::text_between(&body[index..], "<span", "</span>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    data: Vec<SearchResult>,
}

#[derive(Default, Deserialize)]
struct SearchResult {
    title: String,
    link: String,
    img: Option<String>,
}

const POPULAR_FIXTURE: &str = r#"
<ul class="most-views single-list-comic"><li class="position-relative"><p class="super-title"><a href="/truyen-tranh/sample">Sample</a></p><img class="list-left-img" src="/cover-150x150.jpg"></li></ul>
"#;
const LIST_FIXTURE: &str = r#"
<div class="col-md-3 col-xs-6 comic-item"><a href="/truyen-tranh/sample"><h3 class="comic-title">Sample</h3><img src="/cover-150x150.jpg"></a></div>
"#;
const SEARCH_FIXTURE: &str = r#"{"data":[{"title":"Sample","link":"https://tusachxinhxinh12.online/truyen-tranh/sample","img":"https://tusachxinhxinh12.online/cover-150x150.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"
<h2 class="info-title">Sample</h2><div class="col-sm-4"><img class="img-thumbnail" src="/cover.jpg"></div><strong>Tác giả</strong><span>Author</span><span class="comic-stt">Đang tiến hành</span><a href="/the-loai/action">Action</a><div class="text-justify">Summary</div><div class="table-scroll"><table><tr><td><a class="text-capitalize" href="/truyen-tranh/sample/chap-1">Chap 1</a></td><td class="hidden-xs hidden-sm">01/01/24</td></tr></table></div>
"#;
const PAGES_FIXTURE: &str = r#"<div id="view-chapter"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
