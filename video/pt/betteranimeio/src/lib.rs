use manatan_extension::export_video_source;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{DooEndpoint, PtVideoConfig, PtVideoSource};

const SOURCE: PtVideoSource<BetterAnimeIo> = PtVideoSource::new();

struct BetterAnimeIo;

impl PtVideoConfig for BetterAnimeIo {
    const NAME: &'static str = "BetterAnimeIo";
    const BASE_URL: &'static str = "https://betteranime.io";
    const POPULAR_TITLE: &'static str = "Destaques";
    const LATEST_TITLE: &'static str = "Lancamentos";
    const LIST_SELECTOR: &'static str = "div#featured-titles article.item div.poster, article.item div.poster";
    const LATEST_SELECTOR: &'static str = "div#archive-content article.item div.poster, article.item div.poster";
    const SEARCH_SELECTOR: &'static str = "article.item div.poster";
    const DETAILS_TITLE_SELECTOR: &'static str = "div.sheader div.data h1, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.sheader div.poster img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "div.wp-content p, div#info p, p";
    const DETAILS_TAG_SELECTOR: &'static str = "div.sgeneros a, nav.genres a, a[rel='tag']";
    const EPISODE_SELECTOR: &'static str = "ul.episodios li, div.episodios li, li:has(.episodiotitle a)";
    const EPISODE_TITLE_SELECTOR: &'static str = ".episodiotitle a, a";
    const PLAYER_SELECTOR: &'static str = "source[src], iframe[src], script";
    const USE_DOO_AJAX: bool = true;
    const DOO_ENDPOINT: DooEndpoint = DooEndpoint::WpJsonV2;

    fn popular_url(_page: u64) -> String {
        format!("{}/animes", Self::BASE_URL)
    }

    fn latest_url(page: u64) -> String {
        format!("{}/animes/page/{page}", Self::BASE_URL)
    }

    fn search_url(page: u64, query: &str, _request: &serde_json::Value) -> String {
        if query.is_empty() {
            format!("{}/animes/page/{page}", Self::BASE_URL)
        } else {
            format!("{}/page/{page}/?s={}", Self::BASE_URL, manatan_shared::url::query_escape(query))
        }
    }
}

export_video_source!(SOURCE);
