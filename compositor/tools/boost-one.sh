# boost-one.sh gpu|cpu|bigcpu: the scroll benchmark with one set of minimum
# clocks at the maximum; the old minimums back after, whatever happens.
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
case $1 in
    gpu) cat $G/max_freq > $G/min_freq ;;
    gpu[0-9]*) echo ${1#gpu}000000 > $G/min_freq ;;
    cpu) for p in /sys/devices/system/cpu/cpufreq/policy*; do cat $p/cpuinfo_max_freq > $p/scaling_min_freq; done ;;
    little) cat /sys/devices/system/cpu/cpufreq/policy0/cpuinfo_max_freq > /sys/devices/system/cpu/cpufreq/policy0/scaling_min_freq ;;
esac
DRIVER=scroll sh /tmp/bench.sh 30 > /tmp/boost-$1.out 2>&1
