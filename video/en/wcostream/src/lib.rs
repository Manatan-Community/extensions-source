use manatan_extension::export_video_source;
use manatan_shared::video::wco::{WcoConfig, WcoSource};

const SOURCE: WcoSource<WCOStream> = WcoSource::new();

struct WCOStream;

impl WcoConfig for WCOStream {
    const NAME: &'static str = "WCOStream";
    const BASE_URL: &'static str = "https://www.wcostream.tv";
}

export_video_source!(SOURCE);
