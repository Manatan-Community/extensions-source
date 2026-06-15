use manatan_extension::export_video_source;

#[path = "../../_shared/italian_video.rs"]
mod italian_video;

use italian_video::{SaturnConfig, SaturnSource};

const SOURCE: SaturnSource<HentaiSaturn> = SaturnSource::new();

struct HentaiSaturn;

impl SaturnConfig for HentaiSaturn {
    const NAME: &'static str = "HentaiSaturn";
    const BASE_URL: &'static str = "https://www.hentaisaturn.com";
    const CONTENT_RATING: &'static str = "adult";
    const LIST_PATH: &'static str = "/toplist";
    const LATEST_PATH: &'static str = "/newest";
    const ARCHIVE_PATH: &'static str = "/hentailist";
    const CARD_IMG_CLASS: &'static str = "new-hentai";
    const TITLE_CLASS: &'static str = "div.container.hentai-title-as.mb-3.w-100 b";
}

export_video_source!(SOURCE);
