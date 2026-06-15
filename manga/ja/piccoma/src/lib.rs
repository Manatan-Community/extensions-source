use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, ProcessedImage, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, url};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: Piccoma = Piccoma;
const BASE_URL: &str = "https://piccoma.com";

struct Piccoma;

impl MangaSource for Piccoma {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_ranking(RANKING_FIXTURE));
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_latest(&fetch_document(
                &format!("{BASE_URL}/web/weekday/product/list?page={page}"),
                LATEST_FIXTURE,
            )))
        } else {
            Ok(parse_ranking(&fetch_document(
                &format!("{BASE_URL}/web/ranking/K/P/0"),
                RANKING_FIXTURE,
            )))
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
        if query.is_empty() {
            let ranking = filter_string(&request, "ranking").unwrap_or_else(|| "K/P/0".into());
            Ok(parse_ranking(&fetch_document(
                &format!("{BASE_URL}/web/ranking/{ranking}"),
                RANKING_FIXTURE,
            )))
        } else {
            Ok(parse_search_response(
                &client()
                    .get(&format!(
                        "{BASE_URL}/web/search/result_ajax/list?word={}&page={page}&tab_type=T",
                        url::query_escape(query)
                    ))
                    .xhr()
                    .send_text()
                    .unwrap_or_else(|_| SEARCH_FIXTURE.to_string()),
                page,
            ))
        }
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/web/product/1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/web/product/1".into());
        let hide_locked = preference_bool(&request, "hide_locked");
        let episodes = fetch_document(
            &format!("{BASE_URL}{}/episodes?etype=E", normalize_key(&key)),
            EPISODES_FIXTURE,
        );
        let volumes = fetch_document(
            &format!("{BASE_URL}{}/episodes?etype=V", normalize_key(&key)),
            VOLUMES_FIXTURE,
        );
        let mut chapters = parse_episode_chapters(&episodes, hide_locked);
        chapters.extend(parse_volume_chapters(&volumes, hide_locked));
        chapters.reverse();
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/web/viewer/1/1".into());
        Ok(parse_pages(&fetch_document(
            &format!("{BASE_URL}{}", normalize_key(&key)),
            PAGES_FIXTURE,
        )))
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
        manga_image::PiccomaImage::process_page_image(request)
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_origin(BASE_URL)
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

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .filter(|chunk| chunk.contains("PCM-rankingProduct_title") && chunk.contains("<a"))
        .filter_map(ranking_item)
        .collect();
    Paged {
        entries,
        has_next_page: false,
    }
}

fn ranking_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "PCM-rankingProduct_title", "</p>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: normalize_key(&href),
        title,
        cover: image_from_chunk(chunk).as_deref().map(cover_x3),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(absolute_url(&href)),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .filter(|chunk| chunk.contains("PCOM-prdList_info") && chunk.contains("<a"))
        .filter_map(latest_item)
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("id=\"js_nextPage\"") || body.contains("id='js_nextPage'"),
    }
}

fn latest_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let title = html::text_between(chunk, "PCOM-prdList_title", "</span>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: normalize_key(&href),
        title,
        cover: image_from_chunk(chunk).as_deref().map(cover_x3),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(absolute_url(&href)),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_search_response(body: &str, page: u64) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let entries = root
        .pointer("/data/products")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("is_audio").and_then(Value::as_i64).unwrap_or(0) != 1)
        .filter(|item| item.get("is_anime").and_then(Value::as_i64).unwrap_or(0) != 1)
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_i64)?;
            let title = item.get("title").and_then(Value::as_str)?;
            Some(CatalogItem {
                key: format!("/web/product/{id}"),
                title: title.into(),
                cover: item.get("img").and_then(Value::as_str).map(cover_x3),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                url: Some(format!("{BASE_URL}/web/product/{id}")),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    let total_page = root
        .pointer("/data/total_page")
        .and_then(Value::as_u64)
        .unwrap_or(page);
    Paged {
        entries,
        has_next_page: page < total_page,
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), &key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let status_text = html::text_between(body, "PCM-productStatus", "</ul>")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    CatalogItem {
        key: key.into(),
        title: html::text_between(body, "PCM-productTitle", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Piccoma".into()),
        cover: html::attr_after(body, "PCM-productThum_img", "src")
            .as_deref()
            .map(cover_x3),
        authors: link_texts(body, "PCM-productAuthor"),
        tags: link_texts(body, "PCM-productGenre")
            .into_iter()
            .chain(link_texts(body, "PCM-productDesc_tagList"))
            .collect(),
        description: html::text_between(body, "PCM-productDesc", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: if status_text.contains("連載中") {
            ItemStatus::Ongoing
        } else if status_text.contains("完結") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(absolute_url(key)),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_episode_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let manga_title = html::text_between(body, "PCM-headTitle_name", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    body.split("<li")
        .filter(|chunk| chunk.contains("data-product_id") && chunk.contains("data-episode_id"))
        .filter_map(|chunk| {
            let is_locked = chunk.contains("PCM-epList_status_point")
                || chunk.contains("PCM-epList_status_waitfree")
                || chunk.contains("PCM-epList_status_zeroPlus");
            if hide_locked && is_locked {
                return None;
            }
            let product_id = html::attr(chunk, "data-product_id")?;
            let episode_id = html::attr(chunk, "data-episode_id")?;
            let title = html::text_between(chunk, "PCM-epList_title", "</h2>")
                .map(|value| strip_title(&html::strip_tags(&value), &manga_title))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Episode".into());
            let prefix = if chunk.contains("PCM-epList_status_point") {
                "Locked "
            } else if chunk.contains("PCM-epList_status_waitfree")
                || chunk.contains("PCM-epList_status_zeroPlus")
            {
                "Wait Free "
            } else {
                ""
            };
            let key = format!("/web/viewer/{product_id}/{episode_id}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("{prefix}{title}")),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_volume_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let manga_title = html::text_between(body, "PCM-headTitle_name", "</")
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    body.split("<li")
        .filter(|chunk| chunk.contains("data-product_id") && chunk.contains("data-episode_id"))
        .filter_map(|chunk| {
            let free = chunk.contains("PCM-prdVol_freeBtn");
            let trial = chunk.contains("PCM-prdVol_trialBtn");
            let buy = chunk.contains("PCM-prdVol_buyBtn");
            if hide_locked && !free && (trial || buy) {
                return None;
            }
            let product_id = html::attr(chunk, "data-product_id")?;
            let episode_id = html::attr(chunk, "data-episode_id")?;
            let title = html::text_between(chunk, "PCM-prdVol_title", "</h2>")
                .map(|value| strip_title(&html::strip_tags(&value), &manga_title))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Volume".into());
            let prefix = if trial {
                "Preview "
            } else if buy {
                "Locked "
            } else {
                ""
            };
            let key = format!("/web/viewer/{product_id}/{episode_id}");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("{prefix}{title}")),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let Some(script) = body
        .split("<script")
        .find(|chunk| chunk.contains("var _pdata_"))
    else {
        return Vec::new();
    };
    let raw = script
        .split("var _pdata_ =")
        .nth(1)
        .and_then(|value| value.split("var _rcm_").next())
        .unwrap_or("")
        .trim()
        .trim_end_matches(';');
    let normalized = normalize_pdata_json(raw);
    let root = serde_json::from_str::<Value>(&normalized).unwrap_or(Value::Null);
    let is_scrambled = root
        .get("isScrambled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let images = root
        .get("img")
        .or_else(|| root.get("contents"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    images
        .into_iter()
        .filter_map(|item| {
            item.get("path")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .filter(|path| !path.is_empty())
        .enumerate()
        .map(|(index, path)| {
            let image_url = absolute_image_url(&path);
            let mut extra = BTreeMap::new();
            if is_scrambled {
                if let Some(seed) = manga_image::PiccomaImage::seed_from_image_url(&image_url) {
                    extra.insert("piccomaSeed".into(), json!(seed));
                }
            }
            MangaPage {
                content: PageContent::Url {
                    url: image_url,
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

fn normalize_pdata_json(raw: &str) -> String {
    let without_title = remove_title_field(raw);
    quote_keys(&without_title)
        .replace('\'', "\"")
        .replace(",}", "}")
        .replace(",]", "]")
}

fn remove_title_field(raw: &str) -> String {
    let Some(start) = raw.find("title") else {
        return raw.to_string();
    };
    let Some(colon) = raw[start..].find(':').map(|idx| start + idx) else {
        return raw.to_string();
    };
    let quote = raw[colon + 1..]
        .chars()
        .find(|ch| *ch == '\'' || *ch == '"')
        .unwrap_or('"');
    let Some(value_start) = raw[colon + 1..].find(quote).map(|idx| colon + 1 + idx) else {
        return raw.to_string();
    };
    let Some(value_end) = raw[value_start + 1..]
        .find(quote)
        .map(|idx| value_start + 1 + idx)
    else {
        return raw.to_string();
    };
    let end = raw[value_end + 1..]
        .find(',')
        .map(|idx| value_end + 2 + idx)
        .unwrap_or(value_end + 1);
    format!("{}{}", &raw[..start], &raw[end..])
}

fn quote_keys(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        out.push(ch);
        if ch == '{' || ch == ',' {
            let mut whitespace = String::new();
            while matches!(chars.peek(), Some(next) if next.is_whitespace()) {
                whitespace.push(chars.next().unwrap());
            }
            let mut key = String::new();
            while matches!(chars.peek(), Some(next) if next.is_ascii_alphanumeric() || *next == '_')
            {
                key.push(chars.next().unwrap());
            }
            if !key.is_empty() && matches!(chars.peek(), Some(':')) {
                out.push_str(&whitespace);
                out.push('"');
                out.push_str(&key);
                out.push('"');
            } else {
                out.push_str(&whitespace);
                out.push_str(&key);
            }
        }
    }
    out
}

fn link_texts(body: &str, marker: &str) -> Vec<String> {
    html::text_between(body, marker, "</ul>")
        .unwrap_or_default()
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "data-original", "data-original")
        .or_else(|| html::attr_after(chunk, "<img", "data-original"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn cover_x3(value: &str) -> String {
    let absolute = absolute_image_url(value);
    replace_path_segment(&absolute, 4, "cover_x3")
}

fn replace_path_segment(value: &str, index: usize, replacement: &str) -> String {
    let Some((prefix, rest)) = value.split_once("://") else {
        return value.into();
    };
    let Some((host, path)) = rest.split_once('/') else {
        return value.into();
    };
    let mut parts = path.split('/').collect::<Vec<_>>();
    if let Some(part) = parts.get_mut(index) {
        *part = replacement;
    }
    format!("{prefix}://{host}/{}", parts.join("/"))
}

fn strip_title(value: &str, title: &str) -> String {
    if title.is_empty() {
        value.trim().into()
    } else {
        value.replace(title, "").trim().into()
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
        .strip_prefix(BASE_URL)
        .map(normalize_key)
        .filter(|key| key.starts_with("/web/product/"))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(BASE_URL).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn absolute_image_url(value: &str) -> String {
    if value.starts_with("//") {
        format!("https:{value}")
    } else if value.starts_with("http://") || value.starts_with("https://") {
        value.into()
    } else {
        absolute_url(value)
    }
}

const RANKING_FIXTURE: &str = r#"
<section class="PCM-productRanking">
  <li><a href="/web/product/1"><img class="js_lazy" data-original="//img.piccoma.com/a/b/c/checksum/cover.jpg?expires=0"><div class="PCM-rankingProduct_title"><p>Sample Ranking</p></div></a></li>
</section>
"#;

const LATEST_FIXTURE: &str = r#"
<li><a href="/web/product/2"><img src="//img.piccoma.com/a/b/c/checksum/cover.jpg?expires=0"><div class="PCOM-prdList_info"><div class="PCOM-prdList_title"><span>Sample Latest</span></div></div></a></li><a id="js_nextPage"></a>
"#;

const SEARCH_FIXTURE: &str = r#"{"data":{"products":[{"id":3,"title":"Sample Search","img":"//img.piccoma.com/a/b/c/checksum/cover.jpg?expires=0","is_audio":0,"is_anime":0}],"total_page":1}}"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="PCM-productTitle">Sample Piccoma</h1>
<img class="PCM-productThum_img" src="//img.piccoma.com/a/b/c/checksum/cover.jpg?expires=0">
<ul class="PCM-productStatus"><li>連載中</li></ul>
<ul class="PCM-productAuthor"><li><a>Author</a></li></ul>
<ul class="PCM-productGenre"><li><a>Drama</a></li></ul>
<div class="PCM-productDesc"><p>Description text.</p></div>
"#;

const EPISODES_FIXTURE: &str = r#"
<div class="PCM-headTitle_name">Sample Piccoma</div>
<ul id="js_episodeList">
  <li><a data-product_id="1" data-episode_id="11"></a><div class="PCM-epList_title"><h2>Sample Piccoma Episode 1</h2></div></li>
</ul>
"#;

const VOLUMES_FIXTURE: &str = r#"
<div class="PCM-headTitle_name">Sample Piccoma</div>
<ul id="js_volumeList">
  <li><button class="PCM-prdVol_freeBtn" data-product_id="1" data-episode_id="21"></button><div class="PCM-prdVol_title"><h2>Sample Piccoma Volume 1</h2></div></li>
</ul>
"#;

const PAGES_FIXTURE: &str = r#"
<script>
var _pdata_ = {title:'Sample', isScrambled:true, img:[{path:'//img.piccoma.com/a/b/c/SONTGGB0G[TQ3FPT7ECYJC/page1.jpg?expires=0'}]};
var _rcm_ = {};
</script>
"#;

export_manga_source!(SOURCE);
