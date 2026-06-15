use manatan_extension::export_video_source;

#[path = "../../_shared/italian_video.rs"]
mod italian_video;

const SOURCE: italian_video::AniPlaySource = italian_video::AniPlaySource;

export_video_source!(SOURCE);
