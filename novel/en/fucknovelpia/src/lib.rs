use fucknovelpia_novel::Config;
#[cfg(target_arch = "wasm32")]
use fucknovelpia_novel::Source;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "fucknovelpia";

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct English;

impl Config for English {
    const BASE_URL: &'static str = "https://fucknovelpia.com";
    const LANGUAGE: &'static str = "en";
    const RAW_DOWNLOADS: bool = false;
}

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, Source::<English>::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());
