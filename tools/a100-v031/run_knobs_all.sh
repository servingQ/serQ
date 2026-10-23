#!/bin/bash
# Every model's engine settings, after run_all.sh. KNOBS_ALL_DONE only when
# every run_knobs.sh succeeded, else KNOBS_ALL_FAILED and exit 1.
cd ~/g
. ./common.sh
rc=0
for m in "${MODELS[@]}"; do bash run_knobs.sh "${m%%:*}" "${m#*:}" || rc=1; done
if [ $rc -eq 0 ]; then echo KNOBS_ALL_DONE; else echo KNOBS_ALL_FAILED; fi
exit $rc
