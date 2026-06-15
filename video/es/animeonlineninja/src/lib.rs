use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimeOnlineNinja> = DooPlaySource::new();

struct AnimeOnlineNinja;

impl DooPlayConfig for AnimeOnlineNinja {
    const NAME: &'static str = "AnimeOnline.Ninja";
    const BASE_URL: &'static str = "https://ver.animeonline.ninja";
    const LANG: &'static str = "es";
    const LATEST_PATH: &'static str = "episodio";
    const POPULAR_PATH: &'static str = "tendencias";
    const RESOLVE_EMBED_PAGE: bool = true;
    const USE_WP_JSON_PLAYER: bool = true;
}

export_video_source!(SOURCE);
