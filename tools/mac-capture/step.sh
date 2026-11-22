#!/bin/sh
# usage: STEPS=steps.txt ./step.sh "IN1 48V on"
echo "$(date +%s.%N 2>/dev/null | grep -v N || python3 -c 'import time;print(f"{time.time():.6f}")') $*" >> ${STEPS:-steps.txt}
