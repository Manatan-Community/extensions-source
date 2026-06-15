use manatan_extension::export_video_source;

#[path = "../../_shared/pt_video_portal.rs"]
mod pt_video_portal;

use pt_video_portal::{PortalConfig, PortalKind, PortalSource};

const SOURCE: PortalSource<AnimesOnlineVip> = PortalSource::new();

struct AnimesOnlineVip;

impl PortalConfig for AnimesOnlineVip {
    const NAME: &'static str = "Animes Online Vip";
    const BASE_URL: &'static str = "https://animesonlinefhd.vip";
    const CONTENT_RATING: &'static str = "adult";
    const KIND: PortalKind = PortalKind::AnimesOnlineVip;
}

export_video_source!(SOURCE);
