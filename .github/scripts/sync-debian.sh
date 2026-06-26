#!/bin/bash
# sync-debian.sh
# For packages NOT found in Arch, try Debian stable as fallback.
# Skips packages already updated by sync-arch.sh.

set -e

PACKAGES_DIR="packages"
TMP="/tmp/chiral-sync-deb"

mkdir -p "$TMP"

for pkg_file in "$PACKAGES_DIR"/*.tar.gz; do
    pkgname=$(basename "$pkg_file" .tar.gz)

    # Skip if already updated by Arch sync this run
    # (Arch sync writes a .version file with "pkgver-pkgrel" format)
    version_file="${PACKAGES_DIR}/.${pkgname}.version"
    if [ -f "$version_file" ]; then
        arch_ver=$(cat "$version_file")
        # If version looks like it came from Arch (has a pkgrel), skip
        if echo "$arch_ver" | grep -q '-'; then
            echo "$pkgname — handled by Arch sync, skipping"
            continue
        fi
    fi

    echo "──────────────────────────────────────"
    echo "Checking Debian: $pkgname"

    # Fetch Debian download page
    page=$(curl -sf -A "chiral-sync-bot" \
        "https://packages.debian.org/stable/amd64/${pkgname}/download" || true)

    if [ -z "$page" ]; then
        echo "  Not in Debian stable — skipping"
        continue
    fi

    # Extract .deb URL
    deb_url=$(echo "$page" | grep -o 'href="[^"]*\.deb"' | head -1 | cut -d'"' -f2)

    if [ -z "$deb_url" ]; then
        echo "  No .deb URL found — skipping"
        continue
    fi

    # Parse version from filename: pkgname_VERSION_amd64.deb
    filename=$(basename "$deb_url")
    new_version=$(echo "$filename" | sed 's/.*_\(.*\)_amd64\.deb/\1/')

    # Check current version
    current_version=""
    if [ -f "$version_file" ]; then
        current_version=$(cat "$version_file")
    fi

    if [ "$current_version" = "$new_version" ]; then
        echo "  Already up to date ($new_version)"
        continue
    fi

    echo "  Updating $pkgname: $current_version → $new_version"

    # Download .deb
    deb_tmp="$TMP/${pkgname}.deb"
    if ! curl -sf -A "chiral-sync-bot" -o "$deb_tmp" "$deb_url"; then
        echo "  ⚠ Download failed — skipping"
        continue
    fi

    # Extract ar archive
    extract_dir="$TMP/deb-${pkgname}"
    rm -rf "$extract_dir"
    mkdir -p "$extract_dir"
    ar x "$deb_tmp" --output="$extract_dir" 2>/dev/null || \
        (cd "$extract_dir" && ar x "$deb_tmp")

    # Find data.tar.*
    data_tar=$(find "$extract_dir" -name 'data.tar.*' | head -1)
    if [ -z "$data_tar" ]; then
        echo "  ⚠ No data.tar found in .deb — skipping"
        continue
    fi

    # Extract data.tar into stage
    stage="$TMP/stage-deb-${pkgname}"
    rm -rf "$stage"
    mkdir -p "$stage"
    tar xf "$data_tar" -C "$stage"

    # Repack as .tar.gz
    tar czf "${PACKAGES_DIR}/${pkgname}.tar.gz" -C "$stage" .

    echo "$new_version" > "$version_file"
    echo "  ✓ Done: $pkgname $new_version [debian]"

    rm -rf "$extract_dir" "$stage" "$deb_tmp"
done

rm -rf "$TMP"
echo "──────────────────────────────────────"
echo "Debian sync complete."
