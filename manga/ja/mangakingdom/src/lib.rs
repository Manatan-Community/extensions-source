use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: MangaKingdom = MangaKingdom;
const BASE_URL: &str = "https://comic.k-manga.jp";
const VIEWER_URL: &str = "https://bv.k-manga.jp/public/app/action/bd00.php";

struct MangaKingdom;

impl MangaSource for MangaKingdom {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_catalog(RANKING_FIXTURE, false));
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            let target = format!(
                "{BASE_URL}/search/new/?search_option[category]=0&search_option[new]=0&search_option[pvfv_flag]=0&search_option[finished_flag]=0&page={page}"
            );
            Ok(parse_catalog(
                &fetch_document(&target, LATEST_FIXTURE),
                true,
            ))
        } else {
            Ok(parse_catalog(
                &fetch_document(&format!("{BASE_URL}/rank/"), RANKING_FIXTURE),
                false,
            ))
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
        let page = page(&request);
        let target = search_url(query, page, &request);
        Ok(parse_catalog(
            &fetch_document(&target, SEARCH_FIXTURE),
            true,
        ))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        let mut chapters = Vec::new();
        let mut page = 1;
        loop {
            let body = fetch_document(
                &format!("{BASE_URL}/title/{}/pv/{page}", title_id(&key)),
                CHAPTERS_FIXTURE,
            );
            chapters.extend(parse_chapters_page(&body, title_id(&key), hide_locked));
            if !body.contains("paging--next") || page >= 20 {
                break;
            }
            page += 1;
        }
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "3/1/sample/pv/exid/fcipath/readtype/fcid/fcupdated".into());
        if key.starts_with("http") && key.ends_with("#1") {
            return Ok(Vec::new());
        }
        let response = client()
            .get(&format!("{BASE_URL}/viewer-launcher/{key}"))
            .browser_document()
            .send();
        let final_url = response
            .ok()
            .map(|response| response.final_url)
            .unwrap_or_else(|| format!("{BASE_URL}/viewer-launcher/{key}?p0=ticket&p1=obfuid"));
        let ticket = query_param(&final_url, "p0").unwrap_or_else(|| "ticket".into());
        let obfuid = query_param(&final_url, "p1").unwrap_or_else(|| "obfuid".into());
        let header = fetch_document(
            &viewer_url(&ticket, "64kb_QVGA_h", &obfuid, "header", None),
            HEADER_FIXTURE,
        );
        Ok(parse_pages_from_header(&header, &ticket, &obfuid))
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
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::MangaKingdomImage::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/title/{}/pv", title_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/viewer-launcher/{key}")))
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_header("Cookie", "is_verified_age_over_18=1")
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_url(query: &str, page: u64, request: &Value) -> String {
    let mut params = vec![
        ("search_option[search_word]", query.to_string()),
        (
            "search_option[sort]",
            filter_string(request, "sort").unwrap_or_else(|| "popular".into()),
        ),
        (
            "search_option[finished_flag]",
            filter_string(request, "finished_flag").unwrap_or_else(|| "0".into()),
        ),
        (
            "search_option[pvfv_flag]",
            filter_string(request, "pvfv_flag").unwrap_or_else(|| "0".into()),
        ),
    ];
    if let Some(category) = filter_string(request, "category").filter(|value| !value.is_empty()) {
        params.push(("search_option[categories][]", category));
    }
    if let Some(free) =
        filter_string(request, "free_campaign_type").filter(|value| !value.is_empty())
    {
        params.push(("search_option[free_campaign_type]", free));
    }
    if preference_bool(request, "without_sexy_title") {
        params.push(("search_option[without_sexy_title][]", "1".into()));
    }
    if page > 1 {
        params.push(("page", page.to_string()));
    }
    format!(
        "{BASE_URL}/search/detail?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{}={}", url::query_escape(key), url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_catalog(body: &str, paged: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .filter(|chunk| chunk.contains("book-list--item"))
        .filter_map(catalog_item)
        .collect();
    Paged {
        entries,
        has_next_page: paged && body.contains("paging--next"),
    }
}

fn catalog_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let id = href
        .split("/title/")
        .nth(1)
        .and_then(|value| value.split('/').next())
        .or_else(|| href.trim_matches('/').split('/').next_back())?;
    let title = html::text_between(chunk, "book-list--title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: id.into(),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|src| absolute_url(&src)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(format!("{BASE_URL}/title/{id}/pv")),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(
        &fetch_document(
            &format!("{BASE_URL}/title/{}/pv", title_id(key)),
            DETAILS_FIXTURE,
        ),
        title_id(key),
    )
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let info = html::text_between(body, "book-info", "</section>").unwrap_or_else(|| body.into());
    CatalogItem {
        key: key.into(),
        title: html::text_between(&info, "itemprop=name", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Manga Kingdom".into()),
        cover: html::attr_after(&info, "book-info--img", "src").map(|src| absolute_url(&src)),
        authors: detail_links(&info, "著者・作者"),
        tags: detail_links(&info, "ジャンル"),
        description: html::text_between(&info, "book-info--desc-text", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: if detail_text(&info, "配信").contains("完結") {
            ItemStatus::Completed
        } else {
            ItemStatus::Ongoing
        },
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(format!("{BASE_URL}/title/{key}/pv")),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_page(body: &str, manga_key: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let format_type = html::attr_after(body, "id=\"titlejs\"", "data-format-type")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "pv".into());
    let viewer_id = html::attr_after(body, "id=\"titlejs\"", "data-viewer-id-pc")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "3".into());
    let manga_title = html::text_between(body, "itemprop=name", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    body.split("<li")
        .filter(|chunk| chunk.contains("book-chapter--target"))
        .filter_map(|chunk| {
            let btn = chunk
                .split('<')
                .find(|part| part.contains("x-invoke-viewer--btn__selector"));
            let is_sample = btn
                .map(|part| part.contains("book-chapter--btn__sample"))
                .unwrap_or(false);
            let is_locked = btn.is_none() || is_sample;
            if hide_locked && is_locked {
                return None;
            }
            if let Some(btn) = btn {
                let exid = html::attr(btn, "data-chapter-exid")?;
                let fcipath = html::attr(btn, "data-chapter-fcipath")?;
                let readtype = html::attr(btn, "data-chapter-readtype")?;
                let fcid = html::attr(btn, "data-chapter-fcid")?;
                let fcupdated = html::attr(btn, "data-chapter-fcupdated")?;
                let title = html::text_between(chunk, "book-chapter--title", "</a>")
                    .map(|value| html::strip_tags(&value).replace(&manga_title, ""))
                    .unwrap_or_else(|| "Chapter".into());
                let prefix = if is_sample {
                    "Preview "
                } else if is_locked {
                    "Locked "
                } else {
                    ""
                };
                let key = format!(
                    "{viewer_id}/1/{manga_key}/{format_type}/{exid}/{fcipath}/{readtype}/{fcid}/{fcupdated}"
                );
                Some(MangaChapter {
                    key: key.clone(),
                    title: Some(format!("{prefix}{}", title.trim())),
                    url: Some(format!("{BASE_URL}/viewer-launcher/{key}")),
                    ..MangaChapter::default()
                })
            } else {
                None
            }
        })
        .collect()
}

fn parse_pages_from_header(body: &str, ticket: &str, obfuid: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(&strip_jsonp(body)).unwrap_or(Value::Null);
    let dk = root.get("dk").and_then(Value::as_str);
    root.get("contentInfos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|info| {
            let name = info
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("64kb_QVGA_h")
                .to_string();
            let start = info
                .get("startSceneNo")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let end = info
                .get("endSceneNo")
                .and_then(Value::as_u64)
                .unwrap_or(start);
            (start..=end).map(move |scene| (name.clone(), scene))
        })
        .enumerate()
        .map(|(index, (name, scene))| {
            let page_url = viewer_url(ticket, &name, obfuid, "content", dk);
            let mut extra = BTreeMap::new();
            extra.insert("mangaKingdomScene".into(), json!(scene));
            MangaPage {
                content: PageContent::Url {
                    url: page_url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect()
}

fn viewer_url(ticket: &str, file_name: &str, obfuid: &str, kind: &str, dk: Option<&str>) -> String {
    let mut params = vec![
        ("t", ticket.to_string()),
        ("fn", file_name.to_string()),
        ("o", obfuid.to_string()),
        ("type", kind.to_string()),
        ("callback", "cb".to_string()),
        ("u", "0".to_string()),
    ];
    if let Some(dk) = dk {
        params.push(("dk", dk.to_string()));
    }
    format!(
        "{VIEWER_URL}?{}",
        params
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn detail_links(body: &str, label: &str) -> Vec<String> {
    detail_text(body, label)
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn detail_text(body: &str, label: &str) -> String {
    let Some(after) = body.split(label).nth(1) else {
        return String::new();
    };
    html::text_between(after, "<dd", "</dd>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default()
}

fn query_param(input: &str, key: &str) -> Option<String> {
    input
        .split('?')
        .nth(1)?
        .split('#')
        .next()?
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find_map(|(name, value)| (name == key).then(|| value.to_string()))
}

fn strip_jsonp(input: &str) -> String {
    let trimmed = input.trim();
    match (trimmed.find('('), trimmed.rfind(')')) {
        (Some(open), Some(close)) if open < close => trimmed[open + 1..close].to_string(),
        _ => trimmed.to_string(),
    }
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(id))
        .or_else(|| request.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .split("/title/")
        .nth(1)
        .and_then(|value| value.split('/').next())
        .map(ToOwned::to_owned)
}

fn title_id(key: &str) -> &str {
    key.trim_matches('/')
        .strip_prefix("title/")
        .unwrap_or(key.trim_matches('/'))
        .split('/')
        .next()
        .unwrap_or(key)
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

const RANKING_FIXTURE: &str = r#"
<div class="book-list-ranking"><a class="book-list--item" href="/title/1/pv"><img src="/cover.jpg"><span class="book-list--title">Sample Ranking</span></a></div>
"#;

const LATEST_FIXTURE: &str = r#"
<div class="book-list__new"><a class="book-list--item" href="/title/2/pv"><img src="/cover.jpg"><span class="book-list--title">Sample Latest</span></a></div><a class="paging--next"></a>
"#;

const SEARCH_FIXTURE: &str = r#"
<div class="book-list-detail--box"><a class="book-list--item" href="/title/3/pv"><img src="/cover.jpg"><span class="book-list--title">Sample Search</span></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<section class="book-info"><h1 class="book-info--title"><span itemprop="name">Sample Kingdom</span></h1><img class="book-info--img" src="/cover.jpg"><dl class="book-info--detail"><dt>著者・作者</dt><dd><a>Author</a></dd><dt>ジャンル</dt><dd><a>Drama</a></dd><dt>配信</dt><dd>連載中</dd></dl><p class="book-info--desc-text">Description.</p></section>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<div id="titlejs" data-format-type="pv" data-viewer-id-pc="3"></div><h1 class="book-info--title"><span itemprop="name">Sample Kingdom</span></h1><ul class="book-chapter"><li class="book-chapter--target"><h2 class="book-chapter--title"><a>Sample Kingdom Chapter 1</a></h2><button class="x-invoke-viewer--btn__selector" data-chapter-exid="ex" data-chapter-fcipath="path" data-chapter-readtype="rt" data-chapter-fcid="fc" data-chapter-fcupdated="20240101"></button></li></ul>
"#;

const HEADER_FIXTURE: &str = r#"cb({"numOfScenes":1,"contentInfos":[{"name":"content","startSceneNo":1,"endSceneNo":1}],"dk":"dk"})"#;

export_manga_source!(SOURCE);
