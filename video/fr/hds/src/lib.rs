use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<Hds> = DooPlaySource::new();

struct Hds;

impl DooPlayConfig for Hds {
    const NAME: &'static str = "HDS";
    const BASE_URL: &'static str = "https://on1.hds.quest";
    const LANG: &'static str = "fr";
    const POPULAR_PATH: &'static str = "tendance";
    const LATEST_PATH: &'static str = "films";
    const RESOLVE_EMBED_PAGE: bool = true;
    const USE_WP_JSON_PLAYER: bool = true;
}

export_video_source!(SOURCE);
