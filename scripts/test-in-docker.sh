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

echo ""
echo "=== Verifying data rescue without puddle ==="
# 全て停止
vgchange -an puddle-pool
mdadm --stop --scan

# puddle なしで再組み立て
mdadm --assemble --scan
vgscan 2>/dev/null
vgchange -ay puddle-pool
mount /dev/mapper/puddle--pool-data /mnt/pool
cat /mnt/pool/hello.txt
echo "PASS: data rescue without puddle OK"
umount /mnt/pool

echo ""
echo "========================================="
echo "  ALL TESTS PASSED"
echo "========================================="
'
