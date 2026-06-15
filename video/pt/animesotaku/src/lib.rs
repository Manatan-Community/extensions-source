use manatan_extension::export_video_source;

#[path = "../../_shared/pt_video_portal.rs"]
mod pt_video_portal;

use pt_video_portal::{PortalConfig, PortalKind, PortalSource};

const SOURCE: PortalSource<AnimeCore> = PortalSource::new();

struct AnimeCore;

impl PortalConfig for AnimeCore {
    const NAME: &'static str = "AnimeCore";
    const BASE_URL: &'static str = "https://animecore.net";
    const KIND: PortalKind = PortalKind::AnimeCore;
}

export_video_source!(SOURCE);
