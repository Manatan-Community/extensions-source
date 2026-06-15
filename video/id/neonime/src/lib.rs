use manatan_extension::export_video_source;
use manatan_shared::video::indonesian::{IndonesianConfig, IndonesianSource};

const SOURCE: IndonesianSource<NeoNime> = IndonesianSource::new();

struct NeoNime;

impl IndonesianConfig for NeoNime {
    const NAME: &'static str = "NeoNime";
    const BASE_URL: &'static str = "https://neonime.ink";
    const QUALITY_DEFAULT: &'static str = "1080p";

    fn list_url(listing: &str, page: u64) -> String {
        if listing == "latest" {
            format!("{}/episode/page/{page}", Self::BASE_URL)
        } else {
            format!("{}/tvshows/page/{page}", Self::BASE_URL)
        }
    }

    fn search_url(_query: &str, _page: u64, _genre: Option<String>) -> String {
        format!("{}/list-anime/", Self::BASE_URL)
    }
}

export_video_source!(SOURCE);
