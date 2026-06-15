use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<Kuronime> = IndonesianSource::new();

struct Kuronime;

impl IndonesianConfig for Kuronime {
    const NAME: &'static str = "Kuronime";
    const BASE_URL: &'static str = "https://tv1.kuronime.vip";
    const QUALITY_DEFAULT: &'static str = "1080p";

    fn list_url(listing: &str, page: u64) -> String {
        if listing == "latest" {
            format!(
                "{}/anime/?page={page}&status=ongoing&sub=&order=update",
                Self::BASE_URL
            )
        } else {
            format!("{}/anime/page/{page}", Self::BASE_URL)
        }
    }

    fn search_url(query: &str, page: u64, _genre: Option<String>) -> String {
        if query.is_empty() {
            format!("{}/anime/page/{page}", Self::BASE_URL)
        } else {
            format!(
                "{}/page/{page}/?s={}",
                Self::BASE_URL,
                url::query_escape(query)
            )
        }
    }
}

export_video_source!(SOURCE);
