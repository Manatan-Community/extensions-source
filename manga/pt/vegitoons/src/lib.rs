use manatan_extension::export_manga_source;
use manatan_shared::greenscan::{GreenScanConfig, GreenScanSource};

const SOURCE: GreenScanSource<Vegitoons> = GreenScanSource::new();

struct Vegitoons;

impl GreenScanConfig for Vegitoons {
    const NAME: &'static str = "Vegitoons";
    const BASE_URL: &'static str = "https://vegitoons.black";
    const API_URL: &'static str = "https://api.vegitoons.black";
    const CDN_URL: &'static str = "https://cdn.verdinha.wtf";
    const CDN_API_URL: &'static str = "https://api.vegitoons.black/cdn";
    const SCAN_ID: &'static str = "1";
}

export_manga_source!(SOURCE);
