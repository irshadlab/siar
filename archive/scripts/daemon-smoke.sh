#!/bin/bash
D=${SIAR_DAEMON:-target/debug/siar-daemon}
rm -rf /tmp/sa /tmp/sb; mkdir -p /tmp/sa/data /tmp/sa/run /tmp/sb/data /tmp/sb/run; chmod 700 /tmp/sa/run /tmp/sb/run
SA="env RUST_BACKTRACE=0 XDG_DATA_HOME=/tmp/sa/data XDG_RUNTIME_DIR=/tmp/sa/run"
SB="env RUST_BACKTRACE=0 XDG_DATA_HOME=/tmp/sb/data XDG_RUNTIME_DIR=/tmp/sb/run"
$SA $D run > /tmp/sa/log 2>&1 & PA=$!
$SB $D run > /tmp/sb/log 2>&1 & PB=$!
for i in $(seq 1 100); do [ -S /tmp/sa/run/siar/daemon.sock ] && [ -S /tmp/sb/run/siar/daemon.sock ] && break; sleep 0.2; done
TA=$($SA $D ctl ticket); TB=$($SB $D ctl ticket)
$SA $D ctl pair "$TB"; $SB $D ctl pair "$TA"; sleep 2
CA=$($SA $D ctl conversations | head -1 | cut -f1); CB=$($SB $D ctl conversations | head -1 | cut -f1)
echo "A conv: $CA"; echo "B conv: $CB"
$SA $D ctl send $CA "hello from daemon A"
sleep 5
echo "== B reads its conversation"; $SB $D ctl messages $CB
echo "== B replies"; $SB $D ctl send $CB "ack from daemon B"; sleep 5
echo "== A reads"; $SA $D ctl messages $CA
$SA $D ctl shutdown >/dev/null; $SB $D ctl shutdown >/dev/null
wait $PA; echo "A exit=$?"; wait $PB; echo "B exit=$?"
echo "sockets left: $(ls /tmp/sa/run/siar /tmp/sb/run/siar | grep -c sock)"
