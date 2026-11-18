#!/bin/bash
# Q5 r3: the series on bella, one after another (the PR head's release build).
cd ~/.cache/q5r3
bash chipclip.sh > clips.log 2>&1
for lv in full reduced off; do bash clip.sh $lv; done >> clips.log 2>&1
bash clip.sh full 1920x1080 >> clips.log 2>&1
echo CLIPS-DONE >> clips.log
