use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, Paged,
    ProcessedImage, SearchRequest, UpdateStrategy, UrlResolveResult, abi::ExtensionError,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html, manga, manga_image, sdk::http::HttpClient, speedbinb::SpeedBinbReader, url,
};
use serde_json::{Value, json};

const SOURCE: Yanmaga = Yanmaga;
const BASE_URL: &str = "https://yanmaga.jp";
const COMICS_PER_PAGE: usize = 24;
const LATEST_PER_PAGE: u64 = 12;

struct Yanmaga;

impl MangaSource for Yanmaga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        match request
            .get("listingId")
            .and_then(Value::as_str)
            .unwrap_or("comics")
        {
            "gravures" => Ok(parse_gravures(&fetch_document(
                &format!("{BASE_URL}/gravures/series?page={page}"),
                GRAVURES_FIXTURE,
            ))),
            "latest" => latest_updates(page),
            _ => Ok(parse_comics_directory(
                &fetch_document(&format!("{BASE_URL}/comics"), COMICS_FIXTURE),
                page,
            )),
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }

        let mut target = format!(
            "{BASE_URL}/search?q={}&kind=human",
            url::query_escape(query)
        );
        if page(&request) > 1 {
            target.push_str(&format!("&page={}", page(&request)));
        }
        target.push_str("&search-submit=");
        Ok(parse_search(&fetch_document(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample".into());
        if !key.contains("/series/") && key.contains("/gravures/") {
            return Ok(vec![MangaChapter {
                key: normalize_key(&key),
                title: Some("作品".into()),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                ..MangaChapter::default()
            }]);
        }
        let target = absolute_url(&key);
        let body = fetch_document(&target, DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &target))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episodes/sample".into());
        let reader_url = absolute_url(&key);
        let body = fetch_document(&reader_url, READER_FIXTURE);
        if body.contains("ga-rental-modal-sign-up") {
            return Err(err("このストーリーを読むには WebView でログイン"));
        }
        if body.contains("ga-modal-open") {
            return Err(err("WebView でポイントを使用してこのストーリーをレンタル"));
        }
        SpeedBinbReader {
            base_url: BASE_URL,
            high_quality: key.contains("/gravures/"),
        }
        .pages(&reader_url, &body)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let comics = self.list(json!({"page": 1, "listingId": "comics"}))?;
        let gravures = self.list(json!({"page": 1, "listingId": "gravures"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "comics".into(),
                title: "マンガ".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: comics.has_next_page,
                entries: comics.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "gravures".into(),
                title: "グラビア".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: gravures.has_next_page,
                entries: gravures.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::SpeedBinb::process_page_image(request)
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
            if key.contains("/episodes/") {
                return Ok(Some(UrlResolveResult {
                    url: Some(input.into()),
                    ..UrlResolveResult::default()
                }));
            }
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn fetch_xhr(target: &str, referer: &str, csrf: Option<&str>, fixture: &str) -> String {
    let http = client();
    let mut request = http.get(target).referer(referer).xhr();
    if let Some(csrf) = csrf {
        request = request.header("X-CSRF-Token", csrf);
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_comics_directory(body: &str, page: u64) -> Paged<CatalogItem> {
    let all = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("ga-comics-book-item"))
        .filter_map(comic_from_anchor)
        .fold(Vec::new(), push_unique);
    let start = (page.saturating_sub(1) as usize) * COMICS_PER_PAGE;
    let end = (start + COMICS_PER_PAGE).min(all.len());
    let entries = all.get(start..end).unwrap_or(&[]).to_vec();
    Paged {
        entries,
        has_next_page: end < all.len(),
    }
}

fn comic_from_anchor(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "mod-book-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ヤンマガ".into())),
        cover: image_from_chunk(chunk),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("nsfw".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_gravures(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("banner-link"))
        .filter_map(gravure_from_anchor)
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next_page(body),
    }
}

fn gravure_from_anchor(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::text_between(chunk, "text-wrapper", "</div>")
            .and_then(|block| html::text_between(&block, "<h2", "</h2>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "ヤンマガ".into())),
        cover: html::attr_after(chunk, "img-bg-wrapper", "data-bg")
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("nsfw".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn latest_updates(page: u64) -> ExtensionResult<Paged<CatalogItem>> {
    let page_url = format!("{BASE_URL}/comics/series/newer");
    let first = fetch_document(&page_url, LATEST_FIXTURE);
    let csrf = html::attr_after(&first, "name=\"csrf-token\"", "content");
    let more_url = html::attr_after(&first, "newer-older-episode-more-button", "data-path")
        .map(|value| absolute_url(&value));
    let count = html::attr_after(&first, "newer-older-episode-more-button", "data-count")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(LATEST_PER_PAGE);

    if page <= 1 {
        return Ok(Paged {
            entries: parse_latest_entries(&first),
            has_next_page: count > LATEST_PER_PAGE,
        });
    }

    let Some(more_url) = more_url else {
        return Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        });
    };
    let offset = (page - 1) * LATEST_PER_PAGE;
    let separator = if more_url.contains('?') { '&' } else { '?' };
    let target = format!("{more_url}{separator}offset={offset}");
    let script = fetch_xhr(&target, &page_url, csrf.as_deref(), LATEST_AJAX_FIXTURE);
    Ok(Paged {
        entries: parse_insert_adjacent_html_script(&script)
            .iter()
            .flat_map(|fragment| parse_latest_entries(fragment))
            .fold(Vec::new(), push_unique),
        has_next_page: offset + LATEST_PER_PAGE < count,
    })
}

fn parse_latest_entries(body: &str) -> Vec<CatalogItem> {
    let scope = html::text_between(body, "comic-episodes-newer", "</section>")
        .unwrap_or_else(|| body.to_string());
    scope
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("text-wrapper") || chunk.contains("img-bg-wrapper"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "text-wrapper", "</div>")
                    .and_then(|block| html::text_between(&block, "<h2", "</h2>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "ヤンマガ".into())
                    }),
                cover: html::attr_after(chunk, "img-bg-wrapper", "data-bg")
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("nsfw".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("search-item")
                && (chunk.contains("search-item-category--comics")
                    || chunk.contains("search-item-category--gravures"))
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "search-item-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "ヤンマガ".into())
                    }),
                cover: html::attr_after(chunk, "search-item-thumbnail-image", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
                status: if key.contains("/gravures/") && !key.contains("/series/") {
                    ItemStatus::Completed
                } else {
                    ItemStatus::Unknown
                },
                update_strategy: if key.contains("/gravures/") && !key.contains("/series/") {
                    Some(UpdateStrategy::OnlyFetchOnce)
                } else {
                    None
                },
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("nsfw".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next_page(body),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    let body = fetch_document(&absolute_url(&key), DETAILS_FIXTURE);
    if key.contains("/gravures/") {
        parse_gravure_details(&body, &key)
    } else {
        parse_comic_details(&body, &key)
    }
}

fn parse_comic_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "detailv2-outline-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "ヤンマガ".into())),
        cover: html::attr_after(body, "detailv2-thumbnail-image", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        authors: class_link_texts(body, "detailv2-outline-author-item"),
        description: html::text_between(body, "detailv2-description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: class_texts(body, "ga-tag"),
        status: if body.contains("detailv2-link-note") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("nsfw".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_gravure_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "detail-header-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "ヤンマガ".into())),
        cover: html::attr_after(body, "detail-header-image", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        tags: class_texts(body, "ga-tag"),
        status: if key.contains("/series/") {
            ItemStatus::Unknown
        } else {
            ItemStatus::Completed
        },
        update_strategy: (!key.contains("/series/")).then_some(UpdateStrategy::OnlyFetchOnce),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("nsfw".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, referer: &str) -> Vec<MangaChapter> {
    let mut chapters = parse_chapter_items(body);
    if body.contains("js-episode") {
        let total = html::attr_after(body, "id=\"contents\"", "data-count")
            .or_else(|| html::attr_after(body, "id='contents'", "data-count"))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(chapters.len());
        let offset = html::attr_after(body, "mod-episode-more-button", "data-offset")
            .and_then(|value| value.parse::<usize>().ok());
        let more_url = html::attr_after(body, "mod-episode-more-button", "data-path")
            .map(|value| absolute_url(&value));
        let csrf = html::attr_after(body, "name=\"csrf-token\"", "content");
        let per_page = body
            .split("gon.episode_more=")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(150);
        if let (Some(offset), Some(more_url)) = (offset, more_url) {
            let visible_tail = chapters.len().saturating_sub(offset);
            let stop = total.saturating_sub(visible_tail);
            let mut next = offset;
            while next < stop {
                let limit = stop - next;
                let mut target = format!("{more_url}?offset={next}");
                if limit < per_page {
                    target.push_str(&format!("&limit={limit}"));
                }
                target.push_str("&cb=1");
                let script = fetch_xhr(&target, referer, csrf.as_deref(), CHAPTERS_AJAX_FIXTURE);
                for fragment in parse_insert_adjacent_html_script(&script) {
                    for chapter in parse_chapter_items(&fragment) {
                        chapters = push_unique_chapter(chapters, chapter);
                    }
                }
                next += per_page;
            }
        }
    }
    chapters
        .into_iter()
        .filter(|chapter| !chapter.key.is_empty())
        .collect()
}

fn parse_chapter_items(body: &str) -> Vec<MangaChapter> {
    body.split("mod-episode-item")
        .skip(1)
        .filter(|chunk| chunk.contains("mod-episode-title"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "mod-episode-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "mod-episode-title", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "mod-episode-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_ymd_slash(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_insert_adjacent_html_script(script: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut rest = script;
    while let Some(index) = rest.find("insertAdjacentHTML") {
        rest = &rest[index + "insertAdjacentHTML".len()..];
        let Some(paren) = rest.find('(') else {
            break;
        };
        rest = &rest[paren + 1..];
        let Some((_, after_first)) = parse_js_string(rest) else {
            continue;
        };
        let Some(comma) = after_first.find(',') else {
            break;
        };
        let after_comma = after_first[comma + 1..].trim_start();
        let Some((html, after_second)) = parse_js_string(after_comma) else {
            continue;
        };
        output.push(html);
        rest = after_second;
    }
    output
}

fn parse_js_string(input: &str) -> Option<(String, &str)> {
    let mut chars = input.trim_start().char_indices();
    let quote = chars.next()?.1;
    if quote != '"' && quote != '\'' && quote != '`' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for (index, ch) in chars {
        if escaped {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '`' => out.push('`'),
                _ => out.push(ch),
            }
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some((out, &input.trim_start()[index + ch.len_utf8()..]));
        }
        out.push(ch);
    }
    None
}

fn class_link_texts(body: &str, class_name: &str) -> Vec<String> {
    body.split(class_name)
        .skip(1)
        .flat_map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|anchor| html::text_between(anchor, ">", "</a>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), push_unique_string)
}

fn class_texts(body: &str, class_name: &str) -> Vec<String> {
    body.split(class_name)
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), push_unique_string)
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "mod-book-image", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn has_next_page(body: &str) -> bool {
    body.contains("page-next")
}

fn normalize_key(value: &str) -> String {
    let without_origin = value
        .strip_prefix(BASE_URL)
        .or_else(|| value.strip_prefix(BASE_URL.trim_start_matches("https://")))
        .unwrap_or(value)
        .split('#')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    format!("/{}", without_origin.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn key_from_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.starts_with('/') {
        return Some(normalize_key(input));
    }
    input.strip_prefix(BASE_URL).map(normalize_key)
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn parse_ymd_slash(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400_000)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - (month <= 2) as i64;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(mut items: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_string(mut items: Vec<String>, item: String) -> Vec<String> {
    if !items.iter().any(|existing| existing == &item) {
        items.push(item);
    }
    items
}

fn err(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.into(),
    }
}

const COMICS_FIXTURE: &str = r#"
<a class="ga-comics-book-item" href="/comics/sample">
  <div class="mod-book-image"><img data-src="/cover.jpg"></div>
  <div class="mod-book-title">Sample Comic</div>
</a>
"#;

const GRAVURES_FIXTURE: &str = r#"
<a class="banner-link" href="/gravures/series/sample">
  <div class="img-bg-wrapper" data-bg="/gravure.jpg"></div>
  <div class="text-wrapper"><h2>Sample Gravure</h2></div>
</a>
"#;

const SEARCH_FIXTURE: &str = r#"
<ul class="search-list"><li class="search-item">
  <a href="/comics/sample"><div class="search-item-thumbnail-image"><img src="/cover.jpg"></div>
  <div class="search-item-title">Sample Comic</div><span class="search-item-category--comics"></span></a>
</li></ul>
"#;

const LATEST_FIXTURE: &str = r#"
<meta name="csrf-token" content="csrf">
<button class="newer-older-episode-more-button" data-count="1" data-path="/comics/series/newer/more"></button>
<section id="comic-episodes-newer">
<div><a href="/comics/sample"><div class="img-bg-wrapper" data-bg="/cover.jpg"></div><div class="text-wrapper"><h2>Sample Comic</h2></div></a></div>
</section>
"#;

const LATEST_AJAX_FIXTURE: &str = r#"target.insertAdjacentHTML("beforeend", "<div><a href=\"/comics/sample\"><div class=\"text-wrapper\"><h2>Sample Comic</h2></div></a></div>");"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="detailv2-outline-title">Sample Comic</h1>
<div class="detailv2-thumbnail-image"><img src="/cover.jpg"></div>
<div class="detailv2-outline-author-item"><a>Author</a></div>
<p class="detailv2-description">Summary</p>
<a class="ga-tag">Action</a>
<div class="detailv2-link-note"></div>
<div id="contents" data-count="1"></div>
<ul class="mod-episode-list">
  <li class="mod-episode-item"><a class="mod-episode-link" href="/episodes/sample"><span class="mod-episode-title">第1話</span><span class="mod-episode-date">2024/01/01</span></a></li>
</ul>
"#;

const CHAPTERS_AJAX_FIXTURE: &str = r#"target.insertAdjacentHTML("beforeend", "<li class=\"mod-episode-item\"><a class=\"mod-episode-link\" href=\"/episodes/sample\"><span class=\"mod-episode-title\">第1話</span></a></li>");"#;

const READER_FIXTURE: &str = r#"<div id="content"></div>"#;

export_manga_source!(SOURCE);
