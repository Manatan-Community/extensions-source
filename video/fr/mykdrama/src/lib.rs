use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<MyKdrama> = AnimeStreamSource::new();

struct MyKdrama;

impl AnimeStreamConfig for MyKdrama {
    const NAME: &'static str = "MyKdrama";
    const BASE_URL: &'static str = "https://mykdrama.co";
    const LANG: &'static str = "fr";
    const LIST_PATH: &'static str = "drama";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
