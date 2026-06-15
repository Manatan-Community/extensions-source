use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<Cineplus123> = DooPlaySource::new();

struct Cineplus123;

impl DooPlayConfig for Cineplus123 {
    const NAME: &'static str = "Cineplus123";
    const BASE_URL: &'static str = "https://cineplus123.org";
    const LANG: &'static str = "es";
    const LATEST_PATH: &'static str = "ano/2024";
    const POPULAR_PATH: &'static str = "tendencias";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
