mod metadata_manager;
mod recording_helper;

pub use jellyfin_xbmc_metadata::{MovieNfo, NfoMetadata};
pub use metadata_manager::{
    RecordingMetadataClock, RecordingMetadataDocument, RecordingMetadataError,
    RecordingMetadataOptions, RecordingsMetadataManager, SavedRecordingMetadata, SystemUtcClock,
};
pub use recording_helper::get_recording_name;
