# boost-bench.sh: the three benchmarks with the GPU's and CPUs' minimum clocks at
# their maximum; the old minimums back after, whatever happens.
G=/sys/class/kgsl/kgsl-3d0/devfreq
OLD_G=$(cat $G/min_freq)
OLD_C=""
for p in /sys/devices/system/cpu/cpufreq/policy*; do OLD_C="$OLD_C $p:$(cat $p/scaling_min_freq)"; done
restore() {
    echo $OLD_G > $G/min_freq
    for pv in $OLD_C; do echo ${pv#*:} > ${pv%:*}/scaling_min_freq; done
    echo "restored: gpu min $(cat $G/min_freq), cpu min $(cat /sys/devices/system/cpu/cpufreq/policy*/scaling_min_freq | tr '\n' ' ')"
}
trap restore EXIT
echo "before: gpu min $OLD_G, cpu$OLD_C"
cat $G/max_freq > $G/min_freq
for p in /sys/devices/system/cpu/cpufreq/policy*; do cat $p/cpuinfo_max_freq > $p/scaling_min_freq; done
echo "boosted: gpu min $(cat $G/min_freq), cpu min $(cat /sys/devices/system/cpu/cpufreq/policy*/scaling_min_freq | tr '\n' ' ')"
DRIVER=scroll sh /tmp/bench.sh 30 > /tmp/boost-scroll.out 2>&1
sh /tmp/bench.sh 30 > /tmp/boost-tap.out 2>&1
DRIVER=swipe sh /tmp/bench.sh 40 > /tmp/boost-swipe.out 2>&1
echo "gpu now $(cat $G/cur_freq)"
