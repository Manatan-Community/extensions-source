use manatan_extension::export_video_source;
use manatan_shared::video::wco::{WcoConfig, WcoSource};

const SOURCE: WcoSource<WcoForever> = WcoSource::new();

struct WcoForever;

impl WcoConfig for WcoForever {
    const NAME: &'static str = "WcoForever";
    const BASE_URL: &'static str = "https://www.wcoforever.net";
}

export_video_source!(SOURCE);
