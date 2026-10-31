#!/bin/bash
# Two real daemons exchange a message; each daemon's state is then read back
# through the window sync protocol (`ctl ui-state`) — what a window would see.
D=${SIAR_DAEMON:-target/debug/siar-daemon}
R=/tmp/siar-sync; rm -rf $R; mkdir -p $R/a/data $R/a/run $R/b/data $R/b/run; chmod 700 $R/a/run $R/b/run
A="env RUST_BACKTRACE=0 XDG_DATA_HOME=$R/a/data XDG_RUNTIME_DIR=$R/a/run XDG_CONFIG_HOME=$R/a/cfg"
B="env RUST_BACKTRACE=0 XDG_DATA_HOME=$R/b/data XDG_RUNTIME_DIR=$R/b/run XDG_CONFIG_HOME=$R/b/cfg"
$A $D run > $R/a.log 2>&1 & PA=$!
$B $D run > $R/b.log 2>&1 & PB=$!
for i in $(seq 1 100); do [ -S $R/a/run/siar/daemon.sock ] && [ -S $R/b/run/siar/daemon.sock ] && break; sleep 0.2; done
echo "== A before pairing (as a window sees it)"; $A $D ctl ui-state
TA=$($A $D ctl ticket); TB=$($B $D ctl ticket)
$A $D ctl pair "$TB" >/dev/null; $B $D ctl pair "$TA" >/dev/null; sleep 2
CA=$($A $D ctl conversations | head -1 | cut -f1)
$A $D ctl send $CA "hello through the sync protocol" >/dev/null; sleep 4
echo "== A after sending"; $A $D ctl ui-state | sed 's/[0-9a-f]\{8\}-[0-9a-f-]\{27\}/<id>/'
echo "== B after receiving"; $B $D ctl ui-state | sed 's/[0-9a-f]\{8\}-[0-9a-f-]\{27\}/<id>/'
$A $D ctl shutdown >/dev/null; $B $D ctl shutdown >/dev/null; wait $PA; echo "A exit=$?"; wait $PB; echo "B exit=$?"
