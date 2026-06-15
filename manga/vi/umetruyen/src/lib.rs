use manatan_extension::export_manga_source;
use manatan_shared::manhwaz::{ManhwazConfig, ManhwazSource};

const SOURCE: ManhwazSource<UmeTruyenConfig> = ManhwazSource::new();

struct UmeTruyenConfig;

impl ManhwazConfig for UmeTruyenConfig {
    const NAME: &'static str = "UmeTruyen";
    const BASE_URL: &'static str = "https://umetruyenz.org";
    const LANG: &'static str = "vi";
    const CONTENT_RATING: &'static str = "safe";
    const AUTHOR_HEADING: &'static str = "Tác giả";
    const STATUS_HEADING: &'static str = "Trạng thái";
}

export_manga_source!(SOURCE);
