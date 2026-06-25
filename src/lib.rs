pub mod ui;
use flate2::read::GzDecoder;
use std::fs::{self, File};
use tar::Archive;
use crate::ui::ChiralUI;

pub fn install_binary(ui: &mut ChiralUI, package: &str) -> Result<(), String> {
    ui.draw_header("2.0");
    ui.render_progress_frame(0, 100, &["Stabilizing...".to_string()], false);

    let _ = fs::create_dir_all("/tmp");

    let url = format!("https://Amaterus1125.github.io/chpm/packages/{}.tar.gz", package);
    ui.render_progress_frame(20, 100, &["Downloading".to_string()], false);

    let mut response = reqwest::blocking::get(&url)
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Server returned HTTP {}", response.status()));
    }

    let tmp_path = format!("/tmp/{}.tar.gz", package);
    let mut tmp_file = File::create(&tmp_path).map_err(|e| e.to_string())?;
    response.copy_to(&mut tmp_file).map_err(|e| e.to_string())?;

    ui.render_progress_frame(60, 100, &["Extracting".to_string()], false);
    
    let tarball = File::open(&tmp_path).map_err(|e| e.to_string())?;
    let tar = GzDecoder::new(tarball);
    let mut archive = Archive::new(tar);

    archive.unpack("/").map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&tmp_path);
    
    ui.render_progress_frame(100, 100, &[], false);
    ui.finish();
    Ok(())
}
