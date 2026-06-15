use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<Animenix> = DooPlaySource::new();

struct Animenix;

impl DooPlayConfig for Animenix {
    const NAME: &'static str = "Animenix";
    const BASE_URL: &'static str = "https://animenix.com";
    const LANG: &'static str = "es";
    const LATEST_PATH: &'static str = "ver";
    const POPULAR_PATH: &'static str = "ratings";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
