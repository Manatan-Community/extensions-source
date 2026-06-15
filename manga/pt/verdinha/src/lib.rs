use manatan_extension::export_manga_source;
use manatan_shared::greenscan::{GreenScanConfig, GreenScanSource};

const SOURCE: GreenScanSource<Verdinha> = GreenScanSource::new();

struct Verdinha;

impl GreenScanConfig for Verdinha {
    const NAME: &'static str = "Verdinha";
    const BASE_URL: &'static str = "https://verdinha.wtf";
    const API_URL: &'static str = "https://api.verdinha.wtf";
    const CDN_URL: &'static str = "https://cdn.verdinha.wtf";
    const CDN_API_URL: &'static str = "https://api.verdinha.wtf/cdn";
    const SCAN_ID: &'static str = "1";
}

export_manga_source!(SOURCE);
