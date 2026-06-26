pub mod ui;

use flate2::read::GzDecoder;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tar::Archive;
use crate::ui::ChiralUI;


const SERVER: &str = "https://raw.githubusercontent.com/Amaterus1125/chpm/main/packages";


// Paths
//
//   root user:
//     binaries      → /usr/local/bin
//     libraries     → /usr/local/lib
//     headers       → /usr/local/include
//     man pages     → /usr/local/share/man
//     everything else → /usr/local/share
//     DB            → /var/lib/chiral/
//
//   normal user:
//     everything    → ~/.local/   (mirrors the same structure)
//     DB            → ~/.local/share/chiral/


fn is_root() -> bool {
    unsafe { libc::getuid() == 0 }
}

fn home() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "$HOME is not set".to_string())
}

/// The install prefix — everything extracts relative to this
///   root  → /usr/local
///   user  → ~/.local
fn install_prefix() -> Result<PathBuf, String> {
    if is_root() {
        Ok(PathBuf::from("/usr/local"))
    } else {
        Ok(home()?.join(".local"))
    }
}

fn db_dir() -> Result<PathBuf, String> {
    if is_root() {
        Ok(PathBuf::from("/var/lib/chiral"))
    } else {
        Ok(home()?.join(".local").join("share").join("chiral"))
    }
}

fn db_file() -> Result<PathBuf, String> {
    Ok(db_dir()?.join("installed.db"))
}

// File tracking DB  — records every file a package installed so we can remove
// them cleanly later. Format:
//   [package=version]
//   /usr/local/bin/hello
//   /usr/local/lib/libhello.so


fn db_ensure() -> Result<(), String> {
    let dir  = db_dir()?;
    let file = db_file()?;
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create DB dir {:?}: {}", dir, e))?;
    if !file.exists() {
        File::create(&file)
            .map_err(|e| format!("Cannot create DB file: {}", e))?;
    }
    Ok(())
}

fn db_read_all() -> Result<String, String> {
    db_ensure()?;
    fs::read_to_string(db_file()?).map_err(|e| e.to_string())
}

/// Returns list of (name, version) for every installed package
pub fn db_list() -> Result<Vec<(String, String)>, String> {
    let raw = db_read_all()?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let inner = &line[1..line.len()-1];
            let mut parts = inner.splitn(2, '=');
            let name    = parts.next().unwrap_or("").to_string();
            let version = parts.next().unwrap_or("unknown").to_string();
            out.push((name, version));
        }
    }
    Ok(out)
}

/// Returns every file path recorded for a package
fn db_files_for(package: &str) -> Result<Vec<PathBuf>, String> {
    let raw = db_read_all()?;
    let mut in_block = false;
    let mut files    = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line == format!("[{}=", package) || line.starts_with(&format!("[{}=", package)) {
            in_block = true;
            continue;
        }
        if in_block {
            if line.starts_with('[') { break; } // next package block
            if !line.is_empty() {
                files.push(PathBuf::from(line));
            }
        }
    }
    Ok(files)
}

fn db_is_installed(package: &str) -> bool {
    db_list().unwrap_or_default()
        .iter()
        .any(|(n, _)| n == package)
}

/// Write or overwrite a package entry with its list of installed files
fn db_add(package: &str, version: &str, files: &[PathBuf]) -> Result<(), String> {
    let raw = db_read_all()?;

    // Rebuild DB, skipping any existing block for this package
    let mut new_content = String::new();
    let mut skip = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&format!("[{}=", package)) {
            skip = true;
            continue;
        }
        if skip && trimmed.starts_with('[') {
            skip = false;
        }
        if !skip {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    // Append new block
    new_content.push_str(&format!("[{}={}]\n", package, version));
    for f in files {
        new_content.push_str(&format!("{}\n", f.display()));
    }
    new_content.push('\n');

    fs::write(db_file()?, new_content).map_err(|e| e.to_string())
}

/// Remove a package entry from DB
fn db_remove_entry(package: &str) -> Result<(), String> {
    let raw = db_read_all()?;
    let mut new_content = String::new();
    let mut skip = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&format!("[{}=", package)) {
            skip = true;
            continue;
        }
        if skip && trimmed.starts_with('[') {
            skip = false;
        }
        if !skip {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    fs::write(db_file()?, new_content).map_err(|e| e.to_string())
}

// Download


fn download(url: &str, dest: &Path) -> Result<(), String> {
    let mut response = reqwest::blocking::get(url)
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Package not found (HTTP {}). Check the name and try again.",
            response.status()
        ));
    }

    let mut f = File::create(dest)
        .map_err(|e| format!("Cannot write temp file: {}", e))?;
    response.copy_to(&mut f)
        .map_err(|e| format!("Download failed: {}", e))?;
    Ok(())
}


// Extract — preserves full directory structure from tarball
// Your tarballs should be structured like:
//   usr/bin/hello
//   usr/lib/libhello.so.1
//   usr/lib/libhello.so -> libhello.so.1   (symlink)
//   usr/include/hello.h
//   usr/share/man/man1/hello.1
//
// Chiral strips the leading "usr/" and extracts relative to the install
// prefix (/usr/local for root, ~/.local for user), so files land at:
//   /usr/local/bin/hello
//   /usr/local/lib/libhello.so.1
//   etc.
// Returns list of every file actually placed on disk (for DB tracking)


fn extract(tarball: &Path, prefix: &Path) -> Result<Vec<PathBuf>, String> {
    let mut archive = Archive::new(GzDecoder::new(
        File::open(tarball).map_err(|e| e.to_string())?
    ));

    // Allow symlinks — needed for shared libs (libfoo.so → libfoo.so.1.2.3)
    archive.set_preserve_permissions(true);
    archive.set_unpack_xattrs(true);

    let mut placed: Vec<PathBuf> = Vec::new();

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| format!("Bad tar entry: {}", e))?;

        let raw = entry.path().map_err(|e| e.to_string())?;

        // Path-traversal guard — no `..` or absolute paths allowed
        let safe: PathBuf = raw.components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect();

        if safe.as_os_str().is_empty() { continue; }

        // Strip a leading "usr/" component if present so the package layout
        // usr/bin/foo → bin/foo, usr/lib/libfoo.so → lib/libfoo.so, etc.
        let rel: PathBuf = {
            let mut comps = safe.components();
            let first = comps.next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_default();
            if first == "usr" {
                comps.collect()          // strip "usr/"
            } else {
                safe.clone()             // keep as-is
            }
        };

        if rel.as_os_str().is_empty() { continue; }

        let dest = prefix.join(&rel);

        let entry_type = entry.header().entry_type();

        if entry_type.is_dir() {
            fs::create_dir_all(&dest)
                .map_err(|e| format!("Cannot create dir {:?}: {}", dest, e))?;
            continue;
        }

        // Create parent directories
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create dir {:?}: {}", parent, e))?;
        }

        if entry_type.is_symlink() {
            // Recreate the symlink
            let link_target = entry.link_name()
                .map_err(|e| e.to_string())?
                .ok_or("Symlink has no target")?;
            let link_target = PathBuf::from(link_target.as_ref());

            // Remove existing symlink if present
            if dest.exists() || dest.symlink_metadata().is_ok() {
                let _ = fs::remove_file(&dest);
            }

            std::os::unix::fs::symlink(&link_target, &dest)
                .map_err(|e| format!("Cannot create symlink {:?}: {}", dest, e))?;
            placed.push(dest);
            continue;
        }

        if entry_type.is_file() {
            entry.unpack(&dest)
                .map_err(|e| format!("Failed to unpack {:?}: {}", dest, e))?;

            // Set permissions from tarball, but ensure binaries are executable
            let mode = entry.header().mode().unwrap_or(0o644);
            let mut perms = fs::metadata(&dest).map_err(|e| e.to_string())?.permissions();
            perms.set_mode(mode);
            fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;

            placed.push(dest);
        }
    }

    // Run ldconfig if root so new libs are immediately visible
    if is_root() {
        let _ = std::process::Command::new("ldconfig").status();
    }

    Ok(placed)
}


// PATH / ldconfig hint


fn path_hint(prefix: &Path) {
    let bin_dir = prefix.join("bin");
    let lib_dir = prefix.join("lib");

    let in_path = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|p| Path::new(p) == bin_dir);

    if !in_path {
        eprintln!("\n Add to PATH:");
        eprintln!("   export PATH=\"{}:$PATH\"", bin_dir.display());
        eprintln!("   Add that line to ~/.bashrc to make it permanent.");
    }

    // Hint for user-local libs
    if !is_root() {
        let ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let lib_str = lib_dir.to_string_lossy();
        if !ld.contains(lib_str.as_ref()) {
            eprintln!("\n If a package has shared libs, also add:");
            eprintln!("   export LD_LIBRARY_PATH=\"{}:$LD_LIBRARY_PATH\"", lib_dir.display());
        }
    }
}


// PUBLIC API


/// chiral install <package>
pub fn install_binary(ui: &mut ChiralUI, package: &str) -> Result<(), String> {
    ui.draw_header("2.0");
    ui.render_progress_frame(0, 100, &["Checking...".to_string()], false);

    if db_is_installed(package) {
        return Err(format!(
            "'{}' is already installed. Use 'chiral update {}' to upgrade.",
            package, package
        ));
    }

    let prefix = install_prefix()?;
    fs::create_dir_all(&prefix)
        .map_err(|e| format!("Cannot create prefix dir: {}", e))?;

    let url = format!("{}/{}.tar.gz", SERVER, package);
    ui.render_progress_frame(20, 100, &[format!("Downloading {}", package)], false);

    let tmp = std::env::temp_dir().join(format!("chiral-{}.tar.gz", package));
    download(&url, &tmp)?;

    ui.render_progress_frame(60, 100, &["Extracting".to_string()], false);
    let placed = extract(&tmp, &prefix)?;
    let _ = fs::remove_file(&tmp);

    db_add(package, "latest", &placed)?;

    ui.render_progress_frame(100, 100, &[format!("Installed {} ({} files)", package, placed.len())], false);
    ui.finish();
    path_hint(&prefix);
    Ok(())
}

/// chiral remove <package>
pub fn remove_binary(ui: &mut ChiralUI, package: &str) -> Result<(), String> {
    ui.draw_header("2.0");

    if !db_is_installed(package) {
        return Err(format!("'{}' is not installed.", package));
    }

    // Remove every file the package installed
    let files = db_files_for(package)?;
    let mut removed = 0;
    for f in &files {
        if f.exists() || f.symlink_metadata().is_ok() {
            fs::remove_file(f)
                .map_err(|e| format!("Cannot remove {}: {}", f.display(), e))?;
            removed += 1;
        }
    }

    // Clean up any empty directories left behind
    for f in &files {
        if let Some(parent) = f.parent() {
            let _ = fs::remove_dir(parent); // silently fails if not empty — that's fine
        }
    }

    // Run ldconfig again after removal
    if is_root() {
        let _ = std::process::Command::new("ldconfig").status();
    }

    db_remove_entry(package)?;

    ui.render_progress_frame(100, 100, &[format!("Removed {} ({} files)", package, removed)], false);
    ui.finish();
    Ok(())
}

/// chiral update <package>  /  chiral upgrade (pass "all")
pub fn update_binary(ui: &mut ChiralUI, package: &str) -> Result<(), String> {
    ui.draw_header("2.0");

    if package == "all" {
        let installed = db_list()?;
        if installed.is_empty() {
            println!("Nothing to upgrade.");
            return Ok(());
        }
        for (name, _) in installed {
            println!("Upgrading {}...", name);
            remove_binary(ui, &name)?;
            install_binary(ui, &name)?;
        }
        return Ok(());
    }

    if !db_is_installed(package) {
        return Err(format!(
            "'{}' is not installed. Use 'chiral install {}' first.",
            package, package
        ));
    }

    ui.render_progress_frame(0, 100, &[format!("Updating {}", package)], false);

    let prefix = install_prefix()?;
    let url    = format!("{}/{}.tar.gz", SERVER, package);
    let tmp    = std::env::temp_dir().join(format!("chiral-{}.tar.gz", package));

    download(&url, &tmp)?;

    ui.render_progress_frame(60, 100, &["Extracting".to_string()], false);
    let placed = extract(&tmp, &prefix)?;
    let _ = fs::remove_file(&tmp);

    db_add(package, "latest", &placed)?;

    ui.render_progress_frame(100, 100, &[format!("Updated {}", package)], false);
    ui.finish();
    Ok(())
}

/// chiral search <query>
pub fn search_packages(query: &str) -> Result<(), String> {
    let api_url = "https://api.github.com/repos/Amaterus1125/chpm/contents/packages";

    let client   = reqwest::blocking::Client::new();
    let response = client
        .get(api_url)
        .header("User-Agent", "chiral-package-manager")
        .send()
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Cannot reach package index (HTTP {})", response.status()));
    }

    let text: String = response.text().map_err(|e| e.to_string())?;
    let body: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Failed to parse index: {}", e))?;

    let installed   = db_list().unwrap_or_default();
    let query_lower = query.to_lowercase();
    let files       = body.as_array().ok_or("Unexpected response from GitHub")?;

    let mut found = 0;
    println!("Search results for '{}':", query);
    println!("{}", "─".repeat(40));

    for file in files {
        let name = file["name"].as_str().unwrap_or("");
        if name.ends_with(".tar.gz") && name.to_lowercase().contains(&query_lower) {
            let pkg = name.trim_end_matches(".tar.gz");
            let tag = if installed.iter().any(|(n, _)| n == pkg) { " [installed]" } else { "" };
            println!("  {}{}", pkg, tag);
            found += 1;
        }
    }

    if found == 0 { println!("  No packages found matching '{}'.", query); }
    println!("{}", "─".repeat(40));
    println!("Found {} package(s)", found);
    Ok(())
}

/// chiral list
pub fn list_installed() -> Result<(), String> {
    let entries = db_list()?;

    if entries.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    println!("Installed packages:");
    println!("{}", "─".repeat(40));
    for (name, version) in &entries {
        println!("  {:<20} {}", name, version);
    }
    println!("{}", "─".repeat(40));
    println!("Total: {} package(s)", entries.len());
    Ok(())
}
