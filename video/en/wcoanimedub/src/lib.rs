use manatan_extension::export_video_source;
use manatan_shared::video::wco::{WcoConfig, WcoSource};

const SOURCE: WcoSource<WcoAnimeDub> = WcoSource::new();

struct WcoAnimeDub;

impl WcoConfig for WcoAnimeDub {
    const NAME: &'static str = "WcoAnimeDub";
    const BASE_URL: &'static str = "https://www.wcoanimedub.tv";
}

export_video_source!(SOURCE);
