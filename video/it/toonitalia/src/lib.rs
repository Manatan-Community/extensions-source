use manatan_extension::export_video_source;

#[path = "../../_shared/italian_video.rs"]
mod italian_video;

const SOURCE: italian_video::ToonItaliaSource = italian_video::ToonItaliaSource;

export_video_source!(SOURCE);
