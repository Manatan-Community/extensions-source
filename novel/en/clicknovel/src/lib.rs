// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from IReaderorg/IReader-extensions' ClickNovel source.

use madara_novel::{MadaraNovelConfig, MadaraNovelSource};
#[cfg(target_arch = "wasm32")]
use manatan_sdk::Extension;

#[derive(Default)]
pub struct ClickNovelConfig;

impl MadaraNovelConfig for ClickNovelConfig {
    const NAME: &'static str = "ClickNovel";
    const BASE_URL: &'static str = "https://clicknovel.net";
    const LANG: &'static str = "en";
}

pub type ClickNovelSource = MadaraNovelSource<ClickNovelConfig>;

#[cfg(target_arch = "wasm32")]
fn extension() -> Extension {
    Extension::new().novel("clicknovel", ClickNovelSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_upstream_source_metadata() {
        assert_eq!(ClickNovelConfig::NAME, "ClickNovel");
        assert_eq!(ClickNovelConfig::BASE_URL, "https://clicknovel.net");
        assert_eq!(ClickNovelConfig::LANG, "en");
    }
}
