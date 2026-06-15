use manatan_extension::export_manga_source;
use manatan_shared::greenscan::{GreenScanConfig, GreenScanSource};

const SOURCE: GreenScanSource<MaidScan> = GreenScanSource::new();

struct MaidScan;

impl GreenScanConfig for MaidScan {
    const NAME: &'static str = "Maid Scan";
    const BASE_URL: &'static str = "https://empreguetes.wtf";
    const API_URL: &'static str = "https://api.verdinha.wtf";
    const CDN_URL: &'static str = "https://cdn.verdinha.wtf";
    const CDN_API_URL: &'static str = "https://api.verdinha.wtf/cdn";
    const SCAN_ID: &'static str = "3";
    const DEFAULT_GENRE_ID: &'static str = "4";
}

export_manga_source!(SOURCE);
