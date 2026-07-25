use fucknovelpia_novel::Config;
#[cfg(target_arch = "wasm32")]
use fucknovelpia_novel::Source;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "fucknovelpia-raw";

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct KoreanRaw;

impl Config for KoreanRaw {
    const BASE_URL: &'static str = "https://raw-fucknovelpia.com";
    const LANGUAGE: &'static str = "ko";
    const RAW_DOWNLOADS: bool = true;
}

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, Source::<KoreanRaw>::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());
