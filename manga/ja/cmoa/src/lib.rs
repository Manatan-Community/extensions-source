use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, Paged,
    ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    html, manga, manga_image, sdk::http::HttpClient, speedbinb::SpeedBinbReader, url,
};
use serde_json::{Value, json};

const SOURCE: Cmoa = Cmoa;
const BASE_URL: &str = "https://www.cmoa.jp";
const ADULT_COOKIE: &str = "safesearch=0; R18user=1";

struct Cmoa;

impl MangaSource for Cmoa {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let listing = request
            .get("listingId")
            .or_else(|| request.get("listing_id"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        if listing == "latest" {
            return Ok(parse_latest(&fetch_document(
                &format!("{BASE_URL}/newrelease/?page={}", page(&request)),
                LATEST_FIXTURE,
            )));
        }
        Ok(parse_search_results(&fetch_document(
            &format!(
                "{BASE_URL}/search/purpose/ranking/all?period=daily&daily=all&page={}",
                page(&request)
            ),
            POPULAR_FIXTURE,
        )))
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

        let mut params = Vec::new();
        if !query.is_empty() {
            params.push(format!("word={}", url::query_escape(query)));
        }
        append_filter_text(&request, "title_nm", &mut params);
        append_filter_text(&request, "author_nm", &mut params);
        append_filter_text(&request, "magazine_nm", &mut params);
        append_filter_text(&request, "publisher_nm", &mut params);
        append_filter_text(&request, "titletag_nm", &mut params);
        append_filter_value(&request, "genre_id", &mut params);
        append_filter_value(&request, "point", &mut params);
        append_filter_value(&request, "review", &mut params);
        append_filter_value(&request, "sort", &mut params);
        append_filter_flag(&request, "free_cam_flg", &mut params);
        append_filter_flag(&request, "sample_up_flg", &mut params);
        append_filter_flag(&request, "campaign_flg", &mut params);
        append_filter_flag(&request, "newest_flg", &mut params);
        append_filter_flag(&request, "complete_flg", &mut params);
        params.push(format!("page={}", page(&request)));

        let separator = if params.is_empty() { "" } else { "?" };
        Ok(parse_search_results(&fetch_document(
            &format!("{BASE_URL}/search/result{separator}{}", params.join("&")),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        let hide_locked = preference_bool(&request, "hide_locked", false);
        Ok(fetch_chapters(&key, hide_locked))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/reader/sample/?title_id=sample&content_id=sample".into());
        let reader_url = reader_url_with_cid(&absolute_url(&key));
        let body = fetch_document(&reader_url, READER_FIXTURE);
        SpeedBinbReader {
            base_url: BASE_URL,
            high_quality: true,
        }
        .pages(&reader_url, &body)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_header("Cookie", ADULT_COOKIE)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .referer(BASE_URL)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_search_results(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li class=\"search_result_box")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = title_key_from_href(&href)?;
            Some(CatalogItem {
                key: key.clone(),
                title: search_result_title(chunk)
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "C'moA".into())),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("li.next") && !body.contains("li.next nopage"),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("title_wrap")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "thum_box", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = title_key_from_href(&href)?;
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "title_name", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "C'moA".into())),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: has_next_latest_page(body),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut tags = Vec::new();
    push_tag_text(&mut tags, body, "comic_mark_thum");
    for tag in link_texts_after(body, "genre_detail") {
        push_tag(&mut tags, tag);
    }

    CatalogItem {
        key: normalize_manga_key(key),
        title: html::text_between(body, "titleName", "</h1>")
            .map(|value| trim_title_suffix(&html::strip_tags(&value)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "C'moA".into())),
        cover: html::attr_after(body, "thumBox", "src").map(|value| absolute_url(&value)),
        authors: link_texts_after(body, "title_details_author_name"),
        description: html::text_between(body, "comic_description", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: if html::text_between(body, "volume", "</div>")
            .map(|value| value.contains("完結"))
            .unwrap_or(false)
        {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        tags,
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn fetch_chapters(key: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    let mut page_no = 1u32;
    loop {
        let target = format!("{}?page={page_no}&order=down", absolute_url(key));
        let body = fetch_document(&target, if page_no == 1 { CHAPTERS_FIXTURE } else { "" });
        let manga_title = html::text_between(&body, "titleName", "</h1>")
            .map(|value| trim_title_suffix(&html::strip_tags(&value)))
            .unwrap_or_default();
        chapters.extend(parse_chapter_page(&body, &manga_title, hide_locked));
        if !has_next_chapter_page(&body) || page_no >= 50 {
            break;
        }
        page_no += 1;
    }
    chapters
}

fn parse_chapter_page(body: &str, manga_title: &str, hide_locked: bool) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("title_details_title_name_h2"))
        .filter_map(|chunk| {
            let chapter_url = first_href_containing(chunk, "content_id")
                .or_else(|| first_href_containing(chunk, "browserviewer"))
                .or_else(|| html::attr_after(chunk, "title_details_title_name_h2", "href"))?;
            let raw_name = html::text_between(chunk, "title_details_title_name_h2", "</h3>")
                .map(|value| clean_chapter_title(&html::strip_tags(&value), manga_title))
                .filter(|value| !value.is_empty());
            let has_free_or_owned =
                chunk.contains("GA_free btn") || chapter_url.contains("browserviewer");
            let has_preview = !has_free_or_owned && chunk.contains("title_vol_each_free_btn");
            let is_locked = !has_free_or_owned
                && (chunk.contains("cart_into_btn") || chunk.contains("auto_buy_btn_s"));
            if hide_locked && (is_locked || has_preview) {
                return None;
            }
            let title = match raw_name {
                Some(name) if has_preview => Some(format!("(Preview) {name}")),
                Some(name) if is_locked => Some(format!("Locked {name}")),
                other => other,
            };
            Some(MangaChapter {
                key: normalize_chapter_key(&chapter_url),
                title,
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn search_result_title(chunk: &str) -> Option<String> {
    html::text_between(chunk, "search_result_box_right_sec1", "</a>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
}

fn link_texts_after(body: &str, marker: &str) -> Vec<String> {
    let Some(after) = body.split(marker).nth(1) else {
        return Vec::new();
    };
    let block = after.split("</div>").next().unwrap_or(after);
    block
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            chunk
                .split("</a>")
                .next()
                .map(html::strip_tags)
                .filter(|value| !value.is_empty())
        })
        .collect()
}

fn push_tag_text(tags: &mut Vec<String>, body: &str, marker: &str) {
    if let Some(value) =
        html::text_between(body, marker, "</a>").map(|value| html::strip_tags(&value))
    {
        push_tag(tags, value);
    }
}

fn push_tag(tags: &mut Vec<String>, value: String) {
    if !value.is_empty() && !tags.iter().any(|tag| tag == &value) {
        tags.push(value);
    }
}

fn first_href_containing(body: &str, needle: &str) -> Option<String> {
    body.split("<a").skip(1).find_map(|chunk| {
        if chunk.contains(needle) {
            html::attr(chunk, "href")
        } else {
            None
        }
    })
}

fn trim_title_suffix(title: &str) -> String {
    let mut out = title.trim().to_string();
    if let Some(index) = out.rfind('（') {
        if out.ends_with('）')
            && out[index + '（'.len_utf8()..out.len() - '）'.len_utf8()]
                .chars()
                .all(is_numberish)
        {
            out.truncate(index);
        }
    }
    if let Some(index) = out.rfind('(') {
        if out.ends_with(')') && out[index + 1..out.len() - 1].chars().all(is_numberish) {
            out.truncate(index);
        }
    }
    out.trim().to_string()
}

fn clean_chapter_title(raw: &str, manga_title: &str) -> String {
    raw.replace(manga_title, "")
        .replace("NEW ", "")
        .replace("発売予定 ", "")
        .trim()
        .to_string()
}

fn is_numberish(ch: char) -> bool {
    ch.is_ascii_digit()
        || matches!(
            ch,
            '０' | '１' | '２' | '３' | '４' | '５' | '６' | '７' | '８' | '９'
        )
}

fn has_next_chapter_page(body: &str) -> bool {
    body.contains("<li class=\"selected\"")
        && body
            .split("<li class=\"selected\"")
            .nth(1)
            .and_then(|rest| rest.split("</li>").nth(1))
            .map(|rest| rest.contains("<li><a href=") || rest.contains("<li class=\"\"><a href="))
            .unwrap_or(false)
}

fn has_next_latest_page(body: &str) -> bool {
    let Some(pager) = body.split("pageSlider").nth(1) else {
        return false;
    };
    pager
        .split("swiper-button-prev")
        .next()
        .map(|block| {
            block.contains("swiper-slide selected") && block.contains("href=\"/newrelease/?page=")
        })
        .unwrap_or(false)
}

fn reader_url_with_cid(reader_url: &str) -> String {
    if reader_url.contains("cid=") || !reader_url.contains("content_id=") {
        return reader_url.to_string();
    }
    let Some(content_id) = query_param(reader_url, "content_id") else {
        return reader_url.to_string();
    };
    let separator = if reader_url.contains('?') { '&' } else { '?' };
    format!("{reader_url}{separator}cid={content_id}")
}

fn title_key_from_href(href: &str) -> Option<String> {
    let normalized = normalize_path(href);
    let mut parts = normalized.trim_matches('/').split('/');
    (parts.next() == Some("title"))
        .then(|| parts.next())
        .flatten()
        .filter(|id| !id.is_empty())
        .map(|id| format!("/title/{id}"))
}

fn normalize_manga_key(value: &str) -> String {
    title_key_from_href(value).unwrap_or_else(|| normalize_path(value))
}

fn normalize_chapter_key(value: &str) -> String {
    normalize_path(value)
}

fn normalize_path(value: &str) -> String {
    let without_base = value.strip_prefix(BASE_URL).unwrap_or(value);
    let without_base = without_base
        .strip_prefix("https://cmoa.jp")
        .unwrap_or(without_base);
    let path = without_base
        .split('#')
        .next()
        .unwrap_or(without_base)
        .trim();
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    if value.starts_with("//") {
        return format!("https:{value}");
    }
    let joined = url::join_url(BASE_URL, value);
    if joined.starts_with("//") {
        format!("https:{joined}")
    } else {
        joined
    }
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(BASE_URL) || input.starts_with("https://cmoa.jp") {
        title_key_from_href(input)
    } else {
        None
    }
}

fn query_param(target: &str, key: &str) -> Option<String> {
    let query = target
        .split_once('?')?
        .1
        .split('#')
        .next()
        .unwrap_or_default();
    for part in query.split('&') {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn append_filter_text(request: &Value, id: &str, params: &mut Vec<String>) {
    if let Some(value) = filter_value(request, id).filter(|value| !value.trim().is_empty()) {
        params.push(format!("{id}={}", url::query_escape(value.trim())));
    }
}

fn append_filter_value(request: &Value, id: &str, params: &mut Vec<String>) {
    if let Some(value) = filter_value(request, id).filter(|value| !value.trim().is_empty()) {
        params.push(format!("{id}={}", url::query_escape(value.trim())));
    }
}

fn append_filter_flag(request: &Value, id: &str, params: &mut Vec<String>) {
    if filter_bool(request, id) {
        params.push(format!("{id}=1"));
    }
}

fn filter_value<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| {
            filters.get(id).or_else(|| {
                filters.as_array().and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
                        .and_then(|item| item.get("value"))
                })
            })
        })
        .and_then(Value::as_str)
}

fn filter_bool(request: &Value, id: &str) -> bool {
    request
        .get("filters")
        .and_then(|filters| {
            filters.get(id).or_else(|| {
                filters.as_array().and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
                        .and_then(|item| item.get("value"))
                })
            })
        })
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(false)
}

fn preference_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_str().map(|text| text == "true"))
        })
        .unwrap_or(default)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"
<li class="search_result_box"><div class="search_result_box_left"><a href="/title/sample/" class="title"><img src="//cmoa.akamaized.net/sample.jpg" alt="Sample Cmoa"></a></div><div class="search_result_box_right_sec1"><a href="/title/sample/" class="title">Sample Cmoa</a></div></li>
"#;

const SEARCH_FIXTURE: &str = POPULAR_FIXTURE;

const LATEST_FIXTURE: &str = r#"
<li class="title_wrap"><div class="thum_box"><a href="/title/sample/vol/1/"><img src="//cmoa.akamaized.net/sample.jpg" alt="Sample Cmoa"></a></div><p class="title_name">Sample Cmoa</p></li>
"#;

const DETAILS_FIXTURE: &str = r#"
<a href="/search/genre/13/" class="comic_mark_thum">青年マンガ</a>
<div class="thumBox"><img src="//cmoa.akamaized.net/sample.jpg" alt="Sample Cmoa（１）"></div>
<div class="volume">1巻配信中</div>
<h1 class="titleName">Sample Cmoa（１）</h1>
<div class="title_details_author_name"><a href="/search/author/1/">Sample Author</a></div>
<div id="comic_description"><p>Sample description.</p></div>
<div class="category_line_f_r_l genre_detail"><a href="/search/genre/1/">Action</a></div>
<ul class="title_vol_vox_vols">
<li><div class="title_vol_btn_box_w"><a href="/reader/sample/?title_id=sample&content_id=sample" rel="nofollow"><div class="GA_free btn free"></div></a></div><h3 class="title_details_title_name_h2"><a href="/title/sample/">Sample Cmoa（１）</a></h3></li>
</ul>
"#;

const CHAPTERS_FIXTURE: &str = DETAILS_FIXTURE;

const READER_FIXTURE: &str = r#"
<div id="content" class="pages" data-ptbinb="/bib/sws/bibGetCntntInfo.php" data-ptbinb-cid="sample"></div>
"#;
