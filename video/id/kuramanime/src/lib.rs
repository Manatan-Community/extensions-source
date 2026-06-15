use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<Kuramanime> = IndonesianSource::new();

struct Kuramanime;

impl IndonesianConfig for Kuramanime {
    const NAME: &'static str = "Kuramanime";
    const BASE_URL: &'static str = "https://v8.kuramanime.tel";
    const QUALITY_DEFAULT: &'static str = "1080p";

    fn list_url(listing: &str, page: u64) -> String {
        if listing == "latest" {
            format!("{}/anime?order_by=updated&page={page}", Self::BASE_URL)
        } else {
            format!("{}/anime?page={page}", Self::BASE_URL)
        }
    }

    fn search_url(query: &str, page: u64, _genre: Option<String>) -> String {
        format!(
            "{}/anime?search={}&page={page}",
            Self::BASE_URL,
            url::query_escape(query)
        )
    }
}

export_video_source!(SOURCE);
