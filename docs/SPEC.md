# puddle v2 — 設計書（mdadm + LVM ラッパー方式）

## 1. プロジェクト概要

**puddle** は、Drobo BeyondRAID / Synology SHR の体験をソフトウェアで再現する、
個人向けの軽量ストレージプール管理ツールである。

内部的には Linux の成熟した技術スタック（mdadm, LVM, ファイルシステム）を活用し、
puddle 自身は「異種ディスクの最適なパーティション戦略の自動計算」と
「mdadm/LVM オペレーションの安全な自動実行」に責務を集中する。

### 1.1 設計原則

| 原則 | 内容 |
|------|------|
| **手軽さ最優先** | `puddle add /dev/sdX` だけでディスク追加が完了する |
| **車輪を再発明しない** | パリティ計算・リビルドは mdadm に委ねる |
| **データ救出可能性** | puddle がなくても mdadm + LVM の標準ツールでデータにアクセス可能 |
| **単一ノード** | 1台の Linux マシンで完結 |
| **段階的拡張** | 1台から始めて、あとからディスクを足していける |
| **容量効率優先** | 冗長化できない末尾ゾーンは SINGLE + 警告で容量を活用する (詳細: [DESIGN_NOTES.md](DESIGN_NOTES.md)) |

### 1.2 先行技術との関係

| 技術 | puddle との関係 |
|------|----------------|
| **Synology SHR** | 同一アプローチ（mdadm + LVM）。puddle は SHR のオープン再実装 |
| **OpenHyRAID** | 同一コンセプトの先行 OSS。開発停止状態（星4、7ヶ月未更新） |
| **unRAID** | 最も完成度が高いが、プロプライエタリ |
| **SnapRAID + mergerfs** | ファイルレベル。リアルタイム保護なし |
| **Btrfs RAID5/6** | 不安定な実績 |
| **ZFS RAIDZ Expansion** | 拡張時の容量効率劣化が蓄積する |

---

## 2. アーキテクチャ

```
┌──────────────────────────────────────────────────────┐
│                    ユーザー操作                         │
│                                                        │
│   $ puddle init /dev/sdb                               │
│   $ puddle add /dev/sdc                                │
│   $ puddle add /dev/sdd                                │
│   $ mount /dev/mapper/puddle--pool-data /mnt/pool      │
│                                                        │
└────────────────────────┬─────────────────────────────┘
                         │
┌────────────────────────▼─────────────────────────────┐
│               puddle (Rust バイナリ)                    │
│                                                        │
│  ┌──────────────────────────────────────────────┐     │
│  │           Partition Planner                   │     │
│  │                                               │     │
│  │  全ディスクの容量を分析し、ゾーン分割を計算    │     │
│  │  → 各ゾーンの最適 RAID レベルを決定            │     │
│  │  → パーティションテーブルを生成                │     │
│  └──────────────────┬───────────────────────────┘     │
│                     │                                  │
│  ┌──────────────────▼───────────────────────────┐     │
│  │           Executor                            │     │
│  │                                               │     │
│  │  Partition Planner の出力に基づき:             │     │
│  │  1. sgdisk でパーティション作成               │     │
│  │  2. mdadm で RAID アレイ作成/再構成           │     │
│  │  3. pvcreate / vgextend / lvextend            │     │
│  │  4. resize2fs / xfs_growfs                    │     │
│  └──────────────────────────────────────────────┘     │
│                                                        │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────────┐  │
│  │  Monitor    │  │  State DB   │  │  Notifier    │  │
│  │  (SMART/    │  │  (JSON/     │  │  (mail/      │  │
│  │   udev)     │  │   TOML)     │  │   webhook)   │  │
│  └─────────────┘  └─────────────┘  └──────────────┘  │
└────────────────────────────────────────────────────────┘
                         │
┌────────────────────────▼─────────────────────────────┐
│          Linux カーネル（枯れた技術スタック）            │
│                                                        │
│   mdadm (RAID)  ←→  LVM (ボリューム管理)              │
│                       ↓                                │
│               /dev/mapper/puddle--pool-data             │
│                       ↓                                │
│                ext4 / XFS / Btrfs (single)             │
│                                                        │
└────────────────────────────────────────────────────────┘
```

### 2.1 puddle の責務範囲

puddle が **やること:**

- 異種ディスクの容量分析とゾーン分割計算
- パーティション・RAID・LVM 操作のオーケストレーション
- プール状態の追跡と表示
- SMART 監視と通知
- udev によるディスク着脱検知

puddle が **やらないこと:**

- パリティ計算（mdadm が行う）
- リビルド処理（mdadm が行う）
- ボリューム管理（LVM が行う）
- ファイルシステム管理（ext4/XFS ツールが行う）
- 独自のオンディスクフォーマット（標準ツールだけで読める）

---

## 3. ゾーン分割アルゴリズム — puddle の核心

### 3.1 SHR 方式の原理

異種容量のディスクを効率的に使うため、ディスクを「容量帯（ゾーン）」に分割し、
各ゾーンで独立した RAID を構成する。

```
例: Disk0 = 2TB, Disk1 = 4TB, Disk2 = 4TB

Step 1: ディスクを容量順にソート
  sorted = [2TB, 4TB, 4TB]

Step 2: 容量境界でゾーン分割
  Zone A: 0 〜 2TB   → 3台参加 (Disk0, Disk1, Disk2)
  Zone B: 2TB 〜 4TB → 2台参加 (Disk1, Disk2)

Step 3: 各ゾーンで最適 RAID を構成
  Zone A: RAID5 (3台)  → 実効容量 = 2TB × (3-1) = 4TB
  Zone B: RAID1 (2台)  → 実効容量 = 2TB × 1     = 2TB

Step 4: LVM で結合
  VG: puddle-pool
  ├── PV: /dev/md0 (Zone A の RAID5)  4TB
  └── PV: /dev/md1 (Zone B の RAID1)  2TB
  
  LV: puddle-pool/data  → 実効容量合計 6TB
                           (物理合計 10TB, 冗長オーバーヘッド 4TB)
```

### 3.2 ゾーン分割の計算アルゴリズム

```
入力: disks = [(uuid, capacity), ...] をソート済み
      redundancy = 1 (シングル) or 2 (デュアル)

function compute_zones(disks, redundancy):
    zones = []
    prev_boundary = 0
    
    for i in 0..disks.len():
        boundary = disks[i].capacity
        zone_size = boundary - prev_boundary
        
        if zone_size == 0:
            continue  // 同一容量のディスクが続く場合スキップ
        
        participating_disks = disks.len() - i  // この境界以降に参加するディスク数
        
        raid_level = select_raid_level(participating_disks, redundancy)
        
        zones.push(Zone {
            start: prev_boundary,
            size: zone_size,
            disks: disks[i..],      // 参加ディスク
            raid_level: raid_level,
            effective_size: calc_effective(zone_size, participating_disks, raid_level),
        })
        
        prev_boundary = boundary
    
    return zones


function select_raid_level(num_disks, redundancy):
    if redundancy == 1:
        match num_disks:
            1 => SINGLE (冗長なし、警告)
            2 => RAID1
            3+ => RAID5
    elif redundancy == 2:
        match num_disks:
            1 => SINGLE (警告)
            2 => RAID1 (警告: デュアル冗長不可)
            3 => RAID1 (3台ミラー、警告)
            4+ => RAID6
```

### 3.3 具体例

#### 例1: 均一構成 (4TB × 3)

```
sorted = [4TB, 4TB, 4TB]
Zone A: 0〜4TB, 3台, RAID5 → 実効 8TB
合計: 8TB / 物理 12TB
```

#### 例2: 段階的拡張シナリオ

```
--- 初期: 2TB × 1 ---
Zone A: 0〜2TB, 1台, SINGLE → 実効 2TB (警告: 冗長なし)

--- 2台目追加: 2TB + 4TB ---
Zone A: 0〜2TB, 2台, RAID1 → 実効 2TB
Zone B: 2〜4TB, 1台, SINGLE → 実効 2TB (警告: Zone B 冗長なし)
合計: 4TB / 物理 6TB

--- 3台目追加: 2TB + 4TB + 4TB ---
Zone A: 0〜2TB, 3台, RAID5 → 実効 4TB
Zone B: 2〜4TB, 2台, RAID1 → 実効 2TB
合計: 6TB / 物理 10TB  ← 完全冗長

--- 最小ディスク交換: 2TB→8TB → 4TB + 4TB + 8TB ---
Zone A: 0〜4TB, 3台, RAID5 → 実効 8TB
Zone B: 4〜8TB, 1台, SINGLE → 実効 4TB (警告)
合計: 12TB / 物理 16TB
```

### 3.4 ゾーン再構成（リプラン）

ディスクの追加・削除・交換時にゾーン構成が変わる。
リプランの基本戦略:

```
1. 新しいディスク構成から理想のゾーン構成を計算
2. 現在の構成と diff を取る
3. 差分を最小操作で適用:
   a. 拡張のみの場合:
      - 新ディスクにパーティション作成
      - 既存 mdadm アレイに --add
      - 新ゾーン用の mdadm アレイを新規作成
      - LVM pvcreate + vgextend + lvextend
      - FS リサイズ
   b. 縮小を伴う場合:
      - データ移動が必要 → pvmove でデータ退避
      - mdadm アレイの再構成
      - パーティション再作成
```

---

## 4. ディスクレイアウト

### 4.1 パーティションテーブル (GPT)

各ディスクは GPT でパーティションを切る。puddle のメタデータ用に小さなパーティションを確保する。

```
Disk: /dev/sdb (4TB)

Partition 1:  puddle metadata    16 MB    (GPT type: puddle固有UUID)
Partition 2:  Zone A 用          2 TB     (GPT type: Linux RAID = fd00)
Partition 3:  Zone B 用          残り     (GPT type: Linux RAID = fd00)
```

パーティション作成後のカーネル通知は partprobe → partx --update → blockdev --rereadpt の
順にフォールバックする。全て失敗しても sgdisk 自身がカーネル通知を行うため、
警告を出して続行する（特にループバックデバイスでは BLKRRPART が非対応のため）。

### 4.2 メタデータパーティション (16MB)

各ディスクの先頭に puddle の状態情報を保存する。
フォーマットは TOML。全ディスクに同一内容をレプリケーションする。

```toml
# /dev/sdb1 にマウントされた puddle メタデータ (ext4, 16MB)
# ファイル: /puddle.toml

[pool]
uuid = "a1b2c3d4-e5f6-..."
name = "mypool"
created_at = "2026-03-10T12:00:00Z"
redundancy = 1  # 1 = single, 2 = dual

[[disks]]
uuid = "disk-uuid-0"
device_id = "ata-Samsung_SSD_870_EVO_2TB_S1234"  # by-id
capacity_bytes = 2000000000000
seq = 0
status = "active"  # active / failed / removing

[[disks]]
uuid = "disk-uuid-1"
device_id = "ata-WDC_WD40EFRX_1234"
capacity_bytes = 4000000000000
seq = 1
status = "active"

[[zones]]
index = 0
start_bytes = 0
size_bytes = 2000000000000
raid_level = "raid5"
md_device = "/dev/md/puddle-z0"
participating_disk_uuids = ["disk-uuid-0", "disk-uuid-1", "disk-uuid-2"]

[[zones]]
index = 1
start_bytes = 2000000000000
size_bytes = 2000000000000
raid_level = "raid1"
md_device = "/dev/md/puddle-z1"
participating_disk_uuids = ["disk-uuid-1", "disk-uuid-2"]

[lvm]
vg_name = "puddle-pool"
lv_name = "data"
filesystem = "ext4"
mount_point = "/mnt/pool"

[state]
pool_status = "healthy"  # healthy / degraded / critical
last_scrub = "2026-03-08T03:00:00Z"
version = 2
```

### 4.2.1 ローカル状態ファイル (実装補足)

メタデータはディスク上のパーティションに保存されるが、
CLI コマンド (`puddle add`, `puddle status` 等) が既存プールを
ディスクスキャンなしで素早く見つけるため、ローカルにもコピーを保持する。

```
/var/lib/puddle/pool.toml   … ディスク上メタデータのローカルコピー
/var/lib/puddle/operations.log  … 操作ログ (§7.3)
```

ローカルファイルはあくまでキャッシュであり、正本はディスク上のメタデータ。
ローカルファイルが失われた場合は、いずれかのディスクのメタデータパーティションから復元できる。

### 4.3 データ救出シナリオ

puddle がインストールされていない環境でも、標準ツールでデータにアクセスできる:

```bash
# 1. RAID アレイを検出・組み立て
mdadm --assemble --scan

# 2. LVM を検出
vgscan
vgchange -ay puddle-pool

# 3. マウント
mount /dev/mapper/puddle--pool-data /mnt/recovery

# データにアクセス可能！
ls /mnt/recovery/
```

---

## 5. CLI 設計

### 5.1 コマンド一覧

```bash
# ──── プール作成 ────
$ puddle init /dev/sdb
Pool 'puddle-a1b2c3d4' created.
⚠ WARNING: 1台構成では冗長性がありません。
  ディスクを追加してください: puddle add <device>

# ──── ディスク追加 ────
$ puddle add /dev/sdc
Planning zone layout...

  Current layout:
    Zone 0: SINGLE (1 disk, 2TB) → no redundancy

  New layout:
    Zone 0: RAID1 (2 disks × 2TB) → 1-disk fault tolerance ✓
    Zone 1: SINGLE (1 disk × 2TB) → no redundancy ⚠

  Effective capacity: 2TB → 4TB (+2TB)

Proceed? [Y/n] y
  Creating partitions on /dev/sdc...       done
  Creating RAID array puddle-z0...         done (syncing in background)
  Extending volume group...                done
  Extending logical volume...              done
  Resizing filesystem...                   done
✓ Disk added successfully. RAID sync: 15% (ETA: 3h 20m)

# ──── さらに追加 ────
$ puddle add /dev/sdd
Planning zone layout...

  New layout:
    Zone 0: RAID5 (3 disks × 2TB) → 1-disk fault tolerance ✓
    Zone 1: RAID1 (2 disks × 2TB) → 1-disk fault tolerance ✓

  Effective capacity: 4TB → 6TB (+2TB)

Proceed? [Y/n] y
  ...
✓ Pool is now fully redundant.

# ──── ステータス ────
$ puddle status
Pool: puddle-a1b2c3d4
State: HEALTHY ✓
Redundancy: Single Parity (1-disk fault tolerance)

Disks:
  #0  ata-Samsung_870_EVO_2TB   2.0 TB  [ACTIVE]
  #1  ata-WDC_WD40EFRX          4.0 TB  [ACTIVE]
  #2  ata-WDC_WD40EFRX          4.0 TB  [ACTIVE]

Zones:
  Zone 0  RAID5  3 disks × 2.0 TB  /dev/md/puddle-z0  [clean]
  Zone 1  RAID1  2 disks × 2.0 TB  /dev/md/puddle-z1  [clean]

Capacity:
  Physical:  10.0 TB
  Usable:     6.0 TB
  Used:       1.2 TB (20%)
  Free:       4.8 TB

Mount: /mnt/pool (ext4)

# ──── ディスク交換 ────
$ puddle replace /dev/sdb /dev/sde
  Marking /dev/sdb as failed in RAID arrays...
  Adding /dev/sde to RAID arrays...
  Rebuild started: [████░░░░░░░░] 33%  ETA: 2h 10m

# 容量アップグレード交換
$ puddle upgrade /dev/sdb /dev/sde
  Capacity change detected: 2TB → 8TB
  After rebuild, zone layout will be recalculated.
  Rebuild: [████████░░░░] 67%  ETA: 1h 05m

# ──── ディスク健全性 ────
$ puddle health
SMART Status:
  #0  Samsung 870 EVO 2TB   OK  (Temp: 34°C, Written: 12 TB, Wear: 2%)
  #1  WD Red 4TB            OK  (Temp: 38°C, Reallocated: 0)
  #2  WD Red 4TB            OK  (Temp: 37°C, Reallocated: 0)

RAID Sync:
  Zone 0 (RAID5): clean
  Zone 1 (RAID1): clean

Last Scrub: 2026-03-08 03:00 (clean, 0 mismatches)

# ──── 詳細表示（デバッグ用） ────
$ puddle detail
mdadm arrays:
  /dev/md/puddle-z0  raid5  active  clean  3 devices
  /dev/md/puddle-z1  raid1  active  clean  2 devices

LVM:
  VG: puddle-pool  6.0 TB total, 1.2 TB used
  LV: data         6.0 TB  /dev/mapper/puddle--pool-data

Partitions:
  /dev/sdb1  16 MB   puddle-meta
  /dev/sdb2  2.0 TB  puddle-z0 (RAID member)
  /dev/sdc1  16 MB   puddle-meta
  /dev/sdc2  2.0 TB  puddle-z0 (RAID member)
  /dev/sdc3  2.0 TB  puddle-z1 (RAID member)
  /dev/sdd1  16 MB   puddle-meta
  /dev/sdd2  2.0 TB  puddle-z0 (RAID member)
  /dev/sdd3  2.0 TB  puddle-z1 (RAID member)

# ──── 設定 ────
$ puddle init /dev/sdb --redundancy dual --mkfs ext4
Pool 'puddle-xxxxxxxx' created. (Redundancy: Dual)

$ puddle notify --webhook https://hooks.slack.com/services/...
Notification configured.

$ puddle scrub
Starting RAID scrub on all arrays...
  Zone 0: scrub started
  Zone 1: scrub started
```

### 5.2 マウント管理

puddle は専用のマウントコマンドを持たない。
LVM デバイスが標準的に公開されるので、通常の `mount` / `fstab` で管理する。

```bash
# 初回: ファイルシステム作成
$ puddle init /dev/sdb --mkfs ext4 --mount /mnt/pool

# 以降は fstab に記載
# /etc/fstab:
/dev/mapper/puddle--pool-data  /mnt/pool  ext4  defaults,nofail  0  2
```

---

## 6. デーモン設計 (puddled)

### 6.1 役割

常駐デーモンは最小限の責務のみ持つ:

```
puddled
  ├── udev Monitor        … ディスク着脱を検知して通知
  ├── SMART Poller        … 60秒間隔で smartctl 実行、異常検知
  ├── RAID Status Poller  … /proc/mdstat を監視、degraded 検知
  ├── Scrub Scheduler     … cron 相当の定期スクラブ起動
  └── Notification Sender … 上記イベントを webhook/mail で通知
```

CLI コマンド（`puddle add` 等）はデーモンを経由しない。
直接 mdadm/LVM を操作し、完了後にメタデータを更新する。
デーモンはイベント監視と通知に専念する。

### 6.2 デーモンなしでも動作

puddled は**オプション**。デーモンが停止していても:
- データへのアクセスは正常に継続（mdadm/LVM が動いている限り）
- `puddle add` / `puddle replace` 等の CLI 操作も実行可能
- 失われるのは自動通知と自動検知のみ

### 6.3 systemd ユニット

```ini
# /etc/systemd/system/puddled.service
[Unit]
Description=puddle storage pool monitor
After=local-fs.target mdadm.service lvm2-lvmpolld.service

[Service]
Type=notify
ExecStart=/usr/local/bin/puddled
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

---

## 7. 状態遷移とエラーハンドリング

### 7.1 プール状態

```
                     ┌──────────┐
          初回init → │ HEALTHY  │ ← リビルド完了
                     └────┬─────┘
                          │ ディスク障害 or ゾーン冗長欠如
                     ┌────▼─────┐
                     │ DEGRADED │ ← 1台障害、データアクセス可能
                     └────┬─────┘
                          │ 冗長超過の追加障害
                     ┌────▼─────┐
                     │ CRITICAL │ ← データ損失リスク、警告発報
                     └──────────┘
```

プール状態は mdadm の各アレイ状態から自動判定:

```
HEALTHY  = 全ゾーンの mdadm が clean
DEGRADED = いずれかのゾーンが degraded (だが readable)
CRITICAL = いずれかのゾーンで必要ディスク数を下回った
```

### 7.2 操作の安全性

すべての破壊的操作にはプレビューと確認を必須とする:

```
$ puddle add /dev/sdc

Planning zone layout...
  [差分をプレビュー表示]
Proceed? [Y/n]         ← 明示的確認
```

`--yes` フラグでスキップ可能（スクリプト用途）。

### 7.3 ロールバック戦略

mdadm/LVM 操作は段階的に実行し、各ステップで失敗時のロールバック手順を記録:

```
操作ログ: /var/lib/puddle/operations.log

[2026-03-10T12:34:56] BEGIN add_disk /dev/sdc
[2026-03-10T12:34:57] STEP 1: sgdisk /dev/sdc → OK
                       ROLLBACK: sgdisk --zap-all /dev/sdc
[2026-03-10T12:34:58] STEP 2: mdadm --add /dev/md/puddle-z0 /dev/sdc2 → OK
                       ROLLBACK: mdadm --fail --remove /dev/md/puddle-z0 /dev/sdc2
[2026-03-10T12:34:59] STEP 3: pvcreate /dev/md/puddle-z1 → OK
                       ROLLBACK: pvremove /dev/md/puddle-z1
...
[2026-03-10T12:35:10] COMMIT add_disk /dev/sdc
```

---

## 8. 実装計画

### 8.1 技術スタック

| コンポーネント | 技術 | 理由 |
|---|---|---|
| CLI + デーモン | **Rust** (clap + tokio) | シングルバイナリ配布、型安全 |
| メタデータ | TOML ファイル | 人間可読、デバッグ容易 |
| パーティション操作 | sgdisk 呼び出し | GPT 操作の標準ツール |
| RAID 操作 | mdadm 呼び出し | 20年以上の実績 |
| LVM 操作 | pvcreate/vgcreate/lvcreate 呼び出し | Linux 標準 |
| FS リサイズ | resize2fs / xfs_growfs 呼び出し | 各 FS の標準ツール |
| SMART 監視 | smartctl 呼び出し | 事実上の標準 |
| ディスク検知 | udev (libudev / inotify on /dev) | Linux 標準 |
| 通知 | reqwest (HTTP) / lettre (SMTP) | Rust エコシステム |

### 8.2 ディレクトリ構成

```
puddle/
├── Cargo.toml
├── src/
│   ├── main.rs                 # CLI エントリポイント (clap)
│   ├── daemon.rs               # puddled デーモン
│   │
│   ├── planner/
│   │   ├── mod.rs
│   │   ├── zone.rs             # ゾーン分割アルゴリズム ← 核心
│   │   ├── diff.rs             # 現在構成との差分計算
│   │   └── capacity.rs         # 実効容量計算・表示
│   │
│   ├── executor/
│   │   ├── mod.rs
│   │   ├── partition.rs        # sgdisk ラッパー
│   │   ├── mdadm.rs            # mdadm ラッパー
│   │   ├── lvm.rs              # LVM ラッパー
│   │   ├── filesystem.rs       # mkfs / resize ラッパー
│   │   └── rollback.rs         # 操作ログとロールバック
│   │
│   ├── metadata/
│   │   ├── mod.rs
│   │   ├── pool_config.rs      # TOML シリアライズ/デシリアライズ
│   │   └── sync.rs             # 全ディスクへのメタデータ複製
│   │
│   ├── monitor/
│   │   ├── mod.rs
│   │   ├── smart.rs            # SMART 監視
│   │   ├── udev.rs             # ディスク着脱検知
│   │   ├── mdstat.rs           # /proc/mdstat パーサー
│   │   └── notify.rs           # 通知 (webhook / email)
│   │
│   └── cli/
│       ├── mod.rs
│       ├── init.rs             # puddle init
│       ├── add.rs              # puddle add
│       ├── replace.rs          # puddle replace / upgrade
│       ├── status.rs           # puddle status / health / detail
│       └── notify.rs           # puddle notify
│
├── tests/
│   ├── planner_test.rs         # ゾーン分割の単体テスト
│   ├── integration/
│   │   ├── loopback_test.rs    # ループバックデバイスでの統合テスト
│   │   └── failover_test.rs    # 障害シミュレーション
│   └── fixtures/
│       └── scenarios.toml      # テスト用ディスク構成定義
│
└── docs/
    ├── design.md               # この文書
    └── recovery.md             # puddle なしでのデータ救出手順
```

### 8.3 コード規模見積もり

| モジュール | 推定行数 | 備考 |
|---|---|---|
| planner (zone + diff + capacity) | ~800 | puddle の核心ロジック |
| executor (partition + mdadm + lvm + fs) | ~1200 | コマンド実行ラッパー群 |
| metadata (config + sync) | ~400 | TOML 読み書き |
| monitor (smart + udev + mdstat + notify) | ~600 | イベント監視 |
| cli (全サブコマンド) | ~800 | ユーザーインターフェース |
| daemon | ~300 | イベントループ |
| **合計** | **~4100** | テスト除く |

### 8.4 開発フェーズ

```
Phase 1 — MVP (動作するプール)                  目安: 2〜3週間
───────────────────────────────────────────────
  [ ] planner: ゾーン分割アルゴリズム + 単体テスト
  [ ] executor: sgdisk / mdadm / LVM ラッパー
  [ ] metadata: TOML 読み書き + ディスク間同期
  [ ] cli: init, add, status
  [ ] ループバックデバイスでの統合テスト
  → ゴール: 3台のループバックデバイスでプール作成、
    1台を fail させてもデータが読める。
    puddle なしでも mdadm --assemble + LVM でデータ救出可能。

Phase 2 — 実用化                                目安: 2〜3週間
───────────────────────────────────────────────
  [ ] cli: replace, upgrade, health, detail
  [ ] planner: ディスク追加/交換時のリプラン + diff
  [ ] executor: ロールバック機構
  [ ] monitor: SMART 監視 + /proc/mdstat 監視
  [ ] notify: webhook 通知
  → ゴール: 実ディスクでディスク交換ができる。
    SMART 異常で Slack に通知が飛ぶ。

Phase 3 — 堅牢化                                目安: 2〜3週間
───────────────────────────────────────────────
  [ ] daemon: puddled (udev + SMART + mdstat + scrub scheduler)
  [ ] init --redundancy dual (RAID6 対応)
  [ ] planner: RAID6 対応 (デュアル冗長)
  [ ] 容量計算の表示改善 (Drobo 風のビジュアル)
  [ ] man page / ドキュメント整備
  → ゴール: 日常的に信頼して使えるストレージ。

Phase 4 — 発展 (オプション)
───────────────────────────────────────────────
  [ ] Web UI (Svelte/Vue)
  [ ] SSD キャッシュ (dm-cache / bcache 統合)
  [ ] Btrfs (single) + スナップショット対応
  [ ] パッケージング (deb / rpm / AUR)
  [ ] NFS / SMB エクスポート自動設定
```

---

## 9. テスト戦略

### 9.1 ゾーン分割の単体テスト

planner はプール最重要のロジックなので、徹底的にテストする:

```rust
#[test]
fn test_three_equal_disks() {
    let disks = vec![tb(4), tb(4), tb(4)];
    let zones = compute_zones(&disks, Redundancy::Single);
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].raid_level, RaidLevel::Raid5);
    assert_eq!(zones[0].effective_capacity, tb(8));
}

#[test]
fn test_mixed_disks_2_4_4() {
    let disks = vec![tb(2), tb(4), tb(4)];
    let zones = compute_zones(&disks, Redundancy::Single);
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].raid_level, RaidLevel::Raid5);  // 3台 × 2TB
    assert_eq!(zones[1].raid_level, RaidLevel::Raid1);  // 2台 × 2TB
    assert_eq!(total_effective(&zones), tb(6));
}

#[test]
fn test_single_disk_warns() {
    let disks = vec![tb(2)];
    let zones = compute_zones(&disks, Redundancy::Single);
    assert_eq!(zones[0].raid_level, RaidLevel::Single);
    assert!(zones[0].warnings.contains(&Warning::NoRedundancy));
}

#[test]
fn test_add_disk_replan() {
    let before = vec![tb(2), tb(4)];
    let after  = vec![tb(2), tb(4), tb(4)];
    let diff = compute_replan(&before, &after, Redundancy::Single);
    // Zone 0 が RAID1 → RAID5 に昇格
    assert!(diff.zone_changes[0].is_upgrade());
}
```

### 9.2 ループバックデバイスでの統合テスト

```bash
#!/bin/bash
# test/integration/basic_pool.sh

set -e

# テスト用仮想ディスク作成
for i in 0 1 2; do
    dd if=/dev/zero of=/tmp/puddle-test-${i}.img bs=1M count=512
    LOOP[$i]=$(losetup --find --show /tmp/puddle-test-${i}.img)
done

# プール作成・ディスク追加
puddle init ${LOOP[0]} --mkfs ext4
puddle add ${LOOP[1]} --yes
puddle add ${LOOP[2]} --yes

# データ書き込み
MOUNT=$(puddle status --json | jq -r '.mount_point')
dd if=/dev/urandom of=${MOUNT}/testfile bs=1M count=50
HASH_BEFORE=$(md5sum ${MOUNT}/testfile | cut -d' ' -f1)

# 障害シミュレーション: 1台を fail
mdadm --fail /dev/md/puddle-z0 ${LOOP[1]}

# データ読み出し確認
HASH_AFTER=$(md5sum ${MOUNT}/testfile | cut -d' ' -f1)
[ "$HASH_BEFORE" = "$HASH_AFTER" ] && echo "PASS: data intact after failure"

# クリーンアップ
puddle destroy --yes
for i in 0 1 2; do
    losetup -d ${LOOP[$i]}
    rm /tmp/puddle-test-${i}.img
done
```

### 9.3 データ救出テスト

puddle バイナリを使わずにデータを読み出せることを確認:

```bash
# puddle を使わず、標準ツールだけで復旧
mdadm --assemble --scan
vgscan
vgchange -ay
mount /dev/mapper/puddle--pool-data /mnt/recovery
md5sum /mnt/recovery/testfile  # 元のハッシュと一致すること
```

---

## 10. リスクと対策

| リスク | 影響 | 対策 |
|---|---|---|
| ゾーンリプラン中の電源断 | 中間状態でスタック | 操作ログによるロールバック。mdadm/LVM 自体は整合性を保持 |
| mdadm RAID5 write hole | パリティ不整合（UPS なし環境） | ジャーナル付き RAID5 (`--write-journal`) を推奨。ドキュメントで注意喚起 |
| メタデータ TOML の破損 | puddle が構成を読めない | 全ディスクにレプリカ保持。最悪 mdadm --assemble --scan で手動復旧可能 |
| sgdisk/mdadm/LVM の非互換変更 | ラッパーが壊れる | バージョンチェック。CI で複数ディストリを検証 |
| ユーザーが誤ったデバイスを指定 | データ喪失 | デバイス情報のプレビュー表示 + 確認プロンプト。既存パーティション検出で警告 |

---

## 11. 将来の拡張ポイント

### 11.1 SSD キャッシュ層

dm-cache または bcache を LV の下に挟む:

```
LV (puddle-pool/data)
  └── dm-cache
        ├── origin: /dev/md/puddle-z0 (HDD RAID)
        └── cache:  /dev/nvme0n1p1 (SSD)
```

### 11.2 Btrfs 統合

LV 上に Btrfs (single モード) を載せることで、
RAID は mdadm に任せつつ Btrfs のスナップショット・チェックサム機能を活用:

```
Btrfs (single, no RAID) on /dev/mapper/puddle--pool-data
  └── LVM
        └── mdadm RAID arrays (冗長性はここで担保)
```

### 11.3 Web UI

API サーバーを puddled に内蔵し、Svelte SPA で状態表示・操作:

```
puddled --web-ui :8080
```

---

## 12. まとめ

puddle v2 は「賢い mdadm + LVM の自動化ラッパー」である。

- **ゼロから書く部分**: ゾーン分割アルゴリズム、CLI/UX、監視統合 (~4000行)
- **既存技術に委ねる部分**: パリティ計算、リビルド、ボリューム管理、ファイルシステム
- **最大の強み**: puddle がなくても標準 Linux ツールでデータにアクセス可能

この設計により、Drobo/SHR 相当の「ディスクを足すだけ」の体験を、
完全オープンソースかつ安全に実現できる。
