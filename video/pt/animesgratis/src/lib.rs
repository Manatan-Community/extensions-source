use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<Q1N> = DooPlaySource::new();

struct Q1N;

impl DooPlayConfig for Q1N {
    const NAME: &'static str = "Q1N";
    const BASE_URL: &'static str = "https://q1n.net";
    const LANG: &'static str = "pt-BR";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
