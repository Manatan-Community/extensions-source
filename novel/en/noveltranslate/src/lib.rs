// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from IReaderorg/IReader-extensions' NovelTranslate source.

use madara_novel::{MadaraNovelConfig, MadaraNovelSource};
#[cfg(target_arch = "wasm32")]
use manatan_sdk::Extension;

#[derive(Default)]
pub struct NovelTranslateConfig;

impl MadaraNovelConfig for NovelTranslateConfig {
    const NAME: &'static str = "NovelTranslate";
    const BASE_URL: &'static str = "https://noveltranslate.com";
    const LANG: &'static str = "en";
}

pub type NovelTranslateSource = MadaraNovelSource<NovelTranslateConfig>;

#[cfg(target_arch = "wasm32")]
fn extension() -> Extension {
    Extension::new().novel("noveltranslate", NovelTranslateSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_upstream_source_metadata() {
        assert_eq!(NovelTranslateConfig::NAME, "NovelTranslate");
        assert_eq!(NovelTranslateConfig::BASE_URL, "https://noveltranslate.com");
        assert_eq!(NovelTranslateConfig::LANG, "en");
    }
}
