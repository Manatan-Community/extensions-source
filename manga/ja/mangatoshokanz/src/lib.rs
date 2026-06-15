use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    html, manga,
    sdk::http::{Headers, HttpClient},
    url,
    virgo::VirgoCrypto,
};
use serde_json::{Value, json};

const SOURCE: MangaToshokanZ = MangaToshokanZ;
const BASE_URL: &str = "https://www.mangaz.com";
const R18_URL: &str = "https://r18.mangaz.com";
const VIRGO_URL: &str = "https://vw.mangaz.com/virgo";

struct MangaToshokanZ;

impl MangaSource for MangaToshokanZ {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_list(POPULAR_FIXTURE, false));
        }
        let page = page(&request);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            Ok(parse_list(
                &fetch_document(&format!(
                    "{BASE_URL}/title/addpage_renewal?type=official&sort=new&page={page}"
                )),
                true,
            ))
        } else {
            Ok(parse_list(
                &fetch_document(&format!("{BASE_URL}/ranking/views")),
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
        let category = filter_string(&request, "category").unwrap_or_default();
        let sort = filter_string(&request, "sort").unwrap_or_else(|| "popular".into());
        let host = if category == "r18" { R18_URL } else { BASE_URL };
        let mut target = format!(
            "{host}/title/addpage_renewal?query={}&page={page}&sort={}",
            url::query_escape(query),
            url::query_escape(&sort)
        );
        if !category.is_empty() {
            target.push_str("&category=");
            target.push_str(&url::query_escape(&category));
        }
        Ok(parse_list(&fetch_document(&target), true))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "202371".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "202371".into());
        Ok(parse_chapters(&fetch_document(&format!(
            "{BASE_URL}/series/detail/{}",
            series_id(&key)
        ))))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "202371".into());
        Ok(fetch_pages(&book_id(&key)).unwrap_or_else(|| parse_pages(PAGES_FIXTURE, &book_id(&key))))
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

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/series/detail/{}", series_id(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{BASE_URL}/book/detail/{}", book_id(&key))))
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
        .with_header("Cookie", "_LANG_=ja")
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .header("X-Requested-With", "XMLHttpRequest")
        .send_text()
        .unwrap_or_else(|_| POPULAR_FIXTURE.to_string())
}

fn fetch_pages(chapter_id: &str) -> Option<Vec<MangaPage>> {
    let ticket = fetch_ticket(chapter_id)?;
    let serial = fetch_serial()?;
    let keys = VirgoCrypto::key_pair_for(chapter_id)?;
    let target = format!("{VIRGO_URL}/docx/{chapter_id}.json");
    let response = client()
        .post(target)
        .header("Cookie", format!("virgo!__ticket={ticket}; _LANG_=ja"))
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[
            ("__serial", serial.as_str()),
            ("__ticket", ticket.as_str()),
            ("pub", keys.public_pem.as_str()),
        ])
        .send_text()
        .ok()?;
    Some(parse_pages(&response, chapter_id))
        .filter(|pages| !pages.is_empty())
        .or_else(|| {
            let decoded = VirgoCrypto::decrypt_pages(&response, &keys.private_der_base64)?;
            Some(
                decoded
                    .files
                    .into_iter()
                    .enumerate()
                    .map(|(index, file)| {
                        let stem = file.split('.').next().unwrap_or(&file);
                        let image_url = format!("{}{}{}.jpg", decoded.base_url, decoded.path_prefix, stem);
                        page_entry(index, image_url, None)
                    })
                    .collect(),
            )
        })
}

fn fetch_ticket(chapter_id: &str) -> Option<String> {
    let target = format!("{VIRGO_URL}/view/{chapter_id}");
    let response = client()
        .fetch("HEAD", &target, None, Headers::new())
        .or_else(|_| client().get(&target).browser_document().send()) // Some hosts omit Set-Cookie on HEAD fallback in local runners.
        .ok()?;
    response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .find_map(|(_, value)| cookie_value(value, "virgo!__ticket"))
        .or_else(|| {
            manatan_extension::abi::cookies_get(&target)
                .ok()
                .and_then(|response| response.header)
                .and_then(|header| cookie_value(&header, "virgo!__ticket"))
        })
}

fn fetch_serial() -> Option<String> {
    let body = client()
        .get(format!("{VIRGO_URL}/app.js"))
        .browser_document()
        .send_text()
        .ok()?;
    text_between_raw(&body, "__serial = \"", "\";")
}

fn parse_list(body: &str, paged: bool) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .filter(|chunk| chunk.contains("<h4") && chunk.contains("<a"))
        .filter(|chunk| !chunk.contains("iconConsent"))
        .filter_map(parse_list_item)
        .collect::<Vec<_>>();
    let has_next_page = paged && entries.len() >= 50;
    Paged {
        entries,
        has_next_page,
    }
}

fn parse_list_item(chunk: &str) -> Option<CatalogItem> {
    let title_block = text_between_raw(chunk, "<h4", "</h4>")?;
    let href = html::attr_after(&title_block, "<a", "href")?;
    let title = html::strip_tags(&title_block);
    let cover = html::attr_after(chunk, "<img", "data-src")
        .or_else(|| html::attr_after(chunk, "<img", "src"));
    Some(CatalogItem {
        key: series_id(&href),
        title,
        cover: cover.map(|value| absolute_url(&value)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        url: Some(absolute_series_url(&href)),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    let probe = fetch_document(&format!("{BASE_URL}/series/detail/{}", series_id(key)));
    let cover = first_img(&probe);
    let details_key = cover
        .as_deref()
        .and_then(book_id_from_cover)
        .unwrap_or_else(|| series_id(key));
    parse_details(
        &fetch_document(&format!("{BASE_URL}/book/detail/{details_key}")),
        key,
        cover,
    )
}

fn parse_details(body: &str, key: &str, cover: Option<String>) -> CatalogItem {
    let title = text_between_raw(body, "GA4_booktitle", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| text_between_raw(body, "<h1", "</h1>").map(|value| html::strip_tags(&value)))
        .unwrap_or_else(|| "Manga Toshokan Z".into());
    let mut authors = Vec::new();
    let mut artists = Vec::new();
    for chunk in body.split("<li").filter(|chunk| chunk.contains("detailAuthor")) {
        let name = text_between_raw(chunk, "<a", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty());
        if chunk.contains("作画") || chunk.contains("マンガ") {
            if let Some(name) = name {
                artists.push(name);
            }
        } else if chunk.contains('者') || chunk.contains("原作") {
            if let Some(name) = name {
                authors.push(name);
            }
        }
    }
    let description = text_between_raw(body, "wordbreak", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let tags = body
        .split("inductionTags")
        .flat_map(|chunk| chunk.split("</a>").take(24))
        .filter_map(|chunk| text_between_raw(chunk, "<a", ""))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect();
    CatalogItem {
        key: series_id(key),
        title,
        cover,
        authors,
        artists,
        description,
        tags,
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        status: if body.contains("iconContinues") {
            ItemStatus::Ongoing
        } else if body.contains("iconEnd") {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        url: Some(format!("{BASE_URL}/series/detail/{}", series_id(key))),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    if body.contains("GA4_booktitle") && body.contains("/book/detail/") {
        let id = key_from_url(body).unwrap_or_else(|| "202371".into());
        return vec![chapter_entry(0, id, Some(html::strip_tags(body)))];
    }
    let mut chapters = body
        .split("<li")
        .filter(|chunk| chunk.contains("/book/detail/") && chunk.contains("title"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = text_between_raw(chunk, "title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            Some((href, title))
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
        .into_iter()
        .enumerate()
        .map(|(index, (href, title))| chapter_entry(index, book_id(&href), title))
        .collect()
}

fn chapter_entry(index: usize, key: String, title: Option<String>) -> MangaChapter {
    MangaChapter {
        key: book_id(&key),
        title,
        chapter_number: Some((index + 1) as f32),
        language: Some("ja".into()),
        url: Some(format!("{BASE_URL}/book/detail/{}", book_id(&key))),
        source_order: Some(index as i32),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str, chapter_id: &str) -> Vec<MangaPage> {
    let Some(keys) = VirgoCrypto::key_pair_for(chapter_id) else {
        return Vec::new();
    };
    let Some(decoded) = VirgoCrypto::decrypt_pages(body, &keys.private_der_base64) else {
        return Vec::new();
    };
    decoded
        .files
        .into_iter()
        .enumerate()
        .map(|(index, file)| {
            let stem = file.split('.').next().unwrap_or(&file);
            page_entry(
                index,
                format!("{}{}{}.jpg", decoded.base_url, decoded.path_prefix, stem),
                Some(format!("{BASE_URL}/book/detail/{chapter_id}")),
            )
        })
        .collect()
}

fn page_entry(index: usize, image_url: String, referer: Option<String>) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image_url.clone(),
            context: Some(manga::image_headers(referer.as_deref().unwrap_or(BASE_URL))),
        },
        headers: manga::image_headers(referer.as_deref().unwrap_or(BASE_URL)),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn first_img(body: &str) -> Option<String> {
    html::attr_after(body, "<img", "data-src")
        .or_else(|| html::attr_after(body, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn key_from_url(input: &str) -> Option<String> {
    if !input.contains("mangaz.com") {
        return None;
    }
    input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn series_id(input: &str) -> String {
    input
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(input)
        .to_string()
}

fn book_id(input: &str) -> String {
    series_id(input)
}

fn book_id_from_cover(input: &str) -> Option<String> {
    input
        .trim_end_matches('/')
        .rsplit('/')
        .nth(1)
        .filter(|value| value.chars().all(|ch| ch.is_ascii_digit()))
        .map(ToOwned::to_owned)
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        url::join_url(BASE_URL, input)
    }
}

fn absolute_series_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        format!("{BASE_URL}/series/detail/{}", series_id(input))
    }
}

fn cookie_value(header: &str, name: &str) -> Option<String> {
    header
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&format!("{name}=")).map(ToOwned::to_owned))
}

fn text_between_raw(input: &str, start: &str, end: &str) -> Option<String> {
    let start_index = input.find(start)? + start.len();
    let rest = &input[start_index..];
    let text_start = rest.find('>').map(|index| index + 1).unwrap_or(0);
    let rest = &rest[text_start..];
    let end_index = if end.is_empty() {
        rest.len()
    } else {
        rest.find(end)?
    };
    Some(rest[..end_index].to_string())
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"
<ul class="itemList">
  <li><a href="/series/detail/202371"><img data-src="https://books.j-comi.jp/Books/202/202371/thumb160.jpg"></a><h4><a href="/series/detail/202371">Sample Manga</a></h4></li>
</ul>
"#;

const PAGES_FIXTURE: &str = r#"{
  "bi":"AAAAAAAAAAAAAAAAAAAAAA==",
  "ek":"",
  "data":""
}"#;
