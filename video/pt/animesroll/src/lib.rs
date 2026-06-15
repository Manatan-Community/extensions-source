use manatan_extension::export_video_source;

#[path = "../../_shared/pt_video_common.rs"]
mod pt_video_common;

use pt_video_common::{DooEndpoint, PtVideoConfig, PtVideoSource};

const SOURCE: PtVideoSource<AnimesRoll> = PtVideoSource::new();

struct AnimesRoll;

impl PtVideoConfig for AnimesRoll {
    const NAME: &'static str = "Animes ROLL";
    const BASE_URL: &'static str = "https://anroll.tv";
    const POPULAR_TITLE: &'static str = "Animes";
    const LATEST_TITLE: &'static str = "Episodios";
    const LIST_SELECTOR: &'static str = "div.items.featured article div.poster, article div.poster, article.item div.poster";
    const LATEST_SELECTOR: &'static str = "article div.poster, div.items article div.poster";
    const SEARCH_SELECTOR: &'static str = "div.result-item article div.thumbnail > a, article div.poster";
    const DETAILS_TITLE_SELECTOR: &'static str = "div.sheader div.data h1, h1";
    const DETAILS_COVER_SELECTOR: &'static str = "div.sheader div.poster img, img";
    const DETAILS_DESCRIPTION_SELECTOR: &'static str = "div.wp-content p, div#info p, p";
    const DETAILS_TAG_SELECTOR: &'static str = "div.sgeneros a, a[rel='tag']";
    const EPISODE_SELECTOR: &'static str = "ul.episodios li a, div.episodios a[href]";
    const PLAYER_SELECTOR: &'static str = "source[src], iframe[src], script";
    const USE_DOO_AJAX: bool = true;
    const DOO_ENDPOINT: DooEndpoint = DooEndpoint::AdminAjax;

    fn popular_url(_page: u64) -> String {
        format!("{}/animes/", Self::BASE_URL)
    }

    fn latest_url(page: u64) -> String {
        format!("{}/episodios/page/{page}", Self::BASE_URL)
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
