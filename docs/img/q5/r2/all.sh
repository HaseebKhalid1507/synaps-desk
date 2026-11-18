#!/bin/bash
# Q5 r2: the whole series on bella, one after another (nothing of ours overlaps a bench run).
cd ~/.cache/q5r2
bash chips.sh > chips.log 2>&1
bash chipclip.sh > clips.log 2>&1
bash composer.sh >> clips.log 2>&1
for lv in full reduced off; do bash clip.sh $lv; done >> clips.log 2>&1
echo CLIPS-DONE >> clips.log
bash bench.sh > bench.out 2>&1
