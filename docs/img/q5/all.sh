#!/bin/bash
# The whole series, one after another (nothing of ours overlaps a bench run).
cd ~/.cache/q5
bash ev.sh > ev.log 2>&1
for lv in full reduced off; do bash clip.sh $lv; done > clips.log 2>&1
bash bench.sh > bench.out 2>&1   # docs/bench/q5/bench.sh
echo ALL-DONE >> clips.log
