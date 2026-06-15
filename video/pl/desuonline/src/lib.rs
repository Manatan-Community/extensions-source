use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<DesuOnline> = AnimeStreamSource::new();

struct DesuOnline;

impl AnimeStreamConfig for DesuOnline {
    const NAME: &'static str = "desu-online";
    const BASE_URL: &'static str = "https://desu-online.pl";
    const LANG: &'static str = "pl";
    const QUALITY_DEFAULT: &'static str = "720p";
}

export_video_source!(SOURCE);
