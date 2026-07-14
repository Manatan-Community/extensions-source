// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from IReaderorg/IReader-extensions' FirstKissNovel source.

use madara_novel::{MadaraNovelConfig, MadaraNovelSource};
#[cfg(target_arch = "wasm32")]
use manatan_sdk::Extension;

#[derive(Default)]
pub struct FirstKissNovelConfig;

impl MadaraNovelConfig for FirstKissNovelConfig {
    const NAME: &'static str = "FirstKissNovel";
    const BASE_URL: &'static str = "https://1stkissnovel.love";
    const LANG: &'static str = "en";
}

pub type FirstKissNovelSource = MadaraNovelSource<FirstKissNovelConfig>;

#[cfg(target_arch = "wasm32")]
fn extension() -> Extension {
    Extension::new().novel("firstkissnovel", FirstKissNovelSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_upstream_source_metadata() {
        assert_eq!(FirstKissNovelConfig::NAME, "FirstKissNovel");
        assert_eq!(FirstKissNovelConfig::BASE_URL, "https://1stkissnovel.love");
        assert_eq!(FirstKissNovelConfig::LANG, "en");
    }
}
