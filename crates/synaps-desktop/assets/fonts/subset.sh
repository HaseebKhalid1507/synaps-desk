#!/bin/bash
# Rebuild the embedded fonts: download the upstream OFL fonts, subset them with
# pyftsubset (fonttools) and rename the families.
#
# Renamed because IBM Plex carries the Reserved Font Name "Plex" (OFL 1.1 §3: a
# Modified Version, which a subset is, must not use it), and so that an
# installed full Rajdhani / IBM Plex never shadows (or is shadowed by) these
# subsets in the font database.
#
#   Hybrid Display   <- Rajdhani SemiBold        (Indian Type Foundry, OFL)   Latin
#   Hybrid Sans      <- IBM Plex Sans Regular/SemiBold (IBM, OFL, RFN Plex)   Latin
#   Hybrid Mono      <- IBM Plex Mono Regular    (IBM, OFL, RFN Plex)         Latin + box drawing, block elements
#   Hybrid Labels JP <- IBM Plex Sans JP Regular (IBM, OFL, RFN Plex)         ONLY the label glyphs below
#
# Keep JP_LABELS in step with `fonts::JP_LABELS` (a unit test checks every one
# of those characters is in the subset). Never ship the full CJK font.
# usage: crates/synaps-desktop/assets/fonts/subset.sh   (needs curl, python3 + fonttools)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
JP_LABELS="セッション文脈トークン変更固定稼働履歴お気に入り"
LATIN="U+0020-007E,U+00A0-017F,U+0192,U+02C6-02DC,U+2000-206F,U+20AC,U+2122,U+2190-2193,U+2212,U+2713"
# Tool output (`tree`, tables, progress bars) keeps its columns in the mono face.
MONO="$LATIN,U+2500-259F"
DISPLAY="U+0020-007E,U+00A0-00FF,U+2013-2014,U+2018-201D,U+2022,U+2026,U+20AC,U+2212"
gf=https://raw.githubusercontent.com/google/fonts/main/ofl
plex=https://raw.githubusercontent.com/IBM/plex/master/packages/plex-sans/fonts/complete/ttf
get() { curl -sfL -o "$tmp/$2" "$1"; sha256sum "$tmp/$2" | sed "s|$tmp/||"; }
get "$gf/rajdhani/Rajdhani-SemiBold.ttf" Rajdhani-SemiBold.ttf
get "$plex/IBMPlexSans-Regular.ttf" IBMPlexSans-Regular.ttf
get "$plex/IBMPlexSans-SemiBold.ttf" IBMPlexSans-SemiBold.ttf
get "$gf/ibmplexmono/IBMPlexMono-Regular.ttf" IBMPlexMono-Regular.ttf
get "$gf/ibmplexsansjp/IBMPlexSansJP-Regular.ttf" IBMPlexSansJP-Regular.ttf
sub() { pyftsubset "$tmp/$1" "$2" --name-IDs='*' --no-hinting --output-file="$tmp/sub-$3"; }
sub Rajdhani-SemiBold.ttf --unicodes="$DISPLAY" HybridDisplay-SemiBold.ttf
sub IBMPlexSans-Regular.ttf --unicodes="$LATIN" HybridSans-Regular.ttf
sub IBMPlexSans-SemiBold.ttf --unicodes="$LATIN" HybridSans-SemiBold.ttf
sub IBMPlexMono-Regular.ttf --unicodes="$MONO" HybridMono-Regular.ttf
sub IBMPlexSansJP-Regular.ttf --text="$JP_LABELS" HybridLabelsJP-Regular.ttf
python3 - "$tmp" "$here" <<'PY'
import sys
from fontTools.ttLib import TTFont
tmp, out = sys.argv[1], sys.argv[2]
for file, family, style in [
    ("HybridDisplay-SemiBold.ttf", "Hybrid Display", "SemiBold"),
    ("HybridSans-Regular.ttf", "Hybrid Sans", "Regular"),
    ("HybridSans-SemiBold.ttf", "Hybrid Sans", "SemiBold"),
    ("HybridMono-Regular.ttf", "Hybrid Mono", "Regular"),
    ("HybridLabelsJP-Regular.ttf", "Hybrid Labels JP", "Regular"),
]:
    f = TTFont(f"{tmp}/sub-{file}")
    name = f["name"]
    ps = family.replace(" ", "") + "-" + style
    ribbi = style == "Regular"
    for rec in list(name.names):
        if rec.nameID in (1, 2, 3, 4, 6, 16, 17, 21, 22):
            name.removeNames(nameID=rec.nameID)
    name.setName(family if ribbi else f"{family} {style}", 1, 3, 1, 0x409)
    name.setName("Regular", 2, 3, 1, 0x409)
    name.setName(f"{ps};synaps-desktop subset", 3, 3, 1, 0x409)
    name.setName(f"{family} {style}", 4, 3, 1, 0x409)
    name.setName(ps, 6, 3, 1, 0x409)
    name.setName(family, 16, 3, 1, 0x409)
    name.setName(style, 17, 3, 1, 0x409)
    f.save(f"{out}/{file}")
    print(file, family, style)
PY
ls -l "$here"/*.ttf
