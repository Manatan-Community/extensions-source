use manatan_extension::export_video_source;
use manatan_shared::video::wco::{WcoConfig, WcoSource};

const SOURCE: WcoSource<WcoAnimeSub> = WcoSource::new();

struct WcoAnimeSub;

impl WcoConfig for WcoAnimeSub {
    const NAME: &'static str = "WcoAnimeSub";
    const BASE_URL: &'static str = "https://www.wcoanimesub.tv";
}

export_video_source!(SOURCE);
