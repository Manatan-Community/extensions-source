use madara_manga::{
    chapters_endpoint_url, listing_url, parse_chapters_html, parse_details_html,
    parse_listing_html, parse_pages_html, parse_relative_date_at, parse_search_html, search_url,
    MadaraMangaConfig,
};
use manatan_sdk::{html, PageContent};
use serde_json::json;

struct TestConfig;
impl MadaraMangaConfig for TestConfig {
    const BASE_URL: &'static str = "https://lhtranslation.net";
    const USE_NEW_CHAPTER_ENDPOINT: bool = true;
}

#[test]
fn parses_popular_and_pagination() {
    let document = html::document(include_str!("fixtures/list.html"));
    let page = parse_listing_html(&document, "https://lhtranslation.net/manga/", true).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].title, "Alpha");
    assert_eq!(
        page.entries[0]
            .cover
            .as_ref()
            .map(|request| request.url.as_str()),
        Some("https://lhtranslation.net/covers/alpha.jpg")
    );
    assert!(page.has_next_page);
}

#[test]
fn parses_search_details_chapters_and_pages() {
    let list = html::document(include_str!("fixtures/list.html"));
    assert_eq!(
        parse_search_html(&list, "https://lhtranslation.net/")
            .unwrap()
            .entries
            .len(),
        0
    );
    let details = parse_details_html(
        &html::document(include_str!("fixtures/details.html")),
        "https://lhtranslation.net/manga/alpha/",
    )
    .unwrap();
    assert_eq!(details.authors, ["Jane Doe"]);
    assert_eq!(details.status, Some(json!("ongoing")));
    let chapters = parse_chapters_html(
        &html::document(include_str!("fixtures/chapters.html")),
        "https://lhtranslation.net/manga/alpha/",
    )
    .unwrap();
    assert_eq!(chapters[0].chapter_number, Some(12.0));
    assert_eq!(
        chapters[0].url.as_deref(),
        Some("https://lhtranslation.net/manga/alpha/chapter-12/?style=list")
    );
    let pages = parse_pages_html(
        &html::document(include_str!("fixtures/pages.html")),
        "https://lhtranslation.net/manga/alpha/chapter-12/?style=list",
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    match &pages[1].content {
        PageContent::Url { url, context } => {
            assert_eq!(url, "https://lhtranslation.net/uploads/alpha/002.jpg");
            assert_eq!(
                context.as_ref().unwrap()["Referer"],
                "https://lhtranslation.net/manga/alpha/chapter-12/?style=list"
            );
        }
        _ => panic!("expected URL page"),
    }
}

#[test]
fn builds_urls_and_serializes_filters() {
    assert_eq!(
        listing_url::<TestConfig>(2, "views").unwrap(),
        "https://lhtranslation.net/manga/page/2/?m_orderby=views"
    );
    let url = search_url::<TestConfig>("a & b", 3, &json!({"author":"Jane Doe","status":["on-going"],"genres":["action","drama"],"order_by":"rating"})).unwrap();
    assert_eq!(url, "https://lhtranslation.net/page/3/?s=a+%26+b&post_type=wp-manga&author=Jane+Doe&m_orderby=rating&status%5B%5D=on-going&genre%5B%5D=action&genre%5B%5D=drama");
    assert_eq!(
        chapters_endpoint_url("https://lhtranslation.net/manga/alpha/").unwrap(),
        "https://lhtranslation.net/manga/alpha/ajax/chapters"
    );
}

#[test]
fn rejects_malformed_required_responses_and_skips_broken_page_images() {
    let malformed_details = html::document("<div class='post-title'></div>");
    assert!(
        parse_details_html(&malformed_details, "https://lhtranslation.net/manga/alpha/").is_err()
    );
    let malformed_chapter = html::document("<li class='wp-manga-chapter'><a>Chapter 1</a></li>");
    assert!(parse_chapters_html(&malformed_chapter, "https://lhtranslation.net/").is_err());
    let malformed_pages = html::document("<div class='page-break'><img></div>");
    assert!(
        parse_pages_html(&malformed_pages, "https://lhtranslation.net/chapter/")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ignores_empty_thumbnail_links_before_the_title_link() {
    let document = html::document(
        r#"<div class="c-tabs-item__content">
            <a class="item-thumb" href="/manga/alpha/"><img src="/alpha.jpg"></a>
            <div class="post-title"><a href="/manga/alpha/">Alpha</a></div>
        </div>"#,
    );
    let page = parse_search_html(&document, "https://example.test/").unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].title, "Alpha");
    assert_eq!(
        page.entries[0].url.as_deref(),
        Some("https://example.test/manga/alpha/")
    );
}

#[test]
fn parses_multilingual_relative_dates_against_the_host_clock() {
    let now = 2_000_000_000_000;
    assert_eq!(
        parse_relative_date_at("2 days ago", now),
        Some(now - 172_800_000)
    );
    assert_eq!(
        parse_relative_date_at("3 ชั่วโมง", now),
        Some(now - 10_800_000)
    );
    assert_eq!(
        parse_relative_date_at("5 天前", now),
        Some(now - 432_000_000)
    );
}
