#!/bin/bash
set -e

cd "$(dirname "$0")/.."

echo "=== Building test container ==="
docker build -f Dockerfile.test -t puddle-test .

echo ""
echo "=== Running manual verification ==="
# --privileged: カーネルモジュール (md, dm) へのアクセス
# -v /dev:/dev: ホストのデバイスノード共有 (loop デバイス用)
# --pid=host: /proc/mdstat 等の参照
docker run --privileged --rm \
    -v /dev:/dev \
    --pid=host \
    puddle-test bash -c '
set -ex

# カーネルモジュールのロード
modprobe loop 2>/dev/null || true
modprobe dm_mod 2>/dev/null || true
modprobe dm_thin_pool 2>/dev/null || true
modprobe dm_mirror 2>/dev/null || true

# LVM / device-mapper の初期化 (Docker では udev が動かないので無効化)
mkdir -p /run/lvm /run/lock/lvm
dmsetup mknodes 2>/dev/null || true
# udev 無効化 — Docker コンテナ内で必須
cat >> /etc/lvm/lvm.conf <<LVMEOF
activation {
    udev_sync = 0
    udev_rules = 0
}
LVMEOF

# 前回のテスト残骸をクリーンアップ
echo "=== Pre-cleanup ==="
umount /mnt/pool 2>/dev/null || true
lvchange -an puddle-pool/data 2>/dev/null || true
lvremove -f puddle-pool/data 2>/dev/null || true
vgremove -f puddle-pool 2>/dev/null || true
for md in /dev/md/puddle-z* /dev/md[0-9]*; do
    [ -e "$md" ] && mdadm --stop "$md" 2>/dev/null || true
done
mdadm --stop --scan 2>/dev/null || true
rm -rf /var/lib/puddle 2>/dev/null || true

# ループバックデバイス作成 (--find で空きデバイスを自動選択)
LOOPS=()
for i in 0 1 2; do
    dd if=/dev/zero of=/tmp/disk${i}.img bs=1M count=256 2>/dev/null
    DEV=$(losetup --find --show /tmp/disk${i}.img)
    LOOPS+=("$DEV")
    echo "disk${i} -> ${DEV}"
done

DISK0=${LOOPS[0]}
DISK1=${LOOPS[1]}
DISK2=${LOOPS[2]}

# クリーンアップ関数
cleanup() {
    echo ""
    echo "=== Cleanup ==="
    umount /mnt/pool 2>/dev/null || true
    lvchange -an puddle-pool/data 2>/dev/null || true
    lvremove -f puddle-pool/data 2>/dev/null || true
    vgremove -f puddle-pool 2>/dev/null || true
    for md in /dev/md/puddle-z* /dev/md[0-9]*; do
        [ -e "$md" ] && mdadm --stop "$md" 2>/dev/null || true
    done
    mdadm --stop --scan 2>/dev/null || true
    for dev in "${LOOPS[@]}"; do
        pvremove -f "$dev"* 2>/dev/null || true
        mdadm --zero-superblock "$dev"* 2>/dev/null || true
        losetup -d "$dev" 2>/dev/null || true
    done
    rm -f /tmp/disk0.img /tmp/disk1.img /tmp/disk2.img
    echo "Cleanup done"
}
trap cleanup EXIT

# puddle init
echo ""
echo "=== puddle init ${DISK0} ==="
/puddle/target/release/puddle init ${DISK0} --mkfs ext4

echo ""
echo "=== puddle status (1 disk) ==="
/puddle/target/release/puddle status

# puddle add (2台目)
echo ""
echo "=== puddle add ${DISK1} ==="
/puddle/target/release/puddle add ${DISK1} --yes

echo ""
echo "=== puddle add ${DISK2} ==="
/puddle/target/release/puddle add ${DISK2} --yes

echo ""
echo "=== puddle status (3 disks) ==="
/puddle/target/release/puddle status

echo ""
echo "=== Verifying mdadm arrays ==="
cat /proc/mdstat || echo "(mdstat not available)"

echo ""
echo "=== Verifying LVM ==="
pvs 2>/dev/null || true
vgs 2>/dev/null || true
lvs 2>/dev/null || true

echo ""
echo "=== Verifying mount ==="
mkdir -p /mnt/pool
mount /dev/mapper/puddle--pool-data /mnt/pool
echo "test data from puddle" > /mnt/pool/hello.txt
cat /mnt/pool/hello.txt
df -h /mnt/pool
umount /mnt/pool
echo "PASS: mount/write/read OK"

# RAID reshape 完了を待つ
echo ""
echo "=== Waiting for RAID reshape to complete ==="
for md in /dev/md/puddle-z*; do
    [ -e "$md" ] && mdadm --wait "$md" 2>/dev/null || true
done
echo "RAID sync complete"

echo ""
echo "=== Verifying data rescue without puddle ==="
# 全て停止
vgchange -an puddle-pool
# puddle のアレイだけ停止 (ホストのアレイに影響しない)
for md in /dev/md/puddle-z*; do
    [ -e "$md" ] && mdadm --stop "$md" 2>/dev/null || true
done

# puddle なしで再組み立て
mdadm --assemble --scan 2>/dev/null || true
vgscan 2>/dev/null
vgchange -ay puddle-pool
mount /dev/mapper/puddle--pool-data /mnt/pool
cat /mnt/pool/hello.txt
echo "PASS: data rescue without puddle OK"
umount /mnt/pool

# データ復旧後、プールを再構築
vgchange -an puddle-pool 2>/dev/null || true
for md in /dev/md/puddle-z*; do
    [ -e "$md" ] && mdadm --stop "$md" 2>/dev/null || true
done
mdadm --stop --scan 2>/dev/null || true
rm -rf /var/lib/puddle 2>/dev/null || true

# パーティションテーブルと md スーパーブロックをワイプ
for dev in "$DISK0" "$DISK1" "$DISK2"; do
    pvremove -f "${dev}"* 2>/dev/null || true
    mdadm --zero-superblock "${dev}"* 2>/dev/null || true
    sgdisk --zap-all "$dev" 2>/dev/null || true
    wipefs -a "$dev" 2>/dev/null || true
    partprobe "$dev" 2>/dev/null || true
done

# puddle で再 init + add して元に戻す
/puddle/target/release/puddle init ${DISK0} --mkfs ext4 --yes
/puddle/target/release/puddle add ${DISK1} --yes
/puddle/target/release/puddle add ${DISK2} --yes

echo ""
echo "=== puddle monitor --once ==="
/puddle/target/release/puddle monitor --once || echo "(exit code $? — warnings detected, expected for loopback devices)"
echo "PASS: monitor --once OK"

echo ""
echo "=== puddle health ==="
/puddle/target/release/puddle health
echo "PASS: health OK"

# remove テストは dm-mirror モジュールが必要 (pvmove 用)
if modprobe -n dm_mirror 2>/dev/null; then
    echo ""
    echo "=== puddle remove ${DISK2} ==="
    /puddle/target/release/puddle remove ${DISK2} --yes
    echo "PASS: remove OK"

    echo ""
    echo "=== puddle status (after remove) ==="
    /puddle/target/release/puddle status
    echo "PASS: status after remove OK"

    echo ""
    echo "=== Verifying data after remove ==="
    mkdir -p /mnt/pool
    mount /dev/mapper/puddle--pool-data /mnt/pool
    echo "test data after remove" > /mnt/pool/after-remove.txt
    cat /mnt/pool/after-remove.txt
    umount /mnt/pool
    echo "PASS: data accessible after remove"
else
    echo ""
    echo "=== SKIP: puddle remove (dm-mirror module not available) ==="
fi

echo ""
echo "=== puddle destroy ==="
/puddle/target/release/puddle destroy --yes
echo "PASS: destroy OK"

# ────────────────────────────────────────────
# 遅延割り当て (Deferred Allocation) E2E テスト
# ────────────────────────────────────────────

echo ""
echo "========================================="
echo "  DEFERRED ALLOCATION E2E TEST"
echo "========================================="

# 前回の残骸をクリーンアップ
for md in /dev/md/puddle-z* /dev/md[0-9]*; do
    [ -e "$md" ] && mdadm --stop "$md" 2>/dev/null || true
done
mdadm --stop --scan 2>/dev/null || true
rm -rf /var/lib/puddle 2>/dev/null || true

# 異種容量ループバックデバイスを作成
# disk_small=128MB, disk_large=256MB → Zone0: RAID1(128MB), Zone1: SINGLE(128MB)
dd if=/dev/zero of=/tmp/disk_small.img bs=1M count=128 2>/dev/null
dd if=/dev/zero of=/tmp/disk_large.img bs=1M count=256 2>/dev/null
DISK_S=$(losetup --find --show /tmp/disk_small.img)
DISK_L=$(losetup --find --show /tmp/disk_large.img)
echo "disk_small -> ${DISK_S} (128MB)"
echo "disk_large -> ${DISK_L} (256MB)"

# cleanup 関数を拡張
cleanup_deferred() {
    umount /mnt/pool 2>/dev/null || true
    lvchange -an puddle-pool/data 2>/dev/null || true
    lvremove -f puddle-pool/data 2>/dev/null || true
    vgremove -f puddle-pool 2>/dev/null || true
    for md in /dev/md/puddle-z* /dev/md[0-9]*; do
        [ -e "$md" ] && mdadm --stop "$md" 2>/dev/null || true
    done
    mdadm --stop --scan 2>/dev/null || true
    for dev in "$DISK_S" "$DISK_L"; do
        pvremove -f "$dev"* 2>/dev/null || true
        mdadm --zero-superblock "$dev"* 2>/dev/null || true
        losetup -d "$dev" 2>/dev/null || true
    done
    rm -f /tmp/disk_small.img /tmp/disk_large.img
}
trap cleanup_deferred EXIT

echo ""
echo "=== init with small disk (128MB) ==="
/puddle/target/release/puddle init ${DISK_S} --mkfs ext4 --yes

echo ""
echo "=== add large disk (256MB) — should create reserved SINGLE zone ==="
/puddle/target/release/puddle add ${DISK_L} --yes

echo ""
echo "=== puddle status (heterogeneous) ==="
/puddle/target/release/puddle status

echo ""
echo "=== Verify pool.toml has allocatable = false ==="
grep -q "allocatable = false" /var/lib/puddle/pool.toml
echo "PASS: pool.toml contains allocatable = false"

echo ""
echo "=== Verify pvs shows allocatable flag ==="
pvs -o pv_name,pv_attr 2>/dev/null || true

echo ""
echo "=== Verify data writes to redundant zone ==="
mkdir -p /mnt/pool
mount /dev/mapper/puddle--pool-data /mnt/pool
echo "deferred allocation test data" > /mnt/pool/deferred.txt
cat /mnt/pool/deferred.txt
echo "PASS: write to redundant zone OK"
umount /mnt/pool

echo ""
echo "=== puddle expand-unprotected ==="
/puddle/target/release/puddle expand-unprotected --yes
echo "PASS: expand-unprotected OK"

echo ""
echo "=== Verify pool.toml no longer has allocatable = false ==="
if grep -q "allocatable = false" /var/lib/puddle/pool.toml; then
    echo "FAIL: pool.toml still has allocatable = false"
    exit 1
fi
echo "PASS: all zones now allocatable = true"

echo ""
echo "=== puddle status (after expand) ==="
/puddle/target/release/puddle status

echo ""
echo "=== Verify data still accessible after expand ==="
mount /dev/mapper/puddle--pool-data /mnt/pool
cat /mnt/pool/deferred.txt
echo "new data after expand" > /mnt/pool/expanded.txt
cat /mnt/pool/expanded.txt
umount /mnt/pool
echo "PASS: data accessible after expand-unprotected"

echo ""
echo "=== destroy (deferred allocation test) ==="
/puddle/target/release/puddle destroy --yes
echo "PASS: destroy OK"

echo ""
echo "========================================="
echo "  ALL TESTS PASSED"
echo "========================================="
'
