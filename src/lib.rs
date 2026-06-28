pub mod ui;

use flate2::read::GzDecoder;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tar::Archive;
use crate::ui::ChiralUI;

// Package repositories


const SERVER: &str = "https://raw.githubusercontent.com/Amaterus1125/chpm/main/packages";
const ARCH_MIRROR: &str = "https://mirror.rackspace.com/archlinux";


// Paths


fn is_root() -> bool {
    unsafe { libc::getuid() == 0 }
}

fn home() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "$HOME is not set".to_string())
}

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


// File tracking DB
// Format:
//   [pkgname=1.2.3|debian]
//   /usr/local/bin/foo


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

pub fn db_list() -> Result<Vec<(String, String, String)>, String> {
    let raw = db_read_all()?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let inner = &line[1..line.len()-1];
            let mut eq = inner.splitn(2, '=');
            let name = eq.next().unwrap_or("").to_string();
            let rest = eq.next().unwrap_or("unknown|unknown");
            let mut pipe = rest.splitn(2, '|');
            let version = pipe.next().unwrap_or("unknown").to_string();
            let source  = pipe.next().unwrap_or("unknown").to_string();
            out.push((name, version, source));
        }
    }
    Ok(out)
}

fn db_files_for(package: &str) -> Result<Vec<PathBuf>, String> {
    let raw = db_read_all()?;
    let mut in_block = false;
    let mut files    = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with(&format!("[{}=", package)) {
            in_block = true;
            continue;
        }
        if in_block {
            if line.starts_with('[') { break; }
            if !line.is_empty() { files.push(PathBuf::from(line)); }
        }
    }
    Ok(files)
}

fn db_is_installed(package: &str) -> bool {
    db_list().unwrap_or_default()
        .iter()
        .any(|(n, _, _)| n == package)
}

fn db_get_entry(package: &str) -> Option<(String, String)> {
    db_list().unwrap_or_default()
        .into_iter()
        .find(|(n, _, _)| n == package)
        .map(|(_, v, s)| (v, s))
}

fn db_add(package: &str, version: &str, source: &str, files: &[PathBuf]) -> Result<(), String> {
    let raw = db_read_all()?;
    let mut new_content = String::new();
    let mut skip = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&format!("[{}=", package)) {
            skip = true;
            continue;
        }
        if skip && trimmed.starts_with('[') { skip = false; }
        if !skip {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    new_content.push_str(&format!("[{}={}|{}]\n", package, version, source));
    for f in files {
        new_content.push_str(&format!("{}\n", f.display()));
    }
    new_content.push('\n');
    fs::write(db_file()?, new_content).map_err(|e| e.to_string())
}

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
        if skip && trimmed.starts_with('[') { skip = false; }
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


// Arch API — returns full package info including deps


struct ArchPkg {
    repo:     String,
    pkgname:  String,
    version:  String,   // pkgver-pkgrel
    arch:     String,
    depends:  Vec<String>,
}

fn arch_query(package: &str) -> Result<ArchPkg, String> {
    let api = format!(
        "https://archlinux.org/packages/search/json/?name={}",
        package
    );

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&api)
        .header("User-Agent", "chiral-package-manager")
        .send()
        .map_err(|e| format!("Arch API error: {}", e))?
        .text()
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = serde_json::from_str(&resp)
        .map_err(|e| format!("Arch API parse error: {}", e))?;

    let results = json["results"]
        .as_array()
        .ok_or("No results from Arch API")?;

    if results.is_empty() {
        return Err(format!("'{}' not found in Arch repos", package));
    }

    let pkg = results.iter()
        .find(|r| {
            let repo = r["repo"].as_str().unwrap_or("");
            repo == "core" || repo == "extra"
        })
        .or_else(|| results.first())
        .ok_or("No suitable Arch package found")?;

    let repo     = pkg["repo"].as_str().unwrap_or("extra").to_string();
    let arch_str = pkg["arch"].as_str().unwrap_or("x86_64").to_string();
    let pkgname  = pkg["pkgname"].as_str().unwrap_or(package).to_string();
    let pkgver   = pkg["pkgver"].as_str().unwrap_or("").to_string();
    let pkgrel   = pkg["pkgrel"].as_str().unwrap_or("1").to_string();
    let version  = format!("{}-{}", pkgver, pkgrel);

    // Parse depends array — entries can have version constraints like "glib2>=2.80"
    // We strip the version constraint and just keep the name
    let depends: Vec<String> = pkg["depends"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|d| d.as_str())
        .map(|d| {
            // Strip >=, <=, =, >, < version constraints
            d.split(|c| c == '>' || c == '<' || c == '=')
             .next()
             .unwrap_or(d)
             .trim()
             .to_string()
        })
        .collect();

    Ok(ArchPkg { repo, pkgname, version, arch: arch_str, depends })
}

fn arch_download_url(pkg: &ArchPkg) -> String {
    let filename = format!(
        "{}-{}-{}.pkg.tar.zst",
        pkg.pkgname, pkg.version, pkg.arch
    );
    format!("{}/{}/os/x86_64/{}", ARCH_MIRROR, pkg.repo, filename)
}

fn extract_pkg_zst(pkg_path: &Path, dest: &Path) -> Result<(), String> {
    let tmp_dir = pkg_path.parent().unwrap_or(Path::new("/tmp"))
        .join("arch_extracted");
    fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    let status = std::process::Command::new("tar")
        .args([
            "xf", pkg_path.to_str().unwrap(),
            "-C", tmp_dir.to_str().unwrap(),
            "--exclude=.PKGINFO",
            "--exclude=.BUILDINFO",
            "--exclude=.MTREE",
            "--exclude=.INSTALL",
        ])
        .status()
        .map_err(|e| format!("tar failed: {}", e))?;

    if !status.success() {
        return Err("Failed to extract .pkg.tar.zst — is zstd installed?".to_string());
    }

    let status = std::process::Command::new("tar")
        .args(["czf", dest.to_str().unwrap(), "-C", tmp_dir.to_str().unwrap(), "."])
        .status()
        .map_err(|e| format!("tar repack failed: {}", e))?;

    if !status.success() {
        return Err("Failed to repack Arch package as .tar.gz".to_string());
    }

    let _ = fs::remove_dir_all(&tmp_dir);
    Ok(())
}

fn try_arch(package: &str, dest: &Path) -> Result<(String, Vec<String>), String> {
    let pkg = arch_query(package)?;
    let version = pkg.version.clone();
    let deps    = pkg.depends.clone();
    let url     = arch_download_url(&pkg);

    let pkg_tmp = dest.parent().unwrap_or(Path::new("/tmp"))
        .join(format!("chiral-{}.pkg.tar.zst", package));

    download(&url, &pkg_tmp)?;
    extract_pkg_zst(&pkg_tmp, dest)?;
    let _ = fs::remove_file(&pkg_tmp);

    Ok((version, deps))
}

// Debian fallback


fn debian_find_deb(package: &str) -> Result<(String, String), String> {
    let client = reqwest::blocking::Client::new();
    let search_url = format!(
        "https://packages.debian.org/stable/amd64/{}/download",
        package
    );

    let page = client
        .get(&search_url)
        .header("User-Agent", "chiral-package-manager")
        .send()
        .map_err(|e| format!("Debian search error: {}", e))?
        .text()
        .map_err(|e| e.to_string())?;

    for line in page.lines() {
        if line.contains("deb.debian.org") && line.contains(".deb") {
            if let Some(start) = line.find("href=\"") {
                let rest = &line[start + 6..];
                if let Some(end) = rest.find('"') {
                    let url = &rest[..end];
                    if url.ends_with(".deb") {
                        let filename = url.split('/').last().unwrap_or("");
                        let version = filename
                            .trim_end_matches("_amd64.deb")
                            .splitn(2, '_')
                            .nth(1)
                            .unwrap_or("unknown")
                            .to_string();
                        return Ok((url.to_string(), version));
                    }
                }
            }
        }
    }

    Err(format!("Could not find '{}' in Debian stable", package))
}

fn extract_deb(deb_path: &Path, dest: &Path) -> Result<(), String> {
    let tmp_dir = deb_path.parent().unwrap_or(Path::new("/tmp")).join("deb_extracted");
    fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    let status = std::process::Command::new("ar")
        .args(["x", deb_path.to_str().unwrap()])
        .current_dir(&tmp_dir)
        .status()
        .map_err(|e| format!("ar not found: {}", e))?;

    if !status.success() {
        return Err("Failed to extract .deb with ar".to_string());
    }

    let data_tar = fs::read_dir(&tmp_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("data.tar"))
                .unwrap_or(false)
        })
        .ok_or("No data.tar.* found inside .deb")?;

    let stage = tmp_dir.join("stage");
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;

    let status = std::process::Command::new("tar")
        .args(["xf", data_tar.to_str().unwrap(), "-C", stage.to_str().unwrap()])
        .status()
        .map_err(|e| format!("tar failed: {}", e))?;

    if !status.success() {
        return Err("Failed to extract data.tar from .deb".to_string());
    }

    let status = std::process::Command::new("tar")
        .args(["czf", dest.to_str().unwrap(), "-C", stage.to_str().unwrap(), "."])
        .status()
        .map_err(|e| format!("tar repack failed: {}", e))?;

    if !status.success() {
        return Err("Failed to repack .deb data as .tar.gz".to_string());
    }

    let _ = fs::remove_dir_all(&tmp_dir);
    Ok(())
}

fn try_debian(package: &str, dest: &Path) -> Result<String, String> {
    let (deb_url, version) = debian_find_deb(package)?;
    let deb_tmp = dest.parent().unwrap_or(Path::new("/tmp"))
        .join(format!("chiral-{}.deb", package));
    download(&deb_url, &deb_tmp)?;
    extract_deb(&deb_tmp, dest)?;
    let _ = fs::remove_file(&deb_tmp);
    Ok(version)
}


// Dependency resolution
//
// Uses Arch API for dep lists (most complete and structured).
// Builds a full install order via BFS then topological sort so deepest
// deps install first.
//
// Returns Vec<String> in install order, NOT including packages already
// installed, NOT including the root package itself (caller installs that).

/// Strip version constraints from a dep string e.g. "glib2>=2.80" → "glib2"
fn strip_ver(dep: &str) -> String {
    dep.split(|c| c == '>' || c == '<' || c == '=' || c == ':')
        .next()
        .unwrap_or(dep)
        .trim()
        .to_string()
}

/// Detect which package manager the host OS uses
enum HostPm { Pacman, Apt, Rpm, Unknown }

fn detect_host_pm() -> HostPm {
    if std::process::Command::new("pacman").arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false) {
        return HostPm::Pacman;
    }
    if std::process::Command::new("dpkg").arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false) {
        return HostPm::Apt;
    }
    if std::process::Command::new("rpm").arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false) {
        return HostPm::Rpm;
    }
    HostPm::Unknown
}

/// Check if a dep is already satisfied by the host OS.
/// Checks 4 ways in order:
///   1. chiral's own DB (we installed it)
///   2. host package manager (pacman -Q / dpkg -s / rpm -q)
///   3. binary exists on PATH (for things like "sh", "bash")
///   4. shared lib visible to ldconfig (for "libfoo.so" style dep names)
fn system_has(dep: &str, host_pm: &HostPm) -> bool {
    // 1. Chiral DB
    if db_is_installed(dep) { return true; }

    // 2. Host package manager
    let pm_found = match host_pm {
        HostPm::Pacman => std::process::Command::new("pacman")
            .args(["-Q", dep])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),

        HostPm::Apt => std::process::Command::new("dpkg")
            .args(["-s", dep])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),

        HostPm::Rpm => std::process::Command::new("rpm")
            .args(["-q", dep])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),

        HostPm::Unknown => false,
    };
    if pm_found { return true; }

    // 3. Binary on PATH (handles "sh", "bash", "python", etc.)
    let bin_found = std::process::Command::new("sh")
        .args(["-c", &format!("command -v {}", dep)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if bin_found { return true; }

    // 4. Shared lib visible to ldconfig (handles "libfoo.so", "libz.so" etc.)
    if dep.contains(".so") {
        let ldconfig_found = std::process::Command::new("sh")
            .args(["-c", &format!("ldconfig -p 2>/dev/null | grep -q '{}'", dep)])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ldconfig_found { return true; }

        // Direct lib path check
        let lib_paths = [
            format!("/usr/lib/{}", dep),
            format!("/lib/{}", dep),
            format!("/usr/local/lib/{}", dep),
            format!("/usr/lib/x86_64-linux-gnu/{}", dep),
        ];
        if lib_paths.iter().any(|p| Path::new(p).exists()) {
            return true;
        }
    }

    // 5. pkg-config — covers manually compiled libraries that ran `make install`
    // e.g. alsa-lib installs alsa.pc, so `pkg-config --exists alsa` works
    let pc_found = std::process::Command::new("pkg-config")
        .args(["--exists", dep])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if pc_found { return true; }

    // Also try common pkg-config name variants:
    // dep "alsa-lib" → try "alsa", dep "libpng" → try "libpng16" etc.
    let variants = [
        dep.trim_start_matches("lib").to_string(),          // libfoo → foo
        dep.replace('-', "_"),                               // alsa-lib → alsa_lib
        dep.replace("lib", "").replace('-', ""),             // libfoo-bar → foobar
    ];
    for variant in &variants {
        let found = std::process::Command::new("pkg-config")
            .args(["--exists", variant])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if found { return true; }
    }

    // 6. Direct filesystem checks for manually installed packages
    // Checks /usr, /usr/local, /opt for binaries, libs, headers
    let search_name = dep.trim_start_matches("lib");
    let fs_checks = [
        // Binary
        format!("/usr/bin/{}", dep),
        format!("/usr/local/bin/{}", dep),
        format!("/usr/sbin/{}", dep),
        // Library (with common suffixes)
        format!("/usr/lib/lib{}.so", dep),
        format!("/usr/local/lib/lib{}.so", dep),
        format!("/usr/lib/lib{}.a", dep),
        // Header (covers manually compiled dev packages)
        format!("/usr/include/{}", dep),
        format!("/usr/include/{}", search_name),
        format!("/usr/local/include/{}", dep),
        // pkg-config file directly
        format!("/usr/lib/pkgconfig/{}.pc", dep),
        format!("/usr/share/pkgconfig/{}.pc", dep),
        format!("/usr/local/lib/pkgconfig/{}.pc", dep),
    ];
    if fs_checks.iter().any(|p| Path::new(p).exists()) {
        return true;
    }

    false
}

/// Resolve full dependency tree for a package.
/// Returns packages in install order (deps first, requested package last).
/// Skips anything already satisfied by chiral DB or host OS.
/// Detects circular deps and breaks the cycle rather than looping forever.
pub fn resolve_deps(package: &str) -> Result<Vec<String>, String> {
    let host_pm = detect_host_pm();

    // dep_map: pkgname → its direct deps (from Arch API)
    let mut dep_map: HashMap<String, Vec<String>> = HashMap::new();
    // visited: packages we've already fetched deps for
    let mut visited: HashSet<String> = HashSet::new();
    // queue for BFS
    let mut queue: VecDeque<String> = VecDeque::new();

    queue.push_back(package.to_string());

    while let Some(pkg) = queue.pop_front() {
        if visited.contains(&pkg) { continue; }
        visited.insert(pkg.clone());

        // Skip if already satisfied by system or chiral
        if system_has(&pkg, &host_pm) {
            dep_map.entry(pkg).or_default();
            continue;
        }

        // Query Arch for deps — if Arch doesn't know it, treat as no deps
        let deps = match arch_query(&pkg) {
            Ok(info) => info.depends,
            Err(_)   => vec![],
        };

        let clean_deps: Vec<String> = deps.iter()
            .map(|d| strip_ver(d))
            .filter(|d| !d.is_empty())
            .collect();

        for dep in &clean_deps {
            if !visited.contains(dep) {
                queue.push_back(dep.clone());
            }
        }

        dep_map.insert(pkg, clean_deps);
    }

    // Topological sort (Kahn's algorithm) so deps install before dependents
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for (pkg, deps) in &dep_map {
        in_degree.entry(pkg.clone()).or_insert(0);
        for dep in deps {
            if dep_map.contains_key(dep) {
                *in_degree.entry(dep.clone()).or_insert(0) += 0; // ensure key exists
                // pkg depends on dep → dep must come first
                // in_degree tracks how many things each node depends on
            }
        }
    }

    // Build reverse: for each pkg, which pkgs depend on it
    let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
    let mut indeg: HashMap<String, usize> = HashMap::new();

    for (pkg, deps) in &dep_map {
        indeg.entry(pkg.clone()).or_insert(0);
        for dep in deps {
            if dep_map.contains_key(dep) {
                reverse.entry(dep.clone()).or_default().push(pkg.clone());
                *indeg.entry(pkg.clone()).or_insert(0) += 1;
            }
        }
    }

    // Start with nodes that have no deps (in-degree 0)
    let mut ready: VecDeque<String> = indeg.iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();

    let mut order: Vec<String> = Vec::new();

    while let Some(pkg) = ready.pop_front() {
        order.push(pkg.clone());
        if let Some(dependents) = reverse.get(&pkg) {
            for dep in dependents {
                let d = indeg.entry(dep.clone()).or_insert(0);
                if *d > 0 { *d -= 1; }
                if *d == 0 { ready.push_back(dep.clone()); }
            }
        }
    }

    // Filter out: satisfied by system/chiral, and the root package itself
    let host_pm2 = detect_host_pm();
    let result: Vec<String> = order.into_iter()
        .filter(|p| p != package && !system_has(p, &host_pm2))
        .collect();

    Ok(result)
}


// Download with fallback chain — returns (source, version, deps)
// deps only populated when Arch is used (that's where we get them)


fn download_package(
    ui: &mut ChiralUI,
    package: &str,
    dest: &Path,
) -> Result<(String, String), String> {
    // Try 1: GitHub
    let url = format!("{}/{}.tar.gz", SERVER, package);
    ui.render_progress_frame(20, 100, &[format!("Trying GitHub packages/{}.tar.gz", package)], false);
    if download(&url, dest).is_ok() {
        return Ok(("github".to_string(), "latest".to_string()));
    }

    // Try 2: Debian
    ui.render_progress_frame(35, 100, &["Not in repo — trying Debian stable...".to_string()], false);
    if let Ok(version) = try_debian(package, dest) {
        return Ok(("debian".to_string(), version));
    }

    // Try 3: Arch
    ui.render_progress_frame(50, 100, &["Trying Arch Linux repos...".to_string()], false);
    if let Ok((version, _deps)) = try_arch(package, dest) {
        return Ok(("arch".to_string(), version));
    }

    Err(format!(
        "'{}' not found in GitHub packages, Debian stable, or Arch Linux repos.",
        package
    ))
}


// Install a single package (no dep resolution — used internally)


fn install_one(ui: &mut ChiralUI, package: &str, prefix: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("chiral-{}.tar.gz", package));
    let (source, version) = download_package(ui, package, &tmp)?;

    ui.render_progress_frame(65, 100, &[format!("Extracting {} {}", package, version)], false);
    let placed = extract(&tmp, prefix)?;
    let _ = fs::remove_file(&tmp);

    db_add(package, &version, &source, &placed)?;
    ui.render_progress_frame(100, 100, &[format!("✓ {} {} [{}]", package, version, source)], false);
    Ok(())
}


// Extract


fn extract(tarball: &Path, prefix: &Path) -> Result<Vec<PathBuf>, String> {
    let mut archive = Archive::new(GzDecoder::new(
        File::open(tarball).map_err(|e| e.to_string())?
    ));

    archive.set_preserve_permissions(true);
    archive.set_unpack_xattrs(true);

    let mut placed: Vec<PathBuf> = Vec::new();

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| format!("Bad tar entry: {}", e))?;
        let raw = entry.path().map_err(|e| e.to_string())?;

        let safe: PathBuf = raw.components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect();

        if safe.as_os_str().is_empty() { continue; }

        let rel: PathBuf = {
            let mut comps = safe.components();
            let first = comps.next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_default();
            if first == "usr" || first == "." {
                comps.collect()
            } else {
                safe.clone()
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

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create dir {:?}: {}", parent, e))?;
        }

        if entry_type.is_symlink() {
            let link_target = entry.link_name()
                .map_err(|e| e.to_string())?
                .ok_or("Symlink has no target")?;
            let link_target = PathBuf::from(link_target.as_ref());

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

            let mode = entry.header().mode().unwrap_or(0o644);
            let mut perms = fs::metadata(&dest).map_err(|e| e.to_string())?.permissions();
            perms.set_mode(mode);
            fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;

            placed.push(dest);
        }
    }

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
        eprintln!("\n💡 Add to PATH:");
        eprintln!("   export PATH=\"{}:$PATH\"", bin_dir.display());
        eprintln!("   Add that line to ~/.bashrc to make it permanent.");
    }

    if !is_root() {
        let ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let lib_str = lib_dir.to_string_lossy();
        if !ld.contains(lib_str.as_ref()) {
            eprintln!("\n💡 If a package has shared libs, also add:");
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

    // Resolve dependency tree
    ui.render_progress_frame(5, 100, &[format!("Resolving deps for {}...", package)], false);
    let deps = resolve_deps(package)?;

    if !deps.is_empty() {
        println!("\n📦 Will install {} dependencies first:", deps.len());
        for d in &deps {
            println!("   + {}", d);
        }
        println!();
    }

    // Install each dep in order (deepest first)
    let host_pm = detect_host_pm();
    for dep in &deps {
        if system_has(dep, &host_pm) {
            println!("  ✓ {} already on system — skipping", dep);
            continue;
        }
        println!("  Installing dep: {}", dep);
        install_one(ui, dep, &prefix)?;
    }

    // Install the requested package itself
    println!("\n  Installing: {}", package);
    install_one(ui, package, &prefix)?;

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

    let files = db_files_for(package)?;
    let mut removed = 0;
    for f in &files {
        if f.exists() || f.symlink_metadata().is_ok() {
            fs::remove_file(f)
                .map_err(|e| format!("Cannot remove {}: {}", f.display(), e))?;
            removed += 1;
        }
    }

    for f in &files {
        if let Some(parent) = f.parent() {
            let _ = fs::remove_dir(parent);
        }
    }

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
        for (name, _, _) in installed {
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

    let prefix = install_prefix()?;
    let tmp    = std::env::temp_dir().join(format!("chiral-{}.tar.gz", package));

    ui.render_progress_frame(0, 100, &[format!("Updating {}", package)], false);
    let (source, version) = download_package(ui, package, &tmp)?;

    ui.render_progress_frame(65, 100, &["Extracting".to_string()], false);
    let placed = extract(&tmp, &prefix)?;
    let _ = fs::remove_file(&tmp);

    db_add(package, &version, &source, &placed)?;

    ui.render_progress_frame(100, 100, &[format!("Updated {} → {} [{}]", package, version, source)], false);
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
            let tag = if installed.iter().any(|(n, _, _)| n == pkg) { " [installed]" } else { "" };
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
    println!("{}", "─".repeat(55));
    println!("  {:<20} {:<20} {}", "Name", "Version", "Source");
    println!("{}", "─".repeat(55));
    for (name, version, source) in &entries {
        println!("  {:<20} {:<20} {}", name, version, source);
    }
    println!("{}", "─".repeat(55));
    println!("Total: {} package(s)", entries.len());
    Ok(())
}

/// Get version string from the host package manager
fn get_system_version(package: &str, host_pm: &HostPm) -> Option<String> {
    let output = match host_pm {
        HostPm::Pacman => std::process::Command::new("pacman")
            .args(["-Q", package])
            .output().ok()?,
        HostPm::Apt => std::process::Command::new("dpkg")
            .args(["-s", package])
            .output().ok()?,
        HostPm::Rpm => std::process::Command::new("rpm")
            .args(["-q", package])
            .output().ok()?,
        HostPm::Unknown => return None,
    };

    let out = String::from_utf8_lossy(&output.stdout).to_string();
    match host_pm {
        // pacman -Q returns "pkgname version"
        HostPm::Pacman => out.split_whitespace().nth(1).map(|s| s.to_string()),
        // dpkg -s returns "Version: x.x.x" in the output
        HostPm::Apt => out.lines()
            .find(|l| l.starts_with("Version:"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string()),
        HostPm::Rpm => Some(out.trim().to_string()),
        HostPm::Unknown => None,
    }
}

/// Detect how a package got onto the system — chiral, pacman, dpkg, rustup, PATH, etc.
fn detect_install_source(package: &str, host_pm: &HostPm) -> String {
    // 1. Check pacman/dpkg/rpm
    let pm_has = match host_pm {
        HostPm::Pacman => std::process::Command::new("pacman")
            .args(["-Q", package])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false),
        HostPm::Apt => std::process::Command::new("dpkg")
            .args(["-s", package])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false),
        HostPm::Rpm => std::process::Command::new("rpm")
            .args(["-q", package])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false),
        HostPm::Unknown => false,
    };

    if pm_has {
        return match host_pm {
            HostPm::Pacman => "pacman".to_string(),
            HostPm::Apt    => "apt/dpkg".to_string(),
            HostPm::Rpm    => "rpm".to_string(),
            HostPm::Unknown => "system".to_string(),
        };
    }

    // 2. Check rustup (for rust, cargo, rustc)
    if ["rust", "rustc", "cargo", "rustup"].contains(&package) {
        let rustup_check = std::process::Command::new("rustup")
            .arg("show")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false);
        if rustup_check { return "rustup".to_string(); }
    }

    // 3. Check if binary exists on PATH
    let on_path = std::process::Command::new("sh")
        .args(["-c", &format!("command -v {}", package)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    if on_path { return "manual/PATH".to_string(); }

    // 4. pkg-config
    let pc = std::process::Command::new("pkg-config")
        .args(["--exists", package])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    if pc { return "manual/pkg-config".to_string(); }

    "unknown".to_string()
}

/// chiral info <package>
pub fn info_package(package: &str) -> Result<(), String> {
    let host_pm = detect_host_pm();

    // Not in chiral DB — check if it exists on the system anyway
    if !db_is_installed(package) {
        if system_has(package, &host_pm) {
            let source  = detect_install_source(package, &host_pm);
            let version = get_system_version(package, &host_pm)
                .unwrap_or_else(|| "unknown".to_string());

            println!("{}", "─".repeat(55));
            println!("  Package : {}", package);
            println!("  Status  : not managed by chiral");
            println!("  Version : {}", version);
            println!("  Source  : {} (installed outside chiral)", source);
            println!();
            println!("  Tip: chiral did not install this package.");
            println!("       Use {} to manage it.", source);
            println!("{}", "─".repeat(55));
            return Ok(());
        }
        return Err(format!("'{}' is not installed by chiral or found on this system.", package));
    }

    let (version, source) = db_get_entry(package)
        .ok_or(format!("'{}' not found in DB", package))?;

    let files  = db_files_for(package)?;
    let prefix = install_prefix()?;

    // Also show deps from Arch if available
    let deps = arch_query(package)
        .map(|p| p.depends)
        .unwrap_or_default();

    println!("{}", "─".repeat(55));
    println!("  Package : {}", package);
    println!("  Version : {}", version);
    println!("  Source  : {}", source);
    println!("  Prefix  : {}", prefix.display());
    println!("  Files   : {}", files.len());

    if !deps.is_empty() {
        println!("  Deps    : {}", deps.join(", "));
    }

    println!("{}", "─".repeat(55));

    let mut bins: Vec<&PathBuf> = Vec::new();
    let mut libs: Vec<&PathBuf> = Vec::new();
    let mut hdrs: Vec<&PathBuf> = Vec::new();
    let mut mans: Vec<&PathBuf> = Vec::new();
    let mut rest: Vec<&PathBuf> = Vec::new();

    for f in &files {
        let s = f.to_string_lossy();
        if s.contains("/bin/")      { bins.push(f); }
        else if s.contains("/lib/") { libs.push(f); }
        else if s.contains("/include/") { hdrs.push(f); }
        else if s.contains("/man/") { mans.push(f); }
        else                        { rest.push(f); }
    }

    let print_group = |label: &str, group: &[&PathBuf]| {
        if !group.is_empty() {
            println!("  {}:", label);
            for f in group { println!("    {}", f.display()); }
        }
    };

    print_group("Binaries",  &bins);
    print_group("Libraries", &libs);
    print_group("Headers",   &hdrs);
    print_group("Man pages", &mans);
    print_group("Other",     &rest);

    println!("{}", "─".repeat(55));
    Ok(())
}

/// chiral deps <package> — show what would be installed without installing
pub fn show_deps(package: &str) -> Result<(), String> {
    println!("Resolving dependencies for '{}'...", package);

    let deps = resolve_deps(package)?;

    if deps.is_empty() {
        println!("  No uninstalled dependencies found.");
        return Ok(());
    }

    println!("\n📦 Would install {} package(s) in this order:", deps.len());
    println!("{}", "─".repeat(40));
    for (i, dep) in deps.iter().enumerate() {
        println!("  {}. {}", i + 1, dep);
    }
    println!("{}", "─".repeat(40));
    println!("Then: {}", package);
    Ok(())
}


/// chiral self-update — downloads latest binary from GitHub releases
pub fn self_update() -> Result<(), String> {
    const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
    const RELEASES_API: &str = "https://api.github.com/repos/Amaterus1125/Chiral-CrossDistro-Package-Manager/releases/latest";

    println!("Current version: v{}", CURRENT_VERSION);
    println!("Checking for updates...");

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(RELEASES_API)
        .header("User-Agent", "chiral-package-manager")
        .send()
        .map_err(|e| format!("Network error: {}", e))?
        .text()
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value = serde_json::from_str(&resp)
        .map_err(|e| format!("Parse error: {}", e))?;

    let latest = json["tag_name"]
        .as_str()
        .ok_or("Could not get latest version")?
        .trim_start_matches('v');+

    if latest == CURRENT_VERSION {
        println!("Already up to date (v{}).", CURRENT_VERSION);
        return Ok(());
    }

    println!("New version available: v{} → updating...", latest);

    let assets = json["assets"].as_array()
        .ok_or("No assets in release")?;

    let binary_url = assets.iter()
        .find(|a| {
    let name = a["name"].as_str().unwrap_or("");
    name == "chiral" || name.starts_with("chiral-x86_64")
})
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or("No 'chiral' binary found in release assets")?
        .to_string();

    let tmp = std::env::temp_dir().join("chiral-new");
    download(&binary_url, &tmp)?;

    std::fs::set_permissions(&tmp,
        std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .map_err(|e| e.to_string())?;

    let current = std::env::current_exe()
        .map_err(|e| format!("Cannot find current binary: {}", e))?;

    std::fs::rename(&tmp, &current)
        .map_err(|e| format!("Cannot replace binary (try sudo): {}", e))?;

    println!("✓ Updated to v{} — restart chiral to use new version.", latest);
    Ok(())
}
