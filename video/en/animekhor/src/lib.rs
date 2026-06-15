use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<AnimeKhor> = AnimeStreamSource::new();

struct AnimeKhor;

impl AnimeStreamConfig for AnimeKhor {
    const NAME: &'static str = "AnimeKhor";
    const BASE_URL: &'static str = "https://animekhor.org";
}

export_video_source!(SOURCE);
