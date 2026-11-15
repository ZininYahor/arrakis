// Prints one process's counters from proc_pid_rusage: CPU time, the P cores' share of it, instructions and energy.
// Needs no root for a process of the same user. It holds the CPU only: not the GPU, not the window server.
// energy-drag.py reads it twice, 20 s apart, inside a drive, and takes the difference.
// usage: energytool <pid>
// Build it into $TMPDIR, where energy-drag.py looks for it:
//   clang -O1 energytool.c -o "$TMPDIR/energytool"
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
    printf("cpu_ns=%.0f p_cpu_ns=%.0f instr=%llu p_instr=%llu cycles=%llu p_cycles=%llu energy_nj=%llu p_energy_nj=%llu\n",
           (r.ri_user_time + r.ri_system_time) * scale, (r.ri_user_ptime + r.ri_system_ptime) * scale, r.ri_instructions,
           r.ri_pinstructions, r.ri_cycles, r.ri_pcycles, r.ri_energy_nj, r.ri_penergy_nj);
    return 0;
}
