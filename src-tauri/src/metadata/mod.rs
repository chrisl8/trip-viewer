pub mod mp4_probe;

use crate::error::AppError;
use crate::model::ChannelMeta;
use std::path::Path;

#[tauri::command]
pub async fn probe_file(path: String) -> Result<ChannelMeta, AppError> {
    mp4_probe::probe(Path::new(&path))
}
