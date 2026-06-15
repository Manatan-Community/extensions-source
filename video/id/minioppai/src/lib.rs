use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<MiniOppai> = IndonesianSource::new();

struct MiniOppai;

impl IndonesianConfig for MiniOppai {
    const NAME: &'static str = "MiniOppai";
    const BASE_URL: &'static str = "https://minioppai.org";
    const CONTENT_RATING: &'static str = "adult";
    const QUALITY_DEFAULT: &'static str = "720p";

    fn list_url(listing: &str, page: u64) -> String {
        let order = if listing == "latest" {
            "update"
        } else {
            "popular"
        };
        format!(
            "{}/advanced-search/page/{page}/?order={order}",
            Self::BASE_URL
        )
    }

    fn search_url(query: &str, page: u64, genre: Option<String>) -> String {
        if !query.is_empty() {
            return format!(
                "{}/page/{page}/?s={}",
                Self::BASE_URL,
                url::query_escape(query)
            );
        }
        let genre = genre.filter(|value| !value.is_empty()).unwrap_or_default();
        let suffix = if genre.is_empty() {
            String::new()
        } else {
            format!("&genre[]={}", url::query_escape(&genre))
        };
        format!(
            "{}/advanced-search/page/{page}/?order=popular{suffix}",
            Self::BASE_URL
        )
    }
}

export_video_source!(SOURCE);
