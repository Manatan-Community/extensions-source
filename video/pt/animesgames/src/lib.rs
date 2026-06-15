use manatan_extension::export_video_source;

#[path = "../../_shared/pt_video_portal.rs"]
mod pt_video_portal;

use pt_video_portal::{PortalConfig, PortalKind, PortalSource};

const SOURCE: PortalSource<AnimesGames> = PortalSource::new();

struct AnimesGames;

impl PortalConfig for AnimesGames {
    const NAME: &'static str = "Animes Games";
    const BASE_URL: &'static str = "https://animesgames.cc";
    const KIND: PortalKind = PortalKind::AnimesGames;
}

export_video_source!(SOURCE);
