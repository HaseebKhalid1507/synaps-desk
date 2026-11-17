#!/usr/bin/env python3
"""Frame by frame through a lossless capture (scripts/capture-linux.sh): what changed from the frame
before (pixels, box), and runs of frames where the text in a band dropped out and came back.

usage: scripts/frame-dips.py <clip.mkv> [--band x0,y0,x1,y1] [--size WxH] [--changes]

A frame's ink is its bright pixels (luma > 150: the text on the dark themes) in the band; a dip
is a run of frames whose ink is under 97 % of the clip's median. Prints the runs and their gaps
(frames at 30 fps); `--changes` lists every frame that differs from the one before.
Exit status 1 when there is a dip, so a capture can be checked in a script.
Needs ffmpeg and numpy.
"""

import argparse
import subprocess
import sys

import numpy as np


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("clip")
    ap.add_argument("--band", default="0,0,0,0", help="x0,y0,x1,y1 (default: the whole frame)")
    ap.add_argument("--size", default="1920x1080")
    ap.add_argument("--changes", action="store_true")
    a = ap.parse_args()
    w, h = (int(v) for v in a.size.split("x"))
    x0, y0, x1, y1 = (int(v) for v in a.band.split(","))
    if x1 <= x0 or y1 <= y0:
        x0, y0, x1, y1 = 0, 0, w, h
    cmd = ["ffmpeg", "-loglevel", "error", "-i", a.clip, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE)
    assert p.stdout is not None
    inks, prev, n = [], None, 0
    while True:
        b = p.stdout.read(w * h * 3)
        if len(b) < w * h * 3:
            break
        f = np.frombuffer(b, np.uint8).reshape(h, w, 3)
        inks.append(int((f[y0:y1, x0:x1].mean(axis=2) > 150).sum()))
        if a.changes and prev is not None:
            d = (f != prev).any(axis=2)
            if d.any():
                ys, xs = np.nonzero(d)
                print(f"frame {n}: {int(d.sum())} px changed, box {xs.min()},{ys.min()}..{xs.max()},{ys.max()}, ink {inks[-1]}")
        prev, n = f, n + 1
    p.wait()
    if not inks:
        print("no frames", file=sys.stderr)
        return 2
    med = float(np.median(inks))
    runs, start = [], None
    for i, v in enumerate(inks + [med]):
        low = v < 0.97 * med
        if low and start is None:
            start = i
        elif not low and start is not None:
            runs.append((start, i - 1))
            start = None
    print(f"{a.clip}: {len(inks)} frames, band {x0},{y0}..{x1},{y1}, median ink {med:.0f}")
    print(f"dips: {len(runs)}" + "".join(f"\n  frames {s}..{e} ({e - s + 1}), ink {min(inks[s:e + 1])}" for s, e in runs))
    if len(runs) > 1:
        print("gaps (frames):", [runs[k + 1][0] - runs[k][0] for k in range(len(runs) - 1)])
    return 1 if runs else 0


if __name__ == "__main__":
    sys.exit(main())
