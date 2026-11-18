#!/bin/bash
cd ~/.cache/q6
bash ev.sh > ev.log 2>&1
for lv in full reduced off; do bash clip.sh $lv; done > clips.log 2>&1
bash sheets.sh > sheets.log 2>&1
bash idlepal.sh > idle.out 2>&1
bash bench.sh > bench.out 2>&1
echo ALL-DONE >> clips.log
