use manatan_extension::export_video_source;

#[path = "../../_shared/italian_video.rs"]
mod italian_video;

use italian_video::{SaturnConfig, SaturnSource};

const SOURCE: SaturnSource<AnimeSaturn> = SaturnSource::new();

struct AnimeSaturn;

impl SaturnConfig for AnimeSaturn {
    const NAME: &'static str = "AnimeSaturn";
    const BASE_URL: &'static str = "https://www.anisaturn.net";
    const CONTENT_RATING: &'static str = "safe";
    const LIST_PATH: &'static str = "/ongoing";
    const LATEST_PATH: &'static str = "/newest";
    const ARCHIVE_PATH: &'static str = "/animelist";
    const CARD_IMG_CLASS: &'static str = "new-anime";
    const TITLE_CLASS: &'static str = "div.container.anime-title-as.mb-3.w-100 b";
}

export_video_source!(SOURCE);
