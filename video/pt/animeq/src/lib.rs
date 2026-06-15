use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimeQ> = DooPlaySource::new();

struct AnimeQ;

impl DooPlayConfig for AnimeQ {
    const NAME: &'static str = "AnimeQ";
    const BASE_URL: &'static str = "https://animeq.net";
    const LANG: &'static str = "pt-BR";
    const POPULAR_PATH: &'static str = "anime";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
    const USE_WP_JSON_PLAYER: bool = true;
}

export_video_source!(SOURCE);
