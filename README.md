<div align="center">

# ⚗️ Chiral Package Manager

**A fast, dependency-aware package manager built in Rust**  
*Born from a custom Linux distro built entirely from scratch using LFS/BLFS*

[![Rust](https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?style=flat-square&logo=linux)](https://kernel.org/)

</div>

---

## What is Chiral?

Chiral is a binary package manager that works on **any Linux system** — including custom distros, LFS/BLFS builds, Arch, Debian, and anything in between.

Instead of requiring a specific distro or package format, Chiral uses a **3-way fallback chain** to find and install packages:

```
Your GitHub repo packages/
        ↓ (not found?)
  Debian stable repos
        ↓ (not found?)
   Arch Linux repos
```

If a package exists anywhere in that chain, Chiral will find it, download it, and install it — automatically resolving all dependencies first.

---

## Features

- 🔗 **Automatic dependency resolution** — full recursive dep tree, installed in the right order
- 🌐 **3-way fallback** — GitHub → Debian → Arch, so almost any package is available
- 🧠 **Smart system detection** — checks pacman, dpkg, pkg-config, ldconfig, PATH, and the filesystem before downloading anything already present
- 📦 **Clean installs and removes** — every installed file is tracked, `chiral remove` leaves no orphans
- 🔢 **Real version pinning** — stores actual version strings from Debian/Arch APIs, not just "latest"
- 👤 **Root and user modes** — installs system-wide as root, or into `~/.local` as a regular user
- 🔄 **Weekly auto-sync** — GitHub Actions automatically keeps your package repo up to date
- 🦀 **Written in Rust** — fast, safe, no runtime dependencies

---

## How it works

### Installation

```
chiral install gtk3
```

1. **Resolve deps** — queries Arch Linux API recursively to build the full dependency tree
2. **Check what's already there** — each dep is checked against: chiral DB → system package manager → PATH → ldconfig → pkg-config → filesystem. Anything already present is skipped
3. **Download missing deps** — tries GitHub packages/ first, then Debian, then Arch
4. **Install in order** — deepest dependencies first, requested package last
5. **Track files** — every installed file path is recorded in the local DB for clean removal later

### Dependency resolution

Chiral uses **BFS + Kahn's topological sort**:

```
gtk3
 ├── glib2       ← installed first
 ├── cairo
 │    └── pixman ← installed before cairo
 ├── pango
 └── gdk-pixbuf2
```

Circular dependencies are detected and broken automatically.

### File tracking DB

Every package install is recorded at:
- Root installs: `/var/lib/chiral/installed.db`
- User installs: `~/.local/share/chiral/installed.db`

Format:
```
[alsa-lib=1.2.10-1|arch]
/usr/local/lib/libasound.so
/usr/local/include/alsa/asoundlib.h
...
```

---

## Installation

### Download the binary

Grab the latest release from the [Releases page](../../releases) and put it in your PATH:

```bash
# As root (system-wide)
sudo cp chiral /usr/local/bin/chiral

# As user
cp chiral ~/.local/bin/chiral
export PATH="$HOME/.local/bin:$PATH"  # add to ~/.bashrc
```

### Build from source

Requires Rust 1.70+:

```bash
git clone https://github.com/Amaterus1125/Chiral-Package-Manager---For-Custom-Distro-made-using-LFS-BLFS
cd Chiral-Package-Manager---For-Custom-Distro-made-using-LFS-BLFS
cargo build --release
sudo cp target/release/chiral /usr/local/bin/chiral
```

---

## Usage

```bash
chiral install <package>    # Install a package and all its dependencies
chiral remove  <package>    # Remove an installed package
chiral update  <package>    # Update a package to the latest version
chiral upgrade              # Update all installed packages
chiral search  <query>      # Search available packages
chiral list                 # List installed packages with version and source
chiral info    <package>    # Show version, source, deps, and installed files
chiral deps    <package>    # Preview what would be installed (dry run)
```

### Examples

```bash
# See what installing ffmpeg would pull in — without installing anything
chiral deps ffmpeg

# Install nano (chiral figures out all deps automatically)
chiral install nano

# See info about an installed package
chiral info steam

# Remove a package cleanly
chiral remove nano
```

---

## Supported platforms

Chiral runs on any **Linux x86_64** system:

| Platform | Support | Notes |
|---|---|---|
| Arch Linux | ✅ Full | pacman used for dep checking |
| Debian / Ubuntu | ✅ Full | dpkg used for dep checking |
| LFS / BLFS | ✅ Full | filesystem + ldconfig + pkg-config used |
| Any Linux x86_64 | ✅ Full | falls back to PATH + ldconfig checks |
| macOS / Windows | ❌ | Linux only |

---

## Running as root vs user

| | Root (`sudo chiral`) | User (`chiral`) |
|---|---|---|
| Install prefix | `/usr/local` | `~/.local` |
| DB location | `/var/lib/chiral/` | `~/.local/share/chiral/` |
| ldconfig | runs automatically | skipped |
| Who can use it | everyone | current user only |

---

## Auto-sync (GitHub Actions)

The `packages/` folder in this repo is automatically kept up to date every week via GitHub Actions. Every Sunday at midnight UTC, the workflow:

1. Checks Arch Linux repos for newer versions of every package
2. Downloads and repacks updated packages as `.tar.gz`
3. Falls back to Debian for packages not in Arch
4. Commits only if something actually changed

You can also trigger a sync manually from the Actions tab.

---

## Adding your own packages

Drop a `.tar.gz` into `packages/` with the structure:

```
pkgname.tar.gz
└── usr/
    ├── bin/pkgname
    ├── lib/libpkgname.so
    └── include/pkgname/
```

Chiral will find it automatically on the next `chiral install pkgname`.

---

## Origin story

Chiral was built as the package manager for a custom Linux distribution assembled entirely from scratch using [Linux From Scratch (LFS)](https://www.linuxfromscratch.org/) and [Beyond LFS (BLFS)](https://www.linuxfromscratch.org/blfs/). Every package in the base system — GCC, glibc, systemd, XFCE — was compiled by hand. Chiral was created so that distro could install software without depending on any existing package manager.

---

## License

MIT — do whatever you want with it.
