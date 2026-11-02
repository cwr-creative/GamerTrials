#!/usr/bin/env bash
# Builds a portable GamerTrials folder and tarball for testing on another
# Linux machine: the Linux twin of tools/package.ps1, laid out the same way
# (as the repository root is, since every path the programs open is relative
# to it).
#
#   bash tools/package.sh [--skip-build]
#
# ROMs are left out. The platform never distributes ROM bytes (CLAUDE.md):
# assets/ROMs/README.txt in the package lists each file the manifests expect,
# where it goes and the sha1 it must have, and the player supplies their own.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [ "${1:-}" != "--skip-build" ]; then
    cargo build --release -p app-client -p server-net
fi

commit="$(git rev-parse --short HEAD)"
if [ -n "$(git status --porcelain)" ]; then commit="$commit-dirty"; fi
name="GamerTrials-$commit-linux"
out="$root/dist/$name"
rm -rf "$out"
mkdir -p "$out"

# Copy every file under $1 matching $2 to the same relative path in the package.
copy_tree() {
    (cd "$root" && find "$1" -type f -name "$2" -print0) | while IFS= read -r -d '' f; do
        mkdir -p "$out/$(dirname "$f")"
        cp "$root/$f" "$out/$f"
    done
}

bin="${CARGO_TARGET_DIR:-target}/release"
cp "$bin/app-client" "$out/GamerTrials"
cp "$bin/gamertrials-server" "$out/"
# The server and every client must hash the same manifests, so the package
# carries exactly the repository's.
copy_tree manifests '*.toml'
copy_tree cores '*.so'
# Not states/vault: captures kept for reference, named by no manifest.
mkdir -p "$out/assets/states"
cp assets/states/*.state "$out/assets/states/"
copy_tree assets/patches '*.bps'

# Each [[rom]] a manifest names by path, with its sha1: what the player must
# put where. Patched ROMs carry a patch instead and are derived on first use.
mkdir -p "$out/assets/ROMs"
{
    echo 'Put your own dumps here. GamerTrials ships no ROMs.'
    echo
    echo 'Each file below goes at the path shown, relative to the GamerTrials folder,'
    echo 'and must have exactly this sha1. The lobby shows any that are missing or'
    echo 'differ; a round on a game one player cannot run is voided for everyone,'
    echo 'so the host should enable only games every player has.'
    echo
    for m in manifests/*.toml; do
        awk -v game="$(basename "$m" .toml)" '
            function flush() { if (inrom && path != "") printf "[%s]\n  %s\n  sha1 %s\n\n", game, path, sha1 }
            /^[ \t]*\[\[rom\]\]/ { flush(); inrom = 1; path = ""; sha1 = ""; next }
            /^[ \t]*\[/          { flush(); inrom = 0; next }
            inrom && match($0, /^[ \t]*path[ \t]*=[ \t]*"[^"]+"/) { split($0, a, "\""); path = a[2] }
            inrom && match($0, /^[ \t]*sha1[ \t]*=[ \t]*"[0-9a-f]+"/) { split($0, a, "\""); sha1 = a[2] }
            END { flush() }
        ' "$m"
    done
    echo 'DOOM is the shareware 1.9 bundle zipped exactly as in the repository:'
    echo 're-zipping it changes the hash (ADR 0018), so copy that zip, do not rebuild it.'
} > "$out/assets/ROMs/README.txt"

# The client's log goes to a file beside it, as on Windows.
cat > "$out/GamerTrials.sh" <<'EOF'
#!/usr/bin/env bash
cd "$(dirname "$0")"
./GamerTrials "$@" 2>&1 | tee gamertrials.log
EOF
cat > "$out/start-server.sh" <<'EOF'
#!/usr/bin/env bash
cd "$(dirname "$0")"
./gamertrials-server --listen 0.0.0.0:7878 --manifests manifests "$@"
EOF
chmod +x "$out/GamerTrials" "$out/gamertrials-server" "$out/GamerTrials.sh" "$out/start-server.sh"

protocol="$(sed -n 's/^pub const VERSION: u32 = \([0-9]*\);.*/\1/p' crates/protocol/src/lib.rs)"
cat > "$out/README.txt" <<EOF
GamerTrials $commit (protocol v$protocol), built $(date '+%Y-%m-%d %H:%M'), for Linux

1. Copy your ROM dumps into assets/ROMs as assets/ROMs/README.txt lists.
2. Run ./GamerTrials.sh. It starts the client and writes gamertrials.log
   here; send that file along with any bug report.
3. Join game takes the invite the host sends: host[:port]/CODE.

To host a server on this machine, run ./start-server.sh and forward TCP
7878 to it. Every player must run the same build: a client whose manifests
hash differently from the server's is refused, and says which hash it holds.

Settings are kept in ~/.config/com.cwr.gamertrials/settings.toml.
Needs an X11 session or XWayland (every Ubuntu desktop has one), the
WebKitGTK 4.1 runtime, and libav's H.264 decoder to watch another player:
sudo apt install libwebkit2gtk-4.1-0 gstreamer1.0-libav
EOF

tar -C "$root/dist" -czf "$out.tar.gz" "$name"
echo "Packaged $out"
echo "      -> $out.tar.gz ($(du -h "$out.tar.gz" | cut -f1))"
