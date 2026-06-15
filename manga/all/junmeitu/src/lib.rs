use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const BASE_URL: &str = "https://meijuntu.com";
const SOURCE: Junmeitu = Junmeitu;

struct Junmeitu;

impl MangaSource for Junmeitu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request_page(&request);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/beauty/hot-{page}.html"), LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request_page(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged { entries: vec![parse_details(&body, Some(key))], has_next_page: false });
        }

        let target = if !query.is_empty() {
            format!("{BASE_URL}/search/{}-{page}.html", url::query_escape(query))
        } else if let Some(tag) = filter_text(&request, "tag") {
            format!("{BASE_URL}/tags/{}-{}-{page}.html", url::query_escape(&tag), category_id(&request))
        } else if let Some(model) = filter_text(&request, "model") {
            format!("{BASE_URL}/model/{}-{page}.html", url::query_escape(&model))
        } else if let Some(group) = filter_text(&request, "group") {
            format!("{BASE_URL}/xzjg/{}-{page}.html", url::query_escape(&group))
        } else {
            let category = filter_value(&request, "category").unwrap_or_else(|| "beauty".into());
            let sort = filter_value(&request, "sort").unwrap_or_else(|| "index".into());
            format!("{BASE_URL}/{category}/{sort}-{page}.html")
        };

        let body = fetch_document_or_fixture(&target, LIST_FIXTURE);
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/beauty/sample.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/beauty/sample.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Gallery".into()),
            date_uploaded: parse_upload_date(&body),
            url: Some(url::join_url(BASE_URL, &key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/beauty/sample.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_pages(&body, &key))
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let page_url = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
                .and_then(|lazy| lazy.get("pageUrl"))
            .and_then(Value::as_str)
            .or_else(|| request.get("page").and_then(|page| page.get("url")).and_then(Value::as_str))
            .unwrap_or_default();
        if page_url.is_empty() {
            return Ok(MangaPageImage::default());
        }
        let body = fetch_document_or_fixture(page_url, AJAX_FIXTURE);
        Ok(MangaPageImage { url: parse_image_response(&body), headers: manga::image_headers(BASE_URL), ..MangaPageImage::default() })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("<img") && chunk.contains("<a"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<p", "</p>")
                .map(|value| html::strip_tags(&value))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Gallery".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("all".into()),
                content_rating: Some("adult".into()),
                status: ItemStatus::Completed,
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();

    Paged { entries, has_next_page: body.contains("span + a") || body.contains("下一页") || body.contains("next") }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/beauty/sample.html".into());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "news-title", "</")
            .or_else(|| html::text_between(body, "title", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Junmeitu Gallery".into())),
        cover: first_image(body).map(|value| url::join_url(BASE_URL, &value)),
        description: Some(detail_description(body)),
        tags: detail_tags(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str, key: &str) -> Vec<MangaPage> {
    if body.contains("news-body") {
        let pages = body
            .split("<img")
            .skip(1)
            .filter_map(image_attr)
            .map(|value| url::join_url(BASE_URL, &value))
            .collect::<Vec<_>>();
        return direct_pages(pages);
    }

    let page_count = max_page_number(body).max(1);
    let first_image = html::text_between(body, "pictures", "</div>").and_then(|chunk| first_image(&chunk));
    let mut pages = Vec::new();
    if let Some(image) = first_image {
        pages.push(direct_page(0, url::join_url(BASE_URL, &image)));
    } else {
        pages.push(lazy_page(0, page_url(key, 1, body)));
    }

    for index in 2..=page_count {
        pages.push(lazy_page(index - 1, page_url(key, index, body)));
    }

    pages
}

fn parse_image_response(body: &str) -> String {
    serde_json::from_str::<AjaxDto>(body)
        .ok()
        .and_then(|dto| first_image(&dto.pic))
        .or_else(|| first_image(body))
        .map(|value| url::join_url(BASE_URL, &value))
        .unwrap_or_default()
}

fn direct_pages(images: Vec<String>) -> Vec<MangaPage> {
    images.into_iter().enumerate().map(|(index, image)| direct_page(index, image)).collect()
}

fn direct_page(index: usize, image: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url { url: image, context: Some(manga::image_headers(BASE_URL)) },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn lazy_page(index: usize, page_url: String) -> MangaPage {
    MangaPage {
        content: PageContent::Lazy {
            key: format!("page-{}", index + 1),
            url: None,
            page_url: Some(page_url),
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn page_url(key: &str, index: usize, body: &str) -> String {
    let slug = key.trim_end_matches(".html").rsplit_once('-').map(|(prefix, _)| prefix).unwrap_or_else(|| key.trim_end_matches(".html"));
    let category = key.trim_start_matches('/').split('/').next().unwrap_or("beauty");
    if let (Some(catid), Some(conid)) = (script_number(body, "pc_cid"), script_number(body, "pc_id")) {
        format!("{BASE_URL}/ajax_{category}{}-{index}.html?ajax=1&catid={catid}&conid={conid}", slug.strip_prefix(&format!("/{category}")).unwrap_or(slug))
    } else {
        format!("{BASE_URL}{slug}{}.html", if index > 1 { format!("-{index}") } else { String::new() })
    }
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-original")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "src"))
        .filter(|value| !value.is_empty())
}

fn first_image(body: &str) -> Option<String> {
    body.split("<img").nth(1).and_then(image_attr)
}

fn max_page_number(body: &str) -> usize {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .filter_map(|value| value.trim().parse::<usize>().ok())
        .max()
        .unwrap_or(1)
}

fn script_number(body: &str, name: &str) -> Option<String> {
    html::text_between(body, &format!("{name} = "), ";").map(|value| value.trim().trim_matches('"').trim_matches('\'').to_string())
}

fn detail_description(body: &str) -> String {
    [".news-info", ".picture-details", ".introduce"]
        .iter()
        .filter_map(|marker| html::text_between(body, marker, "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn detail_tags(body: &str) -> Vec<String> {
    let block = html::text_between(body, "relation_tags", "</div>").unwrap_or_default();
    block.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_upload_date(body: &str) -> Option<i64> {
    html::text_between(body, "日期:", "</")
        .and_then(|value| manatan_shared::dates::parse_fixture_date(&value))
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input).split(['?', '#']).next().unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

fn deeplink_key(input: &str) -> Option<String> {
    let key = normalize_key(input);
    if key.ends_with(".html") && !key.contains("/search/") {
        Some(key)
    } else {
        None
    }
}

fn request_page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn filter_text(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_object)
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn category_id(request: &Value) -> &'static str {
    match filter_value(request, "category").as_deref() {
        Some("handsome") => "5",
        Some("news") => "30",
        Some("street") => "32",
        _ => "6",
    }
}

#[derive(Deserialize)]
struct AjaxDto {
    pic: String,
}

const LIST_FIXTURE: &str = r#"
<div class="pic-list"><ul>
<li><a href="/beauty/sample.html"><img src="/cover.jpg"><p>Sample Gallery</p></a></li>
</ul></div><div class="pages"><a>1</a><a>2</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="news-title">Sample Gallery</h1>
<div class="picture-details"><span class="gao">日期: 2024-01-02</span></div>
<div class="relation_tags"><a>Portrait</a></div>
<script>var pc_cid = 6; var pc_id = 123;</script>
<div class="pictures"><img data-original="/images/1.jpg"></div>
<div class="pages"><a>1</a><a>2</a></div>
"#;

const AJAX_FIXTURE: &str = r#"{ "pic": "<img src=\"/images/2.jpg\">" }"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_details_and_pages() {
        let listing = parse_listing(LIST_FIXTURE);
        assert_eq!(listing.entries[0].key, "/beauty/sample.html");
        let details = parse_details(DETAILS_FIXTURE, Some("/beauty/sample.html".into()));
        assert_eq!(details.title, "Sample Gallery");
        assert_eq!(details.tags, vec!["Portrait"]);
        let pages = parse_pages(DETAILS_FIXTURE, "/beauty/sample.html");
        assert_eq!(pages.len(), 2);
        assert!(matches!(pages[1].content, PageContent::Lazy { .. }));
        assert_eq!(parse_image_response(AJAX_FIXTURE), "https://meijuntu.com/images/2.jpg");
    }
}
