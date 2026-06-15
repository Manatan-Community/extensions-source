use manatan_extension::export_video_source;

#[path = "../../_shared/italian_video.rs"]
mod italian_video;

const SOURCE: italian_video::VvvvidSource = italian_video::VvvvidSource;

export_video_source!(SOURCE);
