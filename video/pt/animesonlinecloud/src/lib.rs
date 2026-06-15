use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimesOnlineCloud> = DooPlaySource::new();

struct AnimesOnlineCloud;

impl DooPlayConfig for AnimesOnlineCloud {
    const NAME: &'static str = "AnimesOnlineCloud";
    const BASE_URL: &'static str = "https://animesonline.cloud";
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "adult";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
