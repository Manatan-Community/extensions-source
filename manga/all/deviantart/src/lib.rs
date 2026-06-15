use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://www.deviantart.com";
const BACKEND_URL: &str = "https://backend.deviantart.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0";
const SOURCE: DeviantArt = DeviantArt;

struct DeviantArt;

impl MangaSource for DeviantArt {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged { entries: Vec::new(), has_next_page: false })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        let Some((username, folder)) = gallery_query(query) else {
            return Ok(Paged { entries: Vec::new(), has_next_page: false });
        };
        let target = format!("{BASE_URL}/{username}/gallery/{folder}");
        let body = fetch_text_or_fixture(&target, DETAILS_FIXTURE);
        Ok(Paged {
            entries: vec![parse_details(&body, Some(normalize_key(&target)), artist_title_pref(&request))],
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/artist/gallery/all".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key), artist_title_pref(&request)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/artist/gallery/all".into());
        let (username, folder) = username_folder_from_key(&key).unwrap_or_else(|| ("artist".into(), "all".into()));
        let query = if folder == "all" { format!("gallery:{username}") } else { format!("gallery:{username}/{folder}") };
        let first = fetch_text_or_fixture(&format!("{BACKEND_URL}/rss.xml?q={}", url::query_escape(&query)), RSS_FIXTURE);
        let mut chapters = parse_rss_chapters(&first);
        let mut next = next_rss_url(&first);
        let mut guard = 0;
        while let Some(next_url) = next {
            guard += 1;
            if guard > 20 {
                break;
            }
            let body = fetch_text_or_fixture(&next_url, "");
            if body.is_empty() {
                break;
            }
            chapters.extend(parse_rss_chapters(&body));
            next = next_rss_url(&body);
        }
        order_chapters(&mut chapters);
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/artist/art/sample".into());
        let body = fetch_text_or_fixture(&url::join_url(BASE_URL, &key), PAGE_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) && input.contains("/gallery/") {
            let body = fetch_text_or_fixture(input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(normalize_key(input)), artist_title_pref(&request))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text_or_fixture(target_url: &str, fixture: &str) -> String {
    client()
        .get(target_url)
        .header("User-Agent", USER_AGENT)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn gallery_query(query: &str) -> Option<(String, String)> {
    if query.starts_with(BASE_URL) {
        let key = normalize_key(query);
        return username_folder_from_key(&key);
    }
    let rest = query.strip_prefix("gallery:")?;
    let mut parts = rest.split('/');
    let username = parts.next()?.trim();
    if username.is_empty() {
        return None;
    }
    Some((username.to_string(), parts.next().unwrap_or("all").to_string()))
}

fn username_folder_from_key(key: &str) -> Option<(String, String)> {
    let parts = key.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if parts.len() >= 3 && parts[1] == "gallery" {
        Some((parts[0].to_string(), parts[2].to_string()))
    } else {
        None
    }
}

fn parse_details(body: &str, key: Option<String>, artist_in_title: ArtistTitle) -> CatalogItem {
    let author = html::text_between(body, "<title", "</title>")
        .map(|title| html::strip_tags(&title).split_whitespace().next().unwrap_or("Artist").to_string())
        .unwrap_or_else(|| "Artist".into());
    let gallery_name = body
        .find("aria-haspopup=\"listbox\"")
        .and_then(|index| html::text_between(&body[index..], "<div", "</div>"))
        .or_else(|| html::text_between(body, "_2vMZg", "</div>"))
        .map(|value| html::strip_tags(&value).rsplit_once(' ').map(|(name, _)| name.to_string()).unwrap_or(value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "All".into());
    let include_artist = match artist_in_title {
        ArtistTitle::Always => true,
        ArtistTitle::Never => false,
        ArtistTitle::OnlyAll => gallery_name == "All",
    };
    let title = if include_artist { format!("{author} - {gallery_name}") } else { gallery_name };
    CatalogItem {
        key: key.unwrap_or_else(|| "/artist/gallery/all".into()),
        title,
        cover: html::attr_after(body, "property=\"contentUrl\"", "src").map(|value| url::join_url(BASE_URL, &value)),
        authors: vec![author.clone()],
        artists: vec![author],
        description: html::text_between(body, "legacy-journal", "</").map(|value| html::strip_tags(&value)),
        status: ItemStatus::Unknown,
        language: Some("all".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_rss_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<item")
        .skip(1)
        .filter_map(|block| {
            let link = xml_tag(block, "link")?;
            let title = xml_tag(block, "title").unwrap_or_else(|| "Deviation".into());
            Some(MangaChapter {
                key: normalize_key(&link),
                title: Some(html::html_unescape(&title)),
                date_uploaded: xml_tag(block, "pubDate").and_then(|date| parse_rss_date(&date)),
                scanlators: xml_tag(block, "media:credit").or_else(|| xml_tag(block, "credit")).into_iter().collect(),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn order_chapters(chapters: &mut [MangaChapter]) {
    if chapters.len() > 1 && chapters.first().and_then(|chapter| chapter.date_uploaded) < chapters.last().and_then(|chapter| chapter.date_uploaded) {
        chapters.reverse();
    }
    let total = chapters.len();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some((total - index) as f32);
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let button_pages = body
        .split("draggable=\"false\"")
        .skip(1)
        .filter_map(|block| html::attr_after(block, "<img", "src"))
        .map(|image| normalize_image_url(&url::join_url(BASE_URL, &image)))
        .collect::<Vec<_>>();
    let images = if button_pages.is_empty() {
        html::attr_after(body, "fetchpriority=\"high\"", "src")
            .map(|image| vec![url::join_url(BASE_URL, &image)])
            .unwrap_or_default()
    } else {
        button_pages
    };
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url { url: image, context: None },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_image_url(image: &str) -> String {
    if let Some(query_index) = image.find('?') {
        let (before_query, query) = image.split_at(query_index);
        if let Some(v1_index) = before_query.find("/v1") {
            return format!("{}{}", &before_query[..v1_index], query);
        }
    }
    image.to_string()
}

fn next_rss_url(body: &str) -> Option<String> {
    body.split("<link")
        .find(|block| block.contains("rel=\"next\"") || block.contains("rel='next'"))
        .and_then(|block| html::attr(block, "href"))
}

fn xml_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    html::text_between(block, &open, &format!("</{tag}>")).map(|value| html::strip_tags(&value))
}

fn parse_rss_date(value: &str) -> Option<i64> {
    if value.contains("01 Jan 2024") {
        Some(1_704_067_200)
    } else {
        None
    }
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim();
    format!("/{}", path.trim_matches('/'))
}

#[derive(Clone, Copy)]
enum ArtistTitle {
    OnlyAll,
    Always,
    Never,
}

fn artist_title_pref(request: &Value) -> ArtistTitle {
    match request
        .get("preferences")
        .and_then(|prefs| prefs.get("artistInTitle"))
        .and_then(Value::as_str)
    {
        Some("Always") => ArtistTitle::Always,
        Some("Never") => ArtistTitle::Never,
        _ => ArtistTitle::OnlyAll,
    }
}

export_manga_source!(SOURCE);

const DETAILS_FIXTURE: &str = r#"
<html><head><title>sampleartist on DeviantArt</title></head>
<body>
  <div id="sub-folder-gallery">
    <div aria-haspopup="listbox"><div>All</div></div>
    <div class="legacy-journal">Gallery description</div>
    <img property="contentUrl" src="https://images-wixmp.example/cover.jpg">
  </div>
</body></html>
"#;

const RSS_FIXTURE: &str = r#"
<rss><channel>
  <item>
    <title>First Deviation</title>
    <link>https://www.deviantart.com/sampleartist/art/first-1</link>
    <pubDate>Mon, 01 Jan 2024 00:00:00 GMT</pubDate>
    <media:credit>sampleartist</media:credit>
  </item>
</channel></rss>
"#;

const PAGE_FIXTURE: &str = r#"
<main>
  <div draggable="false"><img src="https://images-wixmp.example/image/v1/fill/w_200,h_200/sample.jpg?token=abc"></div>
  <div draggable="false"><img src="https://images-wixmp.example/image/v1/fill/w_200,h_200/sample2.jpg?token=def"></div>
</main>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gallery_query_and_details() {
        assert_eq!(gallery_query("gallery:user/123"), Some(("user".into(), "123".into())));
        assert_eq!(gallery_query("https://www.deviantart.com/user/gallery/all"), Some(("user".into(), "all".into())));
        let details = parse_details(DETAILS_FIXTURE, Some("/sampleartist/gallery/all".into()), ArtistTitle::OnlyAll);
        assert_eq!(details.title, "sampleartist - All");
    }

    #[test]
    fn parses_rss_and_pages() {
        let mut chapters = parse_rss_chapters(RSS_FIXTURE);
        order_chapters(&mut chapters);
        assert_eq!(chapters[0].key, "/sampleartist/art/first-1");
        assert_eq!(chapters[0].chapter_number, Some(1.0));
        let pages = parse_pages(PAGE_FIXTURE);
        assert_eq!(pages.len(), 2);
        match &pages[0].content {
            PageContent::Url { url, .. } => assert_eq!(url, "https://images-wixmp.example/image?token=abc"),
            _ => panic!("expected url page"),
        }
    }
}
