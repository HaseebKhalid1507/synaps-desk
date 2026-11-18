#!/bin/bash
# Q5 r4: the series on bella, one after another (the PR head's release build).
cd ~/.cache/q5r4
for lv in full reduced off; do bash clip.sh $lv; done > clips.log 2>&1
for lv in full reduced off; do bash r2drive.sh $lv; done >> clips.log 2>&1
echo CLIPS-DONE >> clips.log
