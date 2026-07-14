// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from IReaderorg/IReader-extensions' NovelMultiverse source.

use madara_novel::{MadaraNovelConfig, MadaraNovelSource};
#[cfg(target_arch = "wasm32")]
use manatan_sdk::Extension;

#[derive(Default)]
pub struct NovelMultiverseConfig;

impl MadaraNovelConfig for NovelMultiverseConfig {
    const NAME: &'static str = "NovelMultiverse";
    const BASE_URL: &'static str = "https://www.novelmultiverse.com";
    const LANG: &'static str = "en";
}

pub type NovelMultiverseSource = MadaraNovelSource<NovelMultiverseConfig>;

#[cfg(target_arch = "wasm32")]
fn extension() -> Extension {
    Extension::new().novel("novelmultiverse", NovelMultiverseSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_upstream_source_metadata() {
        assert_eq!(NovelMultiverseConfig::NAME, "NovelMultiverse");
        assert_eq!(
            NovelMultiverseConfig::BASE_URL,
            "https://www.novelmultiverse.com"
        );
        assert_eq!(NovelMultiverseConfig::LANG, "en");
    }
}
