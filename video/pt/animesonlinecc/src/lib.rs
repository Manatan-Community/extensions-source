use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimesOnlineCc> = DooPlaySource::new();

struct AnimesOnlineCc;

impl DooPlayConfig for AnimesOnlineCc {
    const NAME: &'static str = "Animes Online CC";
    const BASE_URL: &'static str = "https://animesonlinecc.to";
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "adult";
    const POPULAR_PATH: &'static str = "";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
