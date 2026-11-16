// Prints one process's counters from one proc_pid_rusage call (RUSAGE_INFO_V6), on one line:
//   cpu_ns      user plus system time, in ns
//   p_cpu_ns    the same, on the P cores only
//   energy_nj   ri_energy_nj: the CPU's energy, in nJ (not the GPU's, not the window server's)
//   idle_wkups  ri_pkg_idle_wkups: the package idle wake-ups
//   intr_wkups  ri_interrupt_wkups: the interrupt wake-ups
//   footprint   ri_phys_footprint, in bytes
// It is energytool.c with the wake-ups and the footprint added, and without the instruction and cycle counts.
// It is a file of its own, and energytool.c stays as it is: other scripts read energytool's line.
// flux-compare.sh logs this line as it prints it, at 30 s and twice 300 s apart; it takes no difference.
// The rest figures (the difference of the first five between the two reads) are worked out from the log.
// Read-only. Needs no root for a process of the same user.
// usage: rusagetool <pid>
// Build it into $TMPDIR, where flux-compare.sh looks for it:
//   clang -O1 rusagetool.c -o "$TMPDIR/rusagetool"
#include <libproc.h>
#include <mach/mach_time.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    struct rusage_info_v6 r;
    if (proc_pid_rusage(atoi(argv[1]), RUSAGE_INFO_V6, (rusage_info_t *)&r) != 0) {
        perror("proc_pid_rusage");
        return 1;
    }
    mach_timebase_info_data_t base;
    mach_timebase_info(&base);
    double scale = (double)base.numer / base.denom;
    printf("cpu_ns=%.0f p_cpu_ns=%.0f energy_nj=%llu idle_wkups=%llu intr_wkups=%llu footprint=%llu\n",
           (r.ri_user_time + r.ri_system_time) * scale, (r.ri_user_ptime + r.ri_system_ptime) * scale,
           r.ri_energy_nj, r.ri_pkg_idle_wkups, r.ri_interrupt_wkups, r.ri_phys_footprint);
    return 0;
}
