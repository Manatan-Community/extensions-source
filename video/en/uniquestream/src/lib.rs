use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<UniqueStream> = DooPlaySource::new();

struct UniqueStream;

impl DooPlayConfig for UniqueStream {
    const NAME: &'static str = "UniqueStream";
    const BASE_URL: &'static str = "https://uniquestream.net";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
