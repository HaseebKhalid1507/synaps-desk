#!/bin/bash
cd ~/.cache/q6r3
bash designs.sh > designs.out 2>&1
bash clip.sh > clip.out 2>&1
bash live.sh > live-run.out 2>&1
echo ALL-DONE >> designs.out
