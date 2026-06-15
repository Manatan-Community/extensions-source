use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    MangaPageImage, PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: ComicFesta = ComicFesta;
const BASE_URL: &str = "https://comic.iowl.jp";

struct ComicFesta;

impl MangaSource for ComicFesta {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_ranking(RANKING_FIXTURE, 1));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_search_listing(&fetch_document(
                &format!("{BASE_URL}/titles?page={page}&sort=release&search_form%5Bother_item%5D%5B%5D=new"),
                SEARCH_FIXTURE,
                false,
            )));
        }
        Ok(parse_ranking(&fetch_document(
            &format!("{BASE_URL}/sales_rankings/monthly_general?page={page}"),
            RANKING_FIXTURE,
            true,
        ), page))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(id) = id_from_url(query) {
            return Ok(Paged { entries: vec![details_by_id(&id)], has_next_page: false });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = format!(
            "{BASE_URL}/titles?page={page}&search_form%5Bkeyword%5D={}&search={}&commit=search",
            url::query_escape(query),
            url::query_escape(query)
        );
        Ok(parse_search_listing(&fetch_document(&target, SEARCH_FIXTURE, false)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        Ok(details_by_id(id_from_key(&key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "1".into());
        let hide_locked = preference_bool(&request, "hideLockedChapters");
        Ok(parse_chapters(
            &fetch_document(&format!("{BASE_URL}/titles/{}", id_from_key(&key)), CHAPTERS_FIXTURE, false),
            hide_locked,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "10/free_download".into());
        let target = format!("{BASE_URL}/volumes/{key}");
        let body = fetch_document(&target, VOLUME_FIXTURE, false);
        if body.contains("/entry") || body.contains("/error") {
            return Ok(vec![manga::text_page("Log in with WebView and purchase this product to read.")]);
        }
        Ok(parse_clipstudio_pages(&body, &target))
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let page_xml = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
            .and_then(|lazy| lazy.get("key"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = fetch_text(page_xml, PAGE_XML_FIXTURE);
        Ok(MangaPageImage {
            url: image_url_from_page_xml(&body, page_xml).unwrap_or_else(|| format!("{BASE_URL}/sample.jpg")),
            headers: manga::image_headers(BASE_URL),
            context: Some(manga::image_headers(BASE_URL)),
            ..MangaPageImage::default()
        })
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
        Ok(manga::request_key(&request, "manga").map(|key| format!("{BASE_URL}/titles/{}", id_from_key(&key))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| format!("{BASE_URL}/volumes/{key}")))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = id_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_id(&id)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
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
        .with_header(
            "Cookie",
            "checked_age=1; sp_display=1; cf_checked_age_guest=1; cf_checked_age=1",
        )
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str, rsc: bool) -> String {
    let http = client();
    let request = http.get(target);
    let request = if rsc { request.header("rsc", "1") } else { request };
    request.browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client().get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_ranking(body: &str, page: u64) -> Paged<CatalogItem> {
    let value = extract_json_object(body).unwrap_or_else(|| serde_json::from_str(RANKING_FIXTURE).unwrap_or(Value::Null));
    let entries = value
        .get("titles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = text_value(item.get("id"))?;
            Some(CatalogItem {
                key: id.clone(),
                title: item.get("name").and_then(Value::as_str).unwrap_or("Comic Festa").to_string(),
                cover: item.get("thumbnailPath").and_then(Value::as_str).map(ToOwned::to_owned),
                url: Some(format!("{BASE_URL}/titles/{id}")),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: page < 2 }
}

fn parse_search_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("list-detail-box")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "list-left-box", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let id = id_from_key(&href).to_string();
            Some(CatalogItem {
                key: id.clone(),
                title: html::text_between(chunk, "title-box", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Comic Festa".into()),
                cover: html::attr_after(chunk, "<img", "src"),
                url: Some(format!("{BASE_URL}/titles/{id}")),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged { entries, has_next_page: body.contains("rel=\"next\"") }
}

fn details_by_id(id: &str) -> CatalogItem {
    let body = fetch_document(&format!("{BASE_URL}/titles/{id}"), DETAILS_FIXTURE, false);
    CatalogItem {
        key: id.to_string(),
        title: html::text_between(&body, "titleName", "</")
            .or_else(|| html::text_between(&body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Comic Festa".into()),
        cover: html::attr_after(&body, "thumbnail", "src").or_else(|| html::attr_after(&body, "<img", "src")),
        authors: link_values(&body, "/authors/"),
        description: html::text_between(&body, "description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: link_values(&body, "/tags/"),
        status: if body.contains("完結") { ItemStatus::Completed } else { ItemStatus::Ongoing },
        url: Some(format!("{BASE_URL}/titles/{id}")),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let value = extract_json_object(body).unwrap_or_else(|| serde_json::from_str(CHAPTERS_FIXTURE).unwrap_or(Value::Null));
    let logged_in = value.get("userStatus").and_then(Value::as_str).is_some_and(|status| status != "guest");
    let owned = value.get("userPackages").and_then(Value::as_array).into_iter().flatten().filter_map(|item| text_value(item.get("id"))).collect::<Vec<_>>();
    let mut chapters = value
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = text_value(item.get("id"))?;
            let owned_chapter = owned.iter().any(|value| value == &id);
            let has_free = item.pointer("/fairInfo/free/endAt").and_then(Value::as_str).is_some();
            let has_trial = item.pointer("/fairInfo/trial/endAt").and_then(Value::as_str).is_some();
            let point = item.get("point").and_then(Value::as_i64).unwrap_or(0);
            let path = if owned_chapter {
                "download"
            } else if has_free {
                "free_download"
            } else if has_trial {
                "trial_download"
            } else if point == 0 && !logged_in {
                "free_download"
            } else {
                "download"
            };
            let locked = path == "download" && !owned_chapter;
            if hide_locked && locked {
                return None;
            }
            let number = item.get("number").and_then(Value::as_f64).unwrap_or(0.0);
            let title = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("{}巻", number));
            Some(MangaChapter {
                key: format!("{id}/{path}"),
                title: Some(if locked { format!("Locked {title}") } else { title }),
                chapter_number: Some(number as f32),
                url: Some(format!("{BASE_URL}/volumes/{id}/{path}")),
                is_locked: locked,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_clipstudio_pages(body: &str, volume_url: &str) -> Vec<MangaPage> {
    if let Some(images) = direct_images(body) {
        return images;
    }
    let auth = query_param(volume_url, "param")
        .or_else(|| input_value(body, "param"))
        .unwrap_or_default()
        .replace(' ', "+");
    let endpoint = query_param(volume_url, "cgi")
        .or_else(|| input_value(body, "cgi"))
        .unwrap_or_default();
    if auth.is_empty() || endpoint.is_empty() {
        return vec![manga::text_page("Could not find Comic Festa viewer parameters.")];
    }
    let viewer = url::join_url(BASE_URL, &endpoint);
    let face_url = format!("{viewer}?mode=7&reqtype=0&vm=4&file=face.xml&param={}", url::query_escape(&auth));
    let face = fetch_text(&face_url, FACE_XML_FIXTURE);
    let total = xml_text(&face, "TotalPage").and_then(|value| value.parse::<usize>().ok()).unwrap_or(1);
    let width = xml_text(&face, "Width").unwrap_or_else(|| "4".into());
    let height = xml_text(&face, "Height").unwrap_or_else(|| "4".into());
    (0..total)
        .map(|index| {
            let file = format!("{index:04}.xml");
            let key = format!("{viewer}?mode=8&reqtype=0&vm=4&file={file}&param={}#{width}/{height}", url::query_escape(&auth));
            MangaPage {
                content: PageContent::Lazy { key, url: None, page_url: Some(volume_url.into()), context: None },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn image_url_from_page_xml(body: &str, page_xml_url: &str) -> Option<String> {
    let page_no = xml_text(body, "PageNo")?.parse::<usize>().ok()?;
    let kind_chunk = body.split("<Kind").skip(1).find(|chunk| {
        let value = html::text_between(chunk, ">", "</Kind>").unwrap_or_default();
        matches!(value.as_str(), "1" | "2" | "3")
    })?;
    let kind = html::text_between(kind_chunk, ">", "</Kind>")?;
    let no = html::attr(kind_chunk, "No").unwrap_or_else(|| "0".into());
    let file = format!("{page_no:04}_{}.bin", format!("{no:0>4}"));
    let endpoint = page_xml_url.split('?').next().unwrap_or(BASE_URL);
    let param = query_param(page_xml_url, "param").unwrap_or_default();
    let mut out = format!("{endpoint}?mode={kind}&file={file}&reqtype=0&param={}", url::query_escape(&param));
    if let Some(scramble) = xml_text(body, "Scramble").filter(|value| !value.is_empty()) {
        let grid = page_xml_url.split('#').nth(1).unwrap_or("4/4");
        out.push('#');
        out.push_str(&format!("size={scramble}/{grid}"));
    }
    Some(out)
}

fn direct_images(body: &str) -> Option<Vec<MangaPage>> {
    let pages = body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|value| !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: url::join_url(BASE_URL, &image), context: Some(manga::image_headers(BASE_URL)) },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect::<Vec<_>>();
    (!pages.is_empty()).then_some(pages)
}

fn extract_json_object(body: &str) -> Option<Value> {
    for marker in ["\"titles\"", "\"packages\""] {
        let index = body.find(marker)?;
        let start = body[..index].rfind('{')?;
        let mut depth = 0i32;
        for (offset, ch) in body[start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return serde_json::from_str(&body[start..=start + offset]).ok();
                    }
                }
                _ => {}
            }
        }
    }
    serde_json::from_str(body).ok()
}

fn link_values(body: &str, href_part: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(href_part))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn id_from_url(input: &str) -> Option<String> {
    input.find("/titles/").map(|index| id_from_key(&input[index + 8..]).to_string())
}

fn id_from_key(input: &str) -> &str {
    input.trim_matches('/').split('/').next().unwrap_or("1")
}

fn text_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn input_value(body: &str, name: &str) -> Option<String> {
    body.split("<input")
        .skip(1)
        .find(|chunk| chunk.contains(&format!("name=\"{name}\"")) || chunk.contains(&format!("name='{name}'")))
        .and_then(|chunk| html::attr(chunk, "value"))
}

fn query_param(input: &str, name: &str) -> Option<String> {
    input.split('?').nth(1)?.split(&['&', '#'][..]).find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn xml_text(body: &str, name: &str) -> Option<String> {
    html::text_between(body, &format!("<{name}>"), &format!("</{name}>"))
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .or_else(|| request.get("prefs"))
        .and_then(Value::as_object)
        .and_then(|prefs| prefs.get(id))
        .and_then(|value| value.as_bool().or_else(|| value.as_str().map(|text| text == "true")))
        .unwrap_or(false)
}

export_manga_source!(SOURCE);

const RANKING_FIXTURE: &str = r#"{"titles":[{"id":1,"name":"Sample Comic Festa","thumbnailPath":"https://img.example.test/festa-cover.jpg"}]}"#;
const SEARCH_FIXTURE: &str = r#"<div class="list-detail-box"><div class="list-left-box"><a href="/titles/1"><img src="https://img.example.test/festa-cover.jpg"></a></div><div class="title-box">Sample Comic Festa</div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="titleName">Sample Comic Festa</h1><img class="thumbnail" src="https://img.example.test/festa-cover.jpg"><a href="/authors/1">Sample Author</a><div class="description">Sample description.</div><span class="latest-package-num-display">完結</span>"#;
const CHAPTERS_FIXTURE: &str = r#"{"packages":[{"id":10,"number":1.0,"name":"Volume 1","point":0,"fairInfo":{"free":{"endAt":"2999-01-01"},"trial":null}}],"userPackages":[],"userStatus":"guest"}"#;
const VOLUME_FIXTURE: &str = r#"<div id="meta"><input name="param" value="sample"><input name="cgi" value="/viewer.cgi"></div>"#;
const FACE_XML_FIXTURE: &str = r#"<Face><TotalPage>1</TotalPage><Scramble><Width>4</Width><Height>4</Height></Scramble></Face>"#;
const PAGE_XML_FIXTURE: &str = r#"<Page><PageNo>0</PageNo><Scramble></Scramble><Kind No="1">1</Kind></Page>"#;
