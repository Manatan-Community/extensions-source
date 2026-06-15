use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimesBr> = DooPlaySource::new();

struct AnimesBr;

impl DooPlayConfig for AnimesBr {
    const NAME: &'static str = "Animes BR";
    const BASE_URL: &'static str = "https://animesbr.tv";
    const LANG: &'static str = "pt-BR";
    const POPULAR_PATH: &'static str = "";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
