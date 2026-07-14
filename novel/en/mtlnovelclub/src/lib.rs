// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from IReaderorg/IReader-extensions' MTLNovelClub source.

use madara_novel::{MadaraNovelConfig, MadaraNovelSource};
#[cfg(target_arch = "wasm32")]
use manatan_sdk::Extension;

#[derive(Default)]
pub struct MTLNovelClubConfig;

impl MadaraNovelConfig for MTLNovelClubConfig {
    const NAME: &'static str = "MTLNovelClub";
    const BASE_URL: &'static str = "https://mtlnovel.club";
    const LANG: &'static str = "en";
}

pub type MTLNovelClubSource = MadaraNovelSource<MTLNovelClubConfig>;

#[cfg(target_arch = "wasm32")]
fn extension() -> Extension {
    Extension::new().novel("mtlnovelclub", MTLNovelClubSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_upstream_source_metadata() {
        assert_eq!(MTLNovelClubConfig::NAME, "MTLNovelClub");
        assert_eq!(MTLNovelClubConfig::BASE_URL, "https://mtlnovel.club");
        assert_eq!(MTLNovelClubConfig::LANG, "en");
    }
}
