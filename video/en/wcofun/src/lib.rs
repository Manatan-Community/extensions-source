use manatan_extension::export_video_source;
use manatan_shared::video::wco::{WcoConfig, WcoSource};

const SOURCE: WcoSource<Wcofun> = WcoSource::new();

struct Wcofun;

impl WcoConfig for Wcofun {
    const NAME: &'static str = "Wcofun";
    const BASE_URL: &'static str = "https://www.wcoflix.tv";
}

export_video_source!(SOURCE);
