use manatan_extension::export_video_source;
use manatan_shared::video::wco::{WcoConfig, WcoSource};

const SOURCE: WcoSource<WcoTv> = WcoSource::new();

struct WcoTv;

impl WcoConfig for WcoTv {
    const NAME: &'static str = "WcoTv";
    const BASE_URL: &'static str = "https://www.wco.tv";
}

export_video_source!(SOURCE);
