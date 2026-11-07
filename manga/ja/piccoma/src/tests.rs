use super::*;

const CATALOG: &str = include_str!("../tests/fixtures/catalog.html");
const DETAILS: &str = include_str!("../tests/fixtures/details.html");
const EPISODES: &str = include_str!("../tests/fixtures/episodes.html");
const VOLUMES: &str = include_str!("../tests/fixtures/volumes.html");
const READER: &str = include_str!("../tests/fixtures/reader.html");

#[test]
fn ranking_keeps_unique_stable_ids_and_real_covers() {
    let page = parse_listing(CATALOG, true).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert!(!page.has_next_page);
    assert_eq!(page.entries[0].key, "101");
    assert_eq!(page.entries[0].title, "テスト作品");
    assert_eq!(
        page.entries[0].cover.as_ref().unwrap().url,
        "https://piccoma.kakaocdn.net/dn/fixture/cover/asset/cover_x3"
    );
    assert_eq!(
        page.entries[0].cover.as_ref().unwrap().headers["Referer"],
        format!("{BASE}/web/")
    );
}
#[test]
fn latest_respects_server_next_page_marker() {
    let html = r#"<ul class="PCOM-productList"><li><a href="/web/product/101"><img src="//piccoma.kakaocdn.net/dn/a/b/c/cover_x1"><div class="PCOM-prdList_info"><div class="PCOM-prdList_title"><span>作品</span></div></div></a></li></ul><a id="js_nextPage"></a>"#;
    assert!(parse_listing(html, false).unwrap().has_next_page);
    assert!(
        !parse_listing(&html.replace("id=\"js_nextPage\"", ""), false)
            .unwrap()
            .has_next_page
    );
}
#[test]
fn search_excludes_non_manga_handles_duplicates_and_paginates() {
    let response = json!({"status":0,"data":{"total_page":2,"products":[
        {"id":101,"title":"漫画","img":null,"is_audio":null,"is_anime":null,"is_novel":null}, {"id":101,"title":"重複","img":null},
        {"id":102,"title":"アニメ","img":null,"is_anime":1},
        {"id":103,"title":"小説","img":null,"is_novel":1},
        {"id":104,"title":"音声","img":null,"is_audio":1}
    ]}})
    .to_string();
    let page = parse_search(&response, 1).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert!(page.has_next_page);
    assert!(!parse_search(&response, 2).unwrap().has_next_page);
    assert!(parse_search("{}", 1).is_err());
    assert_eq!(
        parse_search(r#"{"status":0,"data":{"total_page":0,"products":[]}}"#, 99)
            .unwrap()
            .entries
            .len(),
        0
    );
}
#[test]
fn details_preserve_metadata_and_reading_identity() {
    let item = parse_details(DETAILS, "101").unwrap();
    assert!(item.initialized);
    assert_eq!(item.key, "101");
    assert_eq!(item.authors, ["作者", "作画"]);
    assert!(item.description.unwrap().contains("続きです"));
    assert_eq!(item.status, Some(json!("ongoing")));
    assert_eq!(item.tags.len(), 2);
    assert!(parse_details("<h1>missing selectors</h1>", "101").is_err());
}
#[test]
fn chapter_entitlements_are_never_fabricated() {
    let chapters = parse_chapters(EPISODES, "101", "E", false).unwrap();
    assert_eq!(chapters.len(), 4);
    assert!(!chapters[0].is_locked);
    assert!(chapters[1..].iter().all(|c| c.is_locked));
    assert_eq!(chapters[0].chapter_number, Some(1.));
    assert_eq!(parse_chapters(EPISODES, "101", "E", true).unwrap().len(), 1);
    assert!(parse_chapters(EPISODES, "999", "E", false).is_err());
}
#[test]
fn volume_previews_are_available_but_purchase_only_volumes_stay_locked() {
    let volumes = parse_chapters(VOLUMES, "101", "V", false).unwrap();
    assert_eq!(volumes.len(), 3);
    assert!(volumes[2].is_locked);
    assert!(!volumes[1].is_locked);
    assert_eq!(volumes[1].extra["preview"], true);
    assert_eq!(volumes[1].volume_number, Some(2.));
    assert_eq!(parse_chapters(VOLUMES, "101", "V", true).unwrap().len(), 2);
    let mut all = volumes;
    all.extend(parse_chapters(EPISODES, "101", "E", false).unwrap());
    all.reverse();
    assert_eq!(all[0].chapter_number, Some(4.));
    assert_eq!(all.last().unwrap().volume_number, Some(1.));
}
#[test]
fn reader_accepts_javascript_object_syntax_and_keeps_page_order() {
    let pages = parse_pages(READER, "https://piccoma.com/web/viewer/101/201").unwrap();
    assert_eq!(pages.len(), 2);
    assert!(pages[0].extra["piccomaSeed"].is_string());
    let PageContent::Url { url, .. } = &pages[0].content else {
        panic!("expected page URL")
    };
    assert!(url.contains("001.jpg"));
    assert!(!url.contains("#scrambled"));
    assert_eq!(
        pages[0].headers["Referer"],
        "https://piccoma.com/web/viewer/101/201"
    );
}
#[test]
fn unscrambled_pages_and_img_variant_need_no_transform() {
    let html = r#"<script>var _pdata_ = {title:'作品',isScrambled:false,img:[{path:'//piccoma.kakaocdn.net/free/page.jpg'}]};</script>"#;
    let pages = parse_pages(html, BASE).unwrap();
    assert!(pages[0].extra.is_empty());
}
#[test]
fn locked_missing_and_truncated_reader_data_are_useful_errors() {
    assert!(parse_pages("<p>ログインしてください</p>", BASE)
        .unwrap_err()
        .message
        .contains("purchase"));
    assert!(reader_object("var _pdata_ = {title: 'unfinished'").is_err());
    assert!(parse_pages(
        "<script>var _pdata_={isScrambled:false,img:[]};</script>",
        BASE
    )
    .is_err());
}
#[test]
fn region_restriction_is_not_a_fake_empty_catalog_or_reader() {
    let html = r#"<body class="PCM-error_overseasAccess">Unfortunately, this service can only be used from Japan.</body>"#;
    assert!(parse_listing(html, true)
        .unwrap_err()
        .message
        .contains("Japan"));
    assert!(parse_details(html, "101").is_err());
    assert!(parse_pages(html, BASE).is_err());
}
#[test]
fn image_and_item_urls_cannot_escape_declared_origins() {
    for url in [
        "http://piccoma.com/web/product/101",
        "https://piccoma.com.attacker.test/web/product/101",
        "https://user@piccoma.com/web/product/101",
        "https://piccoma.com:9443/web/product/101",
    ] {
        assert!(resolve_url(url).unwrap().is_none());
    }
    for url in [
        "http://piccoma.kakaocdn.net/x",
        "https://127.0.0.1/x",
        "https://other.kakaocdn.net/x",
        "https://piccoma.kakaocdn.net.attacker.test/x",
    ] {
        assert!(image_url(url).is_err());
    }
    assert!(ranking_filter(&json!({"ranking":"../../private"})).is_err());
    let result = resolve_url("https://piccoma.com/web/viewer/101/302")
        .unwrap()
        .unwrap();
    assert_eq!(result.item.unwrap().key, "101");
    assert_eq!(chapter_id(&result.manga_chapter.unwrap()).unwrap(), "302");
}
#[test]
fn ranking_and_out_of_range_pages_do_not_repeat_the_first_page() {
    let mut source = PiccomaSource::default();
    assert!(source.popular(2).unwrap().entries.is_empty());
}

#[test]
fn login_state_comes_from_the_site_not_the_presence_of_session_cookies() {
    assert!(
        !parse_authentication_status("<script>var _init_ = {'login': false, 'os':'PC'};</script>")
            .unwrap()
            .authenticated
    );
    assert!(
        parse_authentication_status("<script>var _init_ = {'login': true, 'os':'PC'};</script>")
            .unwrap()
            .authenticated
    );
    assert!(
        parse_authentication_status("<script>var unrelated = {'login': true};</script>").is_err()
    );
}
