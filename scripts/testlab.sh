#!/bin/bash
# Test lab: small Avid drives and projects that can be reset to the same state again and again.
# Mountpoint mounts the drive folders as volumes; the app runs with its own HOME and only sees the lab drives.
#
#   scripts/testlab.sh setup      create the lab and the test clips (once)
#   scripts/testlab.sh snapshot   save the current lab drives + projects as the clean state (after Media Composer made them)
#   scripts/testlab.sh reset      restore the clean state, apply the test situations, clear the app state
#   scripts/testlab.sh start a|b  start the app as system A or B
#   scripts/testlab.sh status
set -eu

LAB="${MFC_LAB:-$HOME/MFC-Testlab}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE="$REPO/dist/MediaFiles Check.app"
APP="$BUNDLE/Contents/MacOS/MediaFiles Check"
DRIVES_A="MFC_A1,MFC_A2"
DRIVES_B="MFC_B1"
ALL="MFC_A1 MFC_A2 MFC_B1"
LIMIT=100

die() { echo "✗ $*" >&2; exit 1; }
say() { echo "• $*"; }

# Everything that deletes checks this first: only inside a folder this script created.
guard() {
  [ -f "$LAB/.mfc-testlab" ] || die "$LAB ist kein Testlabor (Markierung .mfc-testlab fehlt)."
}
quiet() {
  pgrep -x AvidMediaComposer >/dev/null && die "Media Composer läuft – bitte erst beenden."
  for s in a b; do
    local pid; pid="$(cat "$LAB/pid-$s" 2>/dev/null || true)"
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && die "MediaFiles Check (System $(echo $s | tr ab AB)) läuft – bitte erst beenden."
  done
  return 0
}
# first MXF folder Media Composer made on a drive (e.g. "<computer name>.1")
main_folder() {
  find "$LAB/drives/$1/Avid MediaFiles/MXF" -mindepth 1 -maxdepth 1 -type d ! -name '.*' | sort | head -1
}
mxfs() { find "$1" -maxdepth 1 -type f -iname '*.mxf' | sort; }

setup() {
  mkdir -p "$LAB"
  touch "$LAB/.mfc-testlab"
  for d in $ALL; do mkdir -p "$LAB/drives/$d/Avid MediaFiles/MXF"; done
  mkdir -p "$LAB/projects" "$LAB/clips" "$LAB/aside"
  if [ -z "$(ls "$LAB/clips")" ]; then
    command -v ffmpeg >/dev/null || die "ffmpeg fehlt (brew install ffmpeg)."
    say "Erzeuge 70 Testclips (je 1 s, 1080p25, Stereo-Ton) in $LAB/clips …"
    for i in $(seq -w 1 70); do
      ffmpeg -loglevel error -f lavfi -i "testsrc2=size=1920x1080:rate=25:duration=1" \
        -f lavfi -i "sine=frequency=$((200 + 10#$i * 10)):duration=1" -ac 2 \
        -c:v libx264 -pix_fmt yuv420p -c:a aac "$LAB/clips/Clip_$i.mov"
    done
  fi
  say "Testlabor liegt in $LAB"
  cat <<EOF

Einmalig von Hand:
  1. Mountpoint → Neues Projekt „MFC Testlabor“, darin drei Workspaces:
       $LAB/drives/MFC_A1
       $LAB/drives/MFC_A2
       $LAB/drives/MFC_B1
     und alle mounten (/Volumes/MFC_A1 …).
  2. Media Composer: neues Projekt „MFC Test“ (1080p/25) im Ordner $LAB/projects.
     - Bin „A1“: Clip_01 … Clip_60 importieren, Ziel-Laufwerk MFC_A1 (DNxHD/DNxHR LB reicht).
     - Bin „A2“: Clip_61 … Clip_70 importieren, Ziel-Laufwerk MFC_A2.
     - Sequenz „Test“ in Bin „A2“ mit ein paar Clips aus beiden Bins.
     - Bin „AMA“: einen Clip aus $LAB/clips nur verlinken (Source Browser → Link).
     Media Composer beenden.
  3. scripts/testlab.sh snapshot
EOF
}

snapshot() {
  guard; quiet
  for d in MFC_A1 MFC_A2; do [ -n "$(main_folder $d)" ] || die "Auf $d gibt es noch keinen MXF-Ordner – erst in Media Composer importieren."; done
  rm -rf "$LAB/golden.new"
  mkdir -p "$LAB/golden.new/drives"
  for d in $ALL; do cp -cR "$LAB/drives/$d" "$LAB/golden.new/drives/$d"; done
  cp -cR "$LAB/projects" "$LAB/golden.new/projects"
  rm -rf "$LAB/golden"
  mv "$LAB/golden.new" "$LAB/golden"
  say "Sauberer Stand gespeichert ($(find "$LAB/golden" -iname '*.mxf' | wc -l | tr -d ' ') MXF-Dateien)."
}

# empty a folder's contents (not the folder itself: Mountpoint mirrors the drive folders)
clear_into() {
  case "$1" in "$LAB"/*) ;; *) die "Nicht im Testlabor: $1";; esac
  find "$1" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
}

reset() {
  guard; quiet
  [ -d "$LAB/golden" ] || die "Noch kein sauberer Stand – erst: scripts/testlab.sh snapshot"
  for d in $ALL; do
    clear_into "$LAB/drives/$d"
    cp -cR "$LAB/golden/drives/$d/" "$LAB/drives/$d/"
  done
  clear_into "$LAB/projects"; cp -cR "$LAB/golden/projects/" "$LAB/projects/"
  clear_into "$LAB/aside"

  local f1 f2
  f1="$(main_folder MFC_A1)"; f2="$(main_folder MFC_A2)"
  [ -n "$f1" ] && [ -n "$f2" ] || die "Im sauberen Stand fehlen die MXF-Ordner auf MFC_A1/MFC_A2."

  # Exchange: system B has the complete A2 folder (made before A2 loses files)
  cp -cR "$f2" "$LAB/drives/MFC_B1/Avid MediaFiles/MXF/"
  # Check + Projects: two media files of A2 are gone (kept in aside/, never deleted)
  mxfs "$f2" | head -2 | while read -r m; do mv "$m" "$LAB/aside/"; done
  # Duplicates + Check (no database): copies of four A1 files in a folder of their own
  local dup="$LAB/drives/MFC_A2/Avid MediaFiles/MXF/Kopie"
  mkdir -p "$dup"
  mxfs "$f1" | head -4 | while read -r m; do cp -c "$m" "$dup/"; done
  # Rebalancing: A1 holds every clip of bin A1 (> $LIMIT files); the limit is set to $LIMIT below

  for s in a b; do
    local h="$LAB/home-$s"
    clear_into "$h" 2>/dev/null || mkdir -p "$h"
    local dir="$h/Library/Application Support/MediaFiles Check"
    mkdir -p "$dir"
    printf 'de\ndark\n0\nfinal\n%s\n' "$LIMIT" > "$dir/settings.txt"
    [ "${FRESH:-}" = 1 ] || touch "$dir/welcome-seen"
  done
  say "Zurückgesetzt. MFC_A1: $(mxfs "$f1" | wc -l | tr -d ' ') Dateien in „$(basename "$f1")“, Grenze $LIMIT."
  say "A2: 2 Medien fehlen (in aside/), Ordner „Kopie“ mit 4 Duplikaten; B1 hat „$(basename "$f2")“ komplett."
}

start() {
  guard
  [ -x "$APP" ] || die "Keine App in dist/ – erst scripts/build-mac-app.sh"
  local h="$LAB/home-$1" drives name
  case "$1" in
    a) drives="$DRIVES_A"; name="" ;;
    b) drives="$DRIVES_B"; name="EDIT-B" ;;
    *) die "start a oder start b" ;;
  esac
  [ -d "$h" ] || die "Erst: scripts/testlab.sh reset"
  for d in ${drives//,/ }; do [ -d "/Volumes/$d/Avid MediaFiles/MXF" ] || say "Achtung: /Volumes/$d ist nicht gemountet (Mountpoint)."; done
  open -n --env HOME="$h" --env MFC_LAB_DRIVES="$drives" --env MFC_LAB_COMPUTER="$name" "$BUNDLE"
  sleep 2
  pgrep -n -f "$APP" > "$LAB/pid-$1" || true
  say "System $(echo "$1" | tr ab AB) gestartet (Laufwerke $drives)."
}

status() {
  guard
  say "Labor: $LAB   sauberer Stand: $([ -d "$LAB/golden" ] && echo ja || echo nein)"
  for d in $ALL; do
    local m; m="$([ -d "/Volumes/$d" ] && echo gemountet || echo 'nicht gemountet')"
    echo "  $d ($m):"
    find "$LAB/drives/$d/Avid MediaFiles/MXF" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort | while read -r f; do
      echo "    $(basename "$f"): $(mxfs "$f" | wc -l | tr -d ' ') MXF"
    done
  done
}

case "${1:-}" in
  setup) setup ;;
  snapshot) snapshot ;;
  reset) reset ;;
  start) start "${2:-}" ;;
  status) status ;;
  *) sed -n 2,9p "$0"; exit 1 ;;
esac
