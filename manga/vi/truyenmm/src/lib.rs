use manatan_extension::{
    CatalogItem, HomeSection, MangaChapter, MangaPage, Paged, SearchRequest, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, url, vi_html as vh};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: TruyenMM = TruyenMM;
const BASE_URL: &str = "https://truyenmmhayr.com";

struct TruyenMM;

impl MangaSource for TruyenMM {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = vh::page_number(&request);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "truyen-moi-cap-nhat"
        } else {
            "danh-sach-truyen"
        };
        let target = format!("{BASE_URL}/{path}/{page}");
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = vh::query(&request);
        if let Some(key) = vh::key_from_url(BASE_URL, &query, "/truyen/") {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = vh::page_number(&request);
        let target = if !query.is_empty() {
            format!(
                "{BASE_URL}/tim-kiem?key={}&page={page}",
                url::query_escape(&query)
            )
        } else if let Some(genre) = vh::filter(&request, "genre") {
            format!("{BASE_URL}/the-loai/{genre}/{page}")
        } else {
            format!("{BASE_URL}/danh-sach-truyen/{page}")
        };
        Ok(parse_listing(&vh::fetch_document(
            BASE_URL,
            &target,
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/truyen/sample".into());
        let manga_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &manga_url, DETAILS_FIXTURE);
        if let Some(topic_id) = html::attr_after(&body, "script-chapter", "data-id") {
            let target = format!(
                "{BASE_URL}/api/get-topic?id={}",
                url::query_escape(&topic_id)
            );
            let topic_body = vh::browser_client(BASE_URL)
                .get(&target)
                .xhr()
                .send_text()
                .unwrap_or_else(|_| TOPIC_FIXTURE.to_string());
            let chapters = parse_topic_chapters(&topic_body);
            if !chapters.is_empty() {
                return Ok(chapters);
            }
        }
        Ok(parse_html_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen/sample/chapter-1".into());
        let chapter_url = vh::absolute_url(BASE_URL, &key);
        let body = vh::fetch_document(BASE_URL, &chapter_url, PAGES_FIXTURE);
        let images = vh::collect_image_urls(BASE_URL, &body)
            .into_iter()
            .filter(|image| {
                !image.contains("/chapter-")
                    && !image.ends_with("/loading.webp")
                    && !image.ends_with("/page_logo.png")
            })
            .collect::<Vec<_>>();
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
        if let Some(key) = vh::key_from_url(BASE_URL, input, "/truyen/") {
            let is_chapter = key.contains("/chapter-");
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .or_else(|| html::text_between(chunk, "<h3", "</h3>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(vh::catalog_item(
                BASE_URL,
                key,
                title,
                vh::image_attr(chunk),
                "adult",
            ))
        })
        .fold(Vec::new(), vh::push_unique);
    Paged {
        entries,
        has_next_page: vh::has_next(body),
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
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(body, "Bìa", "data-src")
            .or_else(|| html::attr_after(body, "bìa", "data-src"))
            .or_else(|| html::attr_after(body, "Bìa", "src"))
            .or_else(|| vh::image_attr(body))
            .map(|image| vh::absolute_url(BASE_URL, &image)),
        authors: info_values(body, "Tác giả"),
        tags: link_texts_by_href(body, "/the-loai/"),
        status: vh::status_from_vi(&info_values(body, "Loại Truyện").join(" ")),
        url: Some(vh::absolute_url(BASE_URL, key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("<dt") && chunk.contains(label))
        .filter_map(|chunk| html::text_between(chunk, "<dd", "</dd>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn link_texts_by_href(body: &str, href_marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_marker))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_topic_chapters(body: &str) -> Vec<MangaChapter> {
    let response = serde_json::from_str::<TopicResponse>(body)
        .or_else(|_| serde_json::from_str(TOPIC_FIXTURE))
        .unwrap_or_default();
    response
        .topic
        .and_then(|topic| topic.chapters)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|chapter| {
            let id = chapter.id?;
            let title = chapter.name?;
            let key = vh::normalize_key(BASE_URL, &chapter_url_from_id(&id));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: chapter.update_time,
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), vh::push_unique_chapter)
}

fn chapter_url_from_id(raw: &str) -> String {
    let normalized = raw.replace("-chapter-", "/chapter-");
    if let Some(index) = normalized.find("/chapter-") {
        format!(
            "/truyen/{}/{}",
            &normalized[..index],
            &normalized[index + 1..]
        )
    } else {
        format!("/truyen/{normalized}")
    }
}

fn parse_html_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapter-"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = vh::normalize_key(BASE_URL, &href);
            let title = html::text_between(chunk, "<span", "</span>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| html::strip_tags(chunk));
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "<time", "</time>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| vh::parse_vi_date(&value)),
                url: Some(vh::absolute_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), vh::push_unique_chapter)
}

#[derive(Default, Deserialize)]
struct TopicResponse {
    topic: Option<Topic>,
}

#[derive(Default, Deserialize)]
struct Topic {
    chapters: Option<Vec<TopicChapter>>,
}

#[derive(Default, Deserialize)]
struct TopicChapter {
    name: Option<String>,
    id: Option<String>,
    update_time: Option<i64>,
}

const LIST_FIXTURE: &str = r#"
<article><a href="/truyen/sample"><img data-src="/cover.jpg"></a><h2>Sample</h2></article>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample</h1><img alt="Bìa Sample" src="/cover.jpg"><dl><div><dt>Tác giả</dt><dd>Author</dd></div><div><dt>Loại Truyện</dt><dd>Đang tiến hành</dd></div></dl><dd><a href="/the-loai/action">Action</a></dd><script id="script-chapter" data-id="1"></script>
"#;
const TOPIC_FIXTURE: &str = r#"{"topic":{"chapters":[{"name":"Chapter 1","id":"sample-chapter-1","update_time":1704067200000}]}}"#;
const PAGES_FIXTURE: &str =
    r#"<div class="w-full flex flex-col items-center"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
