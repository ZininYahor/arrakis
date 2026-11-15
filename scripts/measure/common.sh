# What scripts/measure.sh and the scripts beside this file share. Sourced, never run.
#
# The caller defines `fail`, which prints its own name and exits.

# Refuses while another Arrakis runs: two copies fight over the gamma tables. The scripts that start
# the real app give it its own TMPDIR, which hides `fastframe_instance`'s slot from a copy the owner
# has running, so this check is the only guard. `pgrep` comes from PATH, so a test can stand one in.
refuse_if_running() {
    others=$(pgrep -i -x arrakis 2>/dev/null || true)
    if [ -n "$others" ]; then
        fail "another arrakis is running (pid $(echo $others)); quit it first"
    fi
}
