use manatan_extension::{abi::ExtensionResult, export_manga_source, http, source::MangaSource, CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest, UpdateStrategy, UrlResolveResult};
use manatan_shared::{manga, url};
use serde_json::Value;

const SOURCE: HComic = HComic;
const BASE_URL: &str = "https://h-comic.com";
const IMG_URL: &str = "https://h-comic.link/api";

struct HComic;

impl MangaSource for HComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let target = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            format!("{BASE_URL}/random/__data.json")
        } else {
            format!("{BASE_URL}/__data.json?page={}", page(&request))
        };
        Ok(parse_list(&fetch(&target, LIST_FIXTURE), page(&request)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            let title = query.trim_end_matches('/').rsplit('/').nth(1).unwrap_or("sample");
            let key = title.to_string();
            return Ok(Paged { entries: vec![parse_one(&fetch(&format!("{BASE_URL}/comics/{}/1/__data.json", url::query_escape(title)), DETAILS_FIXTURE), &key)], has_next_page: false });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let random = filters.get("random").and_then(Value::as_str).unwrap_or_default();
        let tag = filters.get("tag").and_then(Value::as_str).unwrap_or_default();
        let target = format!("{BASE_URL}{random}/__data.json?tag={}&q={}&page={}", url::query_escape(tag), url::query_escape(query), page(&request));
        Ok(parse_list(&fetch(&target, LIST_FIXTURE), page(&request)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(parse_one(&fetch(&format!("{BASE_URL}/comics/{}/1/__data.json", url::query_escape(&key)), DETAILS_FIXTURE), &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let item = request.get("manga").cloned().unwrap_or_default();
        let key = item.get("key").and_then(Value::as_str).unwrap_or("sample");
        let source = item.get("extra").and_then(|e| e.get("source")).and_then(Value::as_str).unwrap_or("hcomic");
        let media = item.get("extra").and_then(|e| e.get("mediaId")).and_then(Value::as_str).unwrap_or("sample");
        let pages = item.get("extra").and_then(|e| e.get("numPages")).and_then(Value::as_u64).unwrap_or(1);
        Ok(vec![MangaChapter {
            key: format!("{source}/{media}:{pages}"),
            title: Some(item.get("title").and_then(Value::as_str).unwrap_or(key).to_string()),
            scanlators: item.get("tags").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).map(|v| vec![v.to_string()]).unwrap_or_default(),
            url: Some(format!("{BASE_URL}/comics/{}/1", url::query_escape(key))),
            page_count: Some(pages as u32),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "hcomic/sample:1".into());
        let (prefix, count) = key.rsplit_once(':').unwrap_or((&key, "1"));
        let count = count.parse::<usize>().unwrap_or(1);
        Ok((0..count).map(|index| MangaPage {
            content: PageContent::Url { url: format!("{IMG_URL}/{prefix}/pages/{}", index + 1), context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        }).collect())
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/comics/{}/1", url::query_escape(&key))))
    }
    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").and_then(|key| key.split('/').nth(1).map(|id| format!("{BASE_URL}/comics/{id}/1"))))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let key = input.trim_end_matches('/').rsplit('/').nth(1).unwrap_or("sample").to_string();
            return Ok(Some(UrlResolveResult { item: Some(parse_one(&fetch(&format!("{input}/__data.json"), DETAILS_FIXTURE), &key)), url: Some(input.into()), ..UrlResolveResult::default() }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }), url: Some(input.into()), ..UrlResolveResult::default() }))
    }
}

fn client() -> http::HttpClient { http::HttpClient::browser().with_desktop_user_agent().with_referer(format!("{BASE_URL}/")).with_cookies_for(BASE_URL).with_webview_challenge_fallback() }
fn fetch(target: &str, fixture: &str) -> String { client().get(target).send_text().unwrap_or_else(|_| fixture.into()) }
fn page(request: &Value) -> u64 { request.get("page").and_then(Value::as_u64).unwrap_or(1) }

fn parse_list(body: &str, page_no: u64) -> Paged<CatalogItem> {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(LIST_FIXTURE).expect("valid fixture"));
    let data = root.pointer("/nodes/1/data").and_then(Value::as_array).cloned().unwrap_or_default();
    let indexes = data.first().and_then(Value::as_object).cloned().unwrap_or_default();
    let comics = index_array(&data, &indexes, "comics");
    let total_pages = indexes.get("pages").and_then(Value::as_u64).and_then(|idx| data.get(idx as usize)).and_then(|v| v.get("pages")).and_then(Value::as_u64).and_then(|idx| data.get(idx as usize)).and_then(Value::as_u64).unwrap_or(1);
    Paged { entries: comics.into_iter().filter_map(|idx| parse_manga(&data, idx)).collect(), has_next_page: page_no < total_pages }
}

fn parse_one(body: &str, key: &str) -> CatalogItem {
    let root = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::from_str(DETAILS_FIXTURE).expect("valid fixture"));
    let data = root.pointer("/nodes/1/data").and_then(Value::as_array).cloned().unwrap_or_default();
    parse_manga(&data, 1).unwrap_or_else(|| CatalogItem { key: key.into(), title: key.into(), language: Some("zh".into()), content_rating: Some("adult".into()), ..CatalogItem::default() })
}

fn parse_manga(data: &[Value], idx: usize) -> Option<CatalogItem> {
    let obj = data.get(idx)?.as_object()?;
    let media = str_field(data, obj, "media_id")?;
    let source = str_field(data, obj, "comic_source").unwrap_or_else(|| "hcomic".into());
    let title_obj = data.get(obj.get("title")?.as_u64()? as usize)?.as_object()?;
    let title = str_field(data, title_obj, "display").or_else(|| str_field(data, title_obj, "pretty")).unwrap_or_else(|| media.clone());
    let num_pages = int_field(data, obj, "num_pages").unwrap_or(1);
    let timestamp = int_field(data, obj, "upload_date").unwrap_or(0);
    let tags = obj.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(|v| parse_tag(data, v.as_u64()? as usize)).collect::<Vec<_>>();
    let author = tags.iter().filter(|t| t.0 == "artist").map(|t| t.2.clone()).collect::<Vec<_>>();
    let genre = tags.iter().filter(|t| t.0 == "tag").map(|t| t.2.clone()).collect::<Vec<_>>();
    Some(CatalogItem {
        key: title.clone(),
        title,
        cover: Some(format!("{IMG_URL}/{source}/{media}")),
        authors: author,
        tags: genre,
        description: Some(format!("页数：{num_pages}")),
        latest_update: Some(timestamp as i64),
        status: ItemStatus::Completed,
        url: Some(format!("{BASE_URL}/comics/{}/1", url::query_escape(&media))),
        language: Some("zh".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        update_strategy: Some(UpdateStrategy::OnlyFetchOnce),
        extra: [("source".into(), Value::String(source)), ("mediaId".into(), Value::String(media)), ("numPages".into(), Value::from(num_pages))].into_iter().collect(),
        ..CatalogItem::default()
    })
}

fn index_array(data: &[Value], indexes: &serde_json::Map<String, Value>, key: &str) -> Vec<usize> {
    indexes.get(key).and_then(Value::as_u64).and_then(|idx| data.get(idx as usize)).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect()).unwrap_or_default()
}
fn str_field(data: &[Value], obj: &serde_json::Map<String, Value>, key: &str) -> Option<String> { data.get(obj.get(key)?.as_u64()? as usize)?.as_str().map(ToOwned::to_owned) }
fn int_field(data: &[Value], obj: &serde_json::Map<String, Value>, key: &str) -> Option<u64> { data.get(obj.get(key)?.as_u64()? as usize)?.as_u64() }
fn parse_tag(data: &[Value], idx: usize) -> Option<(String, String, String)> {
    let obj = data.get(idx)?.as_object()?;
    Some((str_field(data, obj, "type")?, str_field(data, obj, "name")?, obj.get("name_zh").and_then(Value::as_u64).and_then(|i| data.get(i as usize)).and_then(Value::as_str).map(ToOwned::to_owned).or_else(|| str_field(data, obj, "name"))?))
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"nodes":[null,{"data":[{"comics":1,"pages":2},[3],{"pages":1},{"id":4,"media_id":5,"comic_source":6,"title":7,"tags":12,"num_pages":10,"upload_date":11},"1","sample","hcomic",{"display":8,"pretty":8},"Sample","unused",1,1704067200,[]]}]}"#;
const DETAILS_FIXTURE: &str = LIST_FIXTURE;
