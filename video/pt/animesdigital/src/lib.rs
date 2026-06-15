use manatan_extension::export_video_source;

#[path = "../../_shared/pt_video_portal.rs"]
mod pt_video_portal;

use pt_video_portal::{PortalConfig, PortalKind, PortalSource};

const SOURCE: PortalSource<AnimesDigital> = PortalSource::new();

struct AnimesDigital;

impl PortalConfig for AnimesDigital {
    const NAME: &'static str = "Animes Digital";
    const BASE_URL: &'static str = "https://animesdigital.org";
    const KIND: PortalKind = PortalKind::AnimesDigital;
}

export_video_source!(SOURCE);
