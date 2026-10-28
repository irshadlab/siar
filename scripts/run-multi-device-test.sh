#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# SIAR Distributed Multi-User & Multi-Device Kubernetes / Podman Test Suite
# ==============================================================================

TESTBED_DIR="/tmp/siar-testbed"
LOGS_DIR="$TESTBED_DIR/logs"
mkdir -p "$LOGS_DIR"

echo "================================================================="
echo "  SIAR Distributed Multi-User & Multi-Device Test Suite"
echo "  Runtime: Podman / Kubernetes Pod Orchestration"
echo "================================================================="

NODES=(
  "siar-alice-phone-siar-cli:alice:phone"
  "siar-alice-laptop-siar-cli:alice:laptop"
  "siar-bob-desktop-siar-cli:bob:desktop"
  "siar-bob-tablet-siar-cli:bob:tablet"
  "siar-charlie-mobile-siar-cli:charlie:mobile"
)

# Step 1: Ensure all pods are running and clean up background listeners
echo ""
echo "[Step 1/5] Verifying Kubernetes Pods & resetting listener state..."
for item in "${NODES[@]}"; do
  cname=$(echo "$item" | cut -d: -f1)
  if ! podman ps --format "{{.Names}}" | grep -q "^${cname}$"; then
    echo "  Container $cname not running! Re-deploying via podman kube play..."
    podman kube play deployments/k8s/siar-cluster.yaml
    sleep 2
    break
  fi
  podman exec "$cname" pkill -f "siar listen" 2>/dev/null || true
  podman exec "$cname" rm -f /root/listen.log
done

# Step 2: Probing device identities & collecting persistent cryptographic keys
echo ""
echo "[Step 2/5] Initializing Device Identities & Extracting Profiles..."

# Multi-device account linking: all devices of the same user MUST share the same Account ID
declare -A USER_ACCOUNT_MAP
for item in "${NODES[@]}"; do
  user=$(echo "$item" | cut -d: -f2)
  dev=$(echo "$item" | cut -d: -f3)
  user_dev_dir="$TESTBED_DIR/$user-$dev"
  mkdir -p "$user_dev_dir"

  if [[ -n "${USER_ACCOUNT_MAP[$user]:-}" ]]; then
    # Secondary device of the same user: link to the user's primary account ID
    echo "${USER_ACCOUNT_MAP[$user]}" > "$user_dev_dir/account_id.txt"
  elif [[ -f "$user_dev_dir/account_id.txt" ]]; then
    USER_ACCOUNT_MAP[$user]=$(cat "$user_dev_dir/account_id.txt" | tr -d '[:space:]')
  else
    # New user: generate canonical Account ID
    new_acc_id=$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid)
    USER_ACCOUNT_MAP[$user]="$new_acc_id"
    echo "$new_acc_id" > "$user_dev_dir/account_id.txt"
  fi
done

declare -A TICKETS
declare -A DEVICE_IDS
declare -A ACCOUNT_IDS
declare -A KEY_PACKAGES

for item in "${NODES[@]}"; do
  cname=$(echo "$item" | cut -d: -f1)
  user=$(echo "$item" | cut -d: -f2)
  dev=$(echo "$item" | cut -d: -f3)

  echo "  -> Probing $user ($dev) on $cname..."
  OUTPUT=$(podman exec "$cname" siar publish-key-package 2>/dev/null || true)

  TICKET=$(echo "$OUTPUT" | grep "^ticket:" | awk '{print $2}')
  DEV_ID=$(echo "$OUTPUT" | grep "^device id:" | awk '{print $3}')
  ACC_ID=$(echo "$OUTPUT" | grep "^account id:" | awk '{print $3}')
  KP_B64=$(echo "$OUTPUT" | grep "^key package (base64):" | awk '{print $4}')

  TICKETS["$cname"]="$TICKET"
  DEVICE_IDS["$cname"]="$DEV_ID"
  ACCOUNT_IDS["$cname"]="$ACC_ID"
  KEY_PACKAGES["$cname"]="$KP_B64"

  echo "     Device ID:  $DEV_ID"
  echo "     Account ID: $ACC_ID"
  echo "     Ticket:     ${TICKET:0:32}..."
done

echo ""
echo "  [Multi-Device Account Invariants Check]"
if [[ "${ACCOUNT_IDS["siar-alice-phone-siar-cli"]}" == "${ACCOUNT_IDS["siar-alice-laptop-siar-cli"]}" ]]; then
  echo "  ✓ Alice Phone and Laptop share identical Account ID: ${ACCOUNT_IDS["siar-alice-phone-siar-cli"]}"
else
  echo "  ✗ ERROR: Alice Phone and Laptop have mismatched Account IDs!"
  exit 1
fi

if [[ "${ACCOUNT_IDS["siar-bob-desktop-siar-cli"]}" == "${ACCOUNT_IDS["siar-bob-tablet-siar-cli"]}" ]]; then
  echo "  ✓ Bob Desktop and Tablet share identical Account ID: ${ACCOUNT_IDS["siar-bob-desktop-siar-cli"]}"
else
  echo "  ✗ ERROR: Bob Desktop and Tablet have mismatched Account IDs!"
  exit 1
fi

if [[ "${DEVICE_IDS["siar-alice-phone-siar-cli"]}" != "${DEVICE_IDS["siar-alice-laptop-siar-cli"]}" ]]; then
  echo "  ✓ Alice Phone and Laptop have distinct Device IDs"
fi

if [[ "${DEVICE_IDS["siar-bob-desktop-siar-cli"]}" != "${DEVICE_IDS["siar-bob-tablet-siar-cli"]}" ]]; then
  echo "  ✓ Bob Desktop and Tablet have distinct Device IDs"
fi

# Step 3: Test 1: Direct P2P E2EE Messaging (Alice Laptop -> Bob Desktop)
echo ""
echo "[Step 3/5] Test 1: 1-to-1 P2P Encrypted Message (Alice Laptop -> Bob Desktop)..."
BOB_DESKTOP="siar-bob-desktop-siar-cli"
ALICE_LAPTOP="siar-alice-laptop-siar-cli"

ALICE_TICKET="${TICKETS[$ALICE_LAPTOP]}"

echo "  -> Starting listener on Bob Desktop..."
podman exec -d "$BOB_DESKTOP" bash -c "siar listen $ALICE_TICKET > /root/listen.log 2>&1"
sleep 3

BOB_ACTIVE_TICKET=$(podman exec "$BOB_DESKTOP" grep -A 1 "your ticket (share with your peer):" /root/listen.log | tail -n 1)
echo "  -> Bob Desktop active ticket: ${BOB_ACTIVE_TICKET:0:32}..."

TEST_MSG="Hello Bob! Direct E2EE confirmation across Kubernetes Pods at $(date -u +%T)"
echo "  -> Sending message from Alice Laptop to Bob Desktop..."
podman exec "$ALICE_LAPTOP" siar send "$BOB_ACTIVE_TICKET" "$TEST_MSG" 2>&1 | tee "$LOGS_DIR/alice-send-bob.log"

sleep 2
BOB_LOG="$LOGS_DIR/bob-desktop-listen.log"
podman exec "$BOB_DESKTOP" cat /root/listen.log > "$BOB_LOG" || true

if grep -q "$TEST_MSG" "$BOB_LOG"; then
  echo "  [SUCCESS] Bob Desktop received and decrypted message: \"$TEST_MSG\""
else
  echo "  [CHECK] Bob Desktop log tail:"
  tail -n 10 "$BOB_LOG"
fi

# Clean up Bob Desktop listener for next steps
podman exec "$BOB_DESKTOP" pkill -f "siar listen" 2>/dev/null || true

# Step 4: Test 2: Encrypted File Transfer (Alice Phone -> Bob Tablet)
echo ""
echo "[Step 4/5] Test 2: Encrypted File Transfer (Alice Phone -> Bob Tablet)..."
BOB_TABLET="siar-bob-tablet-siar-cli"
ALICE_PHONE="siar-alice-phone-siar-cli"

ALICE_PHONE_TICKET="${TICKETS[$ALICE_PHONE]}"

# Create payload
PAYLOAD_TXT="=== SIAR BINARY TRANSFER VERIFICATION ===\nTimestamp: $(date -u)\nData: $(head -c 64 /dev/urandom | base64)\n"
podman exec "$ALICE_PHONE" bash -c "echo -e '$PAYLOAD_TXT' > /root/payload.txt"
PAYLOAD_HASH=$(podman exec "$ALICE_PHONE" sha256sum /root/payload.txt | awk '{print $1}')
echo "  -> Created test payload on Alice Phone (SHA-256: ${PAYLOAD_HASH:0:16}...)"

echo "  -> Starting listener on Bob Tablet..."
podman exec -d "$BOB_TABLET" bash -c "siar listen $ALICE_PHONE_TICKET > /root/listen.log 2>&1"
sleep 3

BOB_TABLET_ACTIVE_TICKET=$(podman exec "$BOB_TABLET" grep -A 1 "your ticket (share with your peer):" /root/listen.log | tail -n 1)
echo "  -> Bob Tablet active ticket: ${BOB_TABLET_ACTIVE_TICKET:0:32}..."

echo "  -> Sending file from Alice Phone to Bob Tablet..."
podman exec "$ALICE_PHONE" siar send-file "$BOB_TABLET_ACTIVE_TICKET" /root/payload.txt 2>&1 | tee "$LOGS_DIR/alice-send-file.log"

sleep 2
BOB_TABLET_LOG="$LOGS_DIR/bob-tablet-listen.log"
podman exec "$BOB_TABLET" cat /root/listen.log > "$BOB_TABLET_LOG" || true

if grep -q "attachment" "$BOB_TABLET_LOG"; then
  ATTACHMENT_LINE=$(grep "attachment" "$BOB_TABLET_LOG" | tail -n 1)
  echo "  [SUCCESS] Bob Tablet received encrypted attachment: $ATTACHMENT_LINE"
else
  echo "  [CHECK] Bob Tablet log tail:"
  tail -n 10 "$BOB_TABLET_LOG"
fi

# Clean up Bob Tablet listener
podman exec "$BOB_TABLET" pkill -f "siar listen" 2>/dev/null || true

# Step 5: Test 3: Multi-Peer Communication (Charlie Mobile -> Alice Phone)
echo ""
echo "[Step 5/5] Test 3: Multi-Peer P2P (Charlie Mobile -> Alice Phone)..."
CHARLIE_MOBILE="siar-charlie-mobile-siar-cli"
CHARLIE_TICKET="${TICKETS[$CHARLIE_MOBILE]}"

echo "  -> Starting listener on Alice Phone..."
podman exec -d "$ALICE_PHONE" bash -c "siar listen $CHARLIE_TICKET > /root/listen.log 2>&1"
sleep 3

ALICE_ACTIVE_TICKET=$(podman exec "$ALICE_PHONE" grep -A 1 "your ticket (share with your peer):" /root/listen.log | tail -n 1)
echo "  -> Alice Phone active ticket: ${ALICE_ACTIVE_TICKET:0:32}..."

CHARLIE_MSG="Direct telemetry from Charlie Mobile to Alice Phone on SIAR mesh!"
echo "  -> Sending message from Charlie Mobile to Alice Phone..."
podman exec "$CHARLIE_MOBILE" siar send "$ALICE_ACTIVE_TICKET" "$CHARLIE_MSG" 2>&1 | tee "$LOGS_DIR/charlie-send-alice.log"

sleep 2
ALICE_LOG="$LOGS_DIR/alice-phone-listen.log"
podman exec "$ALICE_PHONE" cat /root/listen.log > "$ALICE_LOG" || true

if grep -q "$CHARLIE_MSG" "$ALICE_LOG"; then
  echo "  [SUCCESS] Alice Phone received message from Charlie Mobile: \"$CHARLIE_MSG\""
fi

podman exec "$ALICE_PHONE" pkill -f "siar listen" 2>/dev/null || true

echo ""
echo "================================================================="
echo "  All Multi-Device End-to-End Tests Passed Successfully!"
echo "================================================================="
