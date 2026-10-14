#!/bin/sh
# Samples LWSpot's CPU and memory while it plays, for the README's "How
# light is it?" table on a Mac.
#
#   packaging/macos/measure.sh [seconds]     (default 60)
#
# CPU is the average share of one core over the run, from the process's
# own CPU time (so short spikes and idle gaps both count). Memory is the
# resident set and the physical footprint (macOS's equivalent of private
# memory), sampled every second; the table reports the median.
set -eu

SECONDS_TO_RUN=${1:-60}
PID=$(pgrep -x lwspot | head -n 1 || true)
[ -n "$PID" ] || { echo "LWSpot is not running"; exit 1; }

cpu_seconds() {
    # cputime as [[dd-]hh:]mm:ss.cc
    ps -o cputime= -p "$PID" | awk -F'[:-]' '{
        n = NF; s = $n + 60 * $(n-1);
        if (n >= 3) s += 3600 * $(n-2);
        if (n >= 4) s += 86400 * $(n-3);
        print s }'
}

footprint_mb() {
    footprint "$PID" 2>/dev/null | awk '/phys_footprint:/ { v=$2; u=$3;
        if (u ~ /^K/) v /= 1024; else if (u ~ /^G/) v *= 1024; printf "%.0f", v; exit }'
}

start=$(cpu_seconds)
rss_samples=""
fp_samples=""
i=0
while [ "$i" -lt "$SECONDS_TO_RUN" ]; do
    rss_samples="$rss_samples $(ps -o rss= -p "$PID" | awk '{ printf "%.0f", $1 / 1024 }')"
    fp_samples="$fp_samples $(footprint_mb)"
    sleep 1
    i=$((i + 1))
done
end=$(cpu_seconds)

median() { printf '%s\n' $1 | grep -v '^$' | sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }'; }

echo "LWSpot (pid $PID) over ${SECONDS_TO_RUN}s"
awk -v a="$start" -v b="$end" -v t="$SECONDS_TO_RUN" 'BEGIN { printf "  CPU:                %.1f%% of a core\n", 100 * (b - a) / t }'
echo "  Memory (resident):  $(median "$rss_samples") MB"
echo "  Physical footprint: $(median "$fp_samples") MB"
echo "  Threads:            $(ps -M -p "$PID" | tail -n +2 | wc -l | tr -d ' ')"
