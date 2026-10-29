#!/bin/bash
# Live check of Part 16: config validation, upgrade handoff (--replace),
# instance file, SIGHUP reload, metrics, audit, clean shutdown.
D=${SIAR_DAEMON:-target/debug/siar-daemon}
R=/tmp/siar-up; rm -rf $R; mkdir -p $R/data $R/run $R/cfg; chmod 700 $R/run
E="env RUST_BACKTRACE=0 XDG_DATA_HOME=$R/data XDG_RUNTIME_DIR=$R/run XDG_CONFIG_HOME=$R/cfg"
mkdir -p $R/cfg/siar
echo "== 1. invalid config is refused with every problem listed"
printf '[ipc]\nmax_connections = 0\n[logging]\nlevel = "loud"\n' > $R/cfg/siar/daemon.toml
$E $D run 2>&1 | grep -i "problem" | head -2
echo "== 2. embedded secret / typo is refused"
printf 'password = "hunter2"\n' > $R/cfg/siar/daemon.toml
$E $D run 2>&1 | grep -ci "unknown field"
printf '' > $R/cfg/siar/daemon.toml
echo "== 3. start A"
$E $D run > $R/a.log 2>&1 & PA=$!
for i in $(seq 1 100); do [ -S $R/run/siar/daemon.sock ] && break; sleep 0.2; done
$E $D ctl status | head -1
echo "instance file: $(grep -c build $R/run/siar/instance.toml) build line(s)"
$E $D ctl send 00000000-0000-0000-0000-000000000000 hi >/dev/null 2>&1
echo "== 4. metrics"; $E $D ctl metrics | grep -E "^siar_(ipc_requests|commands)_total"
echo "== 5. audit"; $E $D ctl audit
echo "== 6. start B with --replace: A must stop gracefully, B takes over"
$E $D run --replace > $R/b.log 2>&1 & PB=$!
wait $PA; echo "A exit=$?"
for i in $(seq 1 100); do [ -S $R/run/siar/daemon.sock ] && $E $D ctl status >/dev/null 2>&1 && break; sleep 0.2; done
$E $D ctl status | head -1
grep -E "previous daemon stopped|taking over" $R/b.log | head -1 | cut -c1-120
echo "== 7. SIGHUP reload: power is hot-applied, ipc needs a restart"
printf '[power]\nallow_metered_bulk = true\n[ipc]\nmax_connections = 64\n' > $R/cfg/siar/daemon.toml
kill -HUP $PB; sleep 1
grep "configuration reloaded" $R/b.log | sed 's/.*configuration reloaded//' | head -1
echo "== 8. bad reload is rejected, daemon keeps running"
printf '[shutdown]\ngrace_secs = 0\n' > $R/cfg/siar/daemon.toml
kill -HUP $PB; sleep 1
grep -c "reload rejected" $R/b.log
$E $D ctl status | head -1
echo "== 9. graceful stop cleans up"
$E $D ctl shutdown >/dev/null; wait $PB; echo "B exit=$?"
echo "left behind: $(ls $R/run/siar 2>/dev/null | wc -l) file(s)"
