use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<NimeGami> = IndonesianSource::new();

struct NimeGami;

impl IndonesianConfig for NimeGami {
    const NAME: &'static str = "NimeGami";
    const BASE_URL: &'static str = "https://nimegami.id";
    const QUALITY_DEFAULT: &'static str = "720p";

    fn list_url(listing: &str, page: u64) -> String {
        if listing == "latest" && page > 1 {
            format!("{}/page/{page}", Self::BASE_URL)
        } else {
            Self::BASE_URL.to_string()
        }
    }

    fn search_url(query: &str, page: u64, _genre: Option<String>) -> String {
        if query.is_empty() {
            Self::BASE_URL.to_string()
        } else {
            format!(
                "{}/page/{page}/?s={}&post_type=post",
                Self::BASE_URL,
                url::query_escape(query)
            )
        }
    }
}

export_video_source!(SOURCE);
