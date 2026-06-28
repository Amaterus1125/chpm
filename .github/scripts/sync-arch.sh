#!/bin/bash
# sync-arch.sh
# For every .tar.gz in packages/, check if Arch has a newer version.
# If yes, download, repack, replace.

# do NOT set -e — one package failure should not stop the whole sync
PACKAGES_DIR="packages"
ARCH_API="https://archlinux.org/packages/search/json/?name="
ARCH_MIRROR="https://mirror.rackspace.com/archlinux"
TMP="/tmp/chiral-sync"

mkdir -p "$TMP"

for pkg_file in "$PACKAGES_DIR"/*.tar.gz; do
    # Get package name from filename (strip .tar.gz)
    pkgname=$(basename "$pkg_file" .tar.gz)

    echo "──────────────────────────────────────"
    echo "Checking: $pkgname"

    # Query Arch API
    response=$(curl -s -A "chiral-sync-bot" "${ARCH_API}${pkgname}")
    count=$(echo "$response" | jq '.results | length')

    if [ "$count" -eq 0 ]; then
        echo "  Not in Arch repos — skipping"
        continue
    fi

    # Prefer core/extra, fall back to first result
    pkg_json=$(echo "$response" | jq '
        .results | 
        (map(select(.repo == "core" or .repo == "extra")) | first) //
        first
    ')

    repo=$(echo "$pkg_json"     | jq -r '.repo')
    arch=$(echo "$pkg_json"     | jq -r '.arch')
    pkgver=$(echo "$pkg_json"   | jq -r '.pkgver')
    pkgrel=$(echo "$pkg_json"   | jq -r '.pkgrel')
    arch_pkgname=$(echo "$pkg_json" | jq -r '.pkgname')

    new_version="${pkgver}-${pkgrel}"
    filename="${arch_pkgname}-${new_version}-${arch}.pkg.tar.zst"
    url="${ARCH_MIRROR}/${repo}/os/x86_64/${filename}"

    # Check if we already have this version by reading a version tag file
    version_file="${PACKAGES_DIR}/.${pkgname}.version"
    current_version=""
    if [ -f "$version_file" ]; then
        current_version=$(cat "$version_file")
    fi

    if [ "$current_version" = "$new_version" ]; then
        echo "  Already up to date ($new_version)"
        continue
    fi

    echo "  Updating $pkgname: $current_version → $new_version"

    # Download .pkg.tar.zst
    pkg_tmp="$TMP/${pkgname}.pkg.tar.zst"
    if ! curl -sf -A "chiral-sync-bot" -o "$pkg_tmp" "$url"; then
        echo "  ⚠ Download failed — skipping"
        continue
    fi

    # Extract into staging dir, excluding Arch metadata files
    stage="$TMP/stage-${pkgname}"
    rm -rf "$stage"
    mkdir -p "$stage"

    tar xf "$pkg_tmp" -C "$stage" \
        --exclude='.PKGINFO' \
        --exclude='.BUILDINFO' \
        --exclude='.MTREE' \
        --exclude='.INSTALL' 2>/dev/null || true

    # Repack as .tar.gz
    tar czf "${PACKAGES_DIR}/${pkgname}.tar.gz" -C "$stage" .

    # Save version so we don't re-download next week
    echo "$new_version" > "$version_file"

    echo "  ✓ Done: $pkgname $new_version [arch/$repo]"

    # Cleanup — chmod first to handle root-owned files from /etc in packages
    chmod -R u+w "$stage" 2>/dev/null || true
    rm -rf "$stage" "$pkg_tmp"
done

chmod -R u+w "$TMP" 2>/dev/null || true
rm -rf "$TMP"
echo "──────────────────────────────────────"
echo "Arch sync complete."
