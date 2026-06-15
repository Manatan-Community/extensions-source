use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<OtakuDesu> = IndonesianSource::new();

struct OtakuDesu;

impl IndonesianConfig for OtakuDesu {
    const NAME: &'static str = "OtakuDesu";
    const BASE_URL: &'static str = "https://otakudesu.blog";
    const QUALITY_DEFAULT: &'static str = "1080p";

    fn list_url(listing: &str, page: u64) -> String {
        if listing == "latest" {
            format!("{}/ongoing-anime/page/{page}", Self::BASE_URL)
        } else {
            format!("{}/complete-anime/page/{page}", Self::BASE_URL)
        }
    }

    fn search_url(query: &str, page: u64, genre: Option<String>) -> String {
        if !query.is_empty() {
            return format!(
                "{}/?s={}&post_type=anime",
                Self::BASE_URL,
                url::query_escape(query)
            );
        }
        if let Some(genre) = genre.filter(|value| !value.is_empty()) {
            return format!(
                "{}/genres/{}/page/{page}",
                Self::BASE_URL,
                url::query_escape(&genre)
            );
        }
        format!("{}/complete-anime/page/{page}", Self::BASE_URL)
    }
}

export_video_source!(SOURCE);
