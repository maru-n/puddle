# puddle

**異種容量ディスクを賢く束ねる、オープンソースのストレージプール管理ツール。**

Synology SHR / Drobo BeyondRAID の体験を Linux 上で再現する。
内部的には mdadm (RAID) + LVM (ボリューム管理) を自動操作し、puddle 自身はゾーン分割アルゴリズムとオーケストレーションに専念する。

## 特徴

- **ディスクを足すだけ** — `puddle add /dev/sdX` で容量拡張が完了
- **異種容量ディスク対応** — 2TB + 4TB + 4TB のような構成で容量を無駄にしない
- **段階的拡張** — 1台から始めて、あとからディスクを追加していける
- **データ救出可能** — puddle がなくても `mdadm --assemble --scan` + `vgchange -ay` で標準ツールだけでデータにアクセスできる
- **車輪を再発明しない** — パリティ計算・リビルドは mdadm に委ね、20年以上の実績ある技術スタックを活用

## 仕組み

異なる容量のディスクを「ゾーン」に分割し、各ゾーンで最適な RAID を構成する。

```
例: Disk0 = 2TB, Disk1 = 4TB, Disk2 = 4TB

Zone A (0〜2TB):  3台参加 → RAID5 → 実効 4TB
Zone B (2〜4TB):  2台参加 → RAID1 → 実効 2TB

LVM で結合 → 合計 6TB (物理 10TB)
```

ディスク追加時に RAID レベルは自動で昇格する:

| ディスク数 | RAID レベル |
|-----------|-------------|
| 1台 | SINGLE (冗長なし) |
| 2台 | RAID1 (ミラー) |
| 3台以上 | RAID5 (パリティ) |

## クイックスタート

### ビルド

```bash
cargo build --release
```

### 使い方

```bash
# プール作成 (1台目)
sudo puddle init /dev/sdb --mkfs ext4

# ディスク追加
sudo puddle add /dev/sdc --yes
sudo puddle add /dev/sdd --yes

# 状態確認
sudo puddle status

# マウント
sudo mount /dev/mapper/puddle--pool-data /mnt/pool
```

### 動作要件

- Linux (カーネル 4.x 以降)
- Rust 1.72 以降 (ビルド時)
- 以下のパッケージが必要:
  - `mdadm` — RAID 管理
  - `lvm2` — LVM ボリューム管理
  - `gdisk` — GPT パーティション操作 (sgdisk)
  - `e2fsprogs` — ext4 ファイルシステム (mkfs.ext4, resize2fs)
- root 権限が必要

## テスト

```bash
# 単体テスト (root 不要)
cargo test

# Docker コンテナ内での E2E テスト
./scripts/test-in-docker.sh
```

Docker E2E テストでは、ループバックデバイス 3台を使って以下を検証する:

- `puddle init` → `puddle add` × 2 → `puddle status`
- マウント → データ書き込み → データ読み出し
- puddle なしでの mdadm + LVM によるデータ救出

## puddle なしでのデータ救出

puddle が使えない環境でも、標準 Linux ツールだけでデータにアクセスできる:

```bash
mdadm --assemble --scan
vgscan
vgchange -ay puddle-pool
mount /dev/mapper/puddle--pool-data /mnt/recovery
```

## 開発状況

現在 **Phase 1 (MVP)** が完了。ループバックデバイスでの動作確認済み。

| Phase | 内容 | 状態 |
|-------|------|------|
| Phase 1 | MVP: init, add, status + ゾーン分割 + E2E テスト | 完了 |
| Phase 2 | replace, upgrade, SMART 監視, webhook 通知 | 未着手 |
| Phase 3 | デーモン (puddled), RAID6/デュアル冗長 | 未着手 |
| Phase 4 | Web UI, SSD キャッシュ, パッケージング | 未着手 |

詳細は [docs/SPEC.md](docs/SPEC.md) (設計書) と [plan.md](plan.md) (実装計画) を参照。

## ライセンス

TBD
