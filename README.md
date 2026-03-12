# puddle

**異種容量ディスクを賢く束ねる、オープンソースのストレージプール管理ツール。**

Synology SHR / Drobo BeyondRAID の体験を Linux 上で再現する。
内部的には mdadm (RAID) + LVM (ボリューム管理) を自動操作し、puddle 自身はゾーン分割アルゴリズムとオーケストレーションに専念する。

## 特徴

- **ディスクを足すだけ** — `puddle add /dev/sdX` で容量拡張が完了
- **異種容量ディスク対応** — 2TB + 4TB + 4TB のような構成で容量を無駄にしない
- **段階的拡張** — 1台から始めて、あとからディスクを追加していける
- **安全なディスク交換** — `puddle replace` で障害ディスクを交換、`puddle upgrade` で大容量ディスクに入替え
- **ディスク除去** — `puddle remove` でデータ退避後に安全にディスクを取り外し
- **自動監視** — `puddle monitor` で SMART + RAID 状態を継続監視、webhook 通知対応
- **自動ロールバック** — 操作途中で失敗した場合、実行済みステップを自動的に巻き戻し
- **排他制御** — flock で同時実行を防止
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

| ディスク数 | Single 冗長 | Dual 冗長 |
|-----------|-------------|-----------|
| 1台 | SINGLE (冗長なし) | SINGLE (冗長なし) |
| 2台 | RAID1 (ミラー) | RAID1 (ミラー) |
| 3台 | RAID5 (パリティ) | RAID1 (3台ミラー) |
| 4台以上 | RAID5 | RAID6 (二重パリティ) |

## クイックスタート

### ビルド

```bash
cargo build --release
```

### 使い方

```bash
# プール作成 (1台目)
sudo puddle init /dev/sdb --mkfs ext4

# デュアル冗長 (RAID6) でプール作成する場合
sudo puddle init /dev/sdb --mkfs ext4 --redundancy dual

# ディスク追加
sudo puddle add /dev/sdc --yes
sudo puddle add /dev/sdd --yes

# 状態確認
sudo puddle status

# ディスク健全性チェック
sudo puddle health

# マウント
sudo mount /dev/mapper/puddle--pool-data /mnt/pool
```

### ディスク管理

```bash
# 障害ディスクの交換 (同容量以上)
sudo puddle replace /dev/sdb /dev/sde --yes

# 大容量ディスクへのアップグレード
sudo puddle upgrade /dev/sdb /dev/sde --yes

# ディスクの安全な取り外し (データ退避後)
sudo puddle remove /dev/sdb --yes

# プールの破棄 (全データ消失)
sudo puddle destroy --yes
```

### 監視・通知

```bash
# 1回だけ SMART + RAID チェック
sudo puddle monitor --once

# 継続監視 (60秒間隔)
sudo puddle monitor

# 監視間隔を変更 (120秒)
sudo puddle monitor --interval 120

# webhook 通知の設定
sudo puddle notify --webhook https://hooks.slack.com/services/...

# テスト通知を送信
sudo puddle notify --webhook https://hooks.slack.com/services/... --test

# systemd ユニットファイルの生成
sudo puddle generate-systemd > /etc/systemd/system/puddled.service
```

### 動作要件

- Linux (カーネル 4.x 以降)
- Rust 1.72 以降 (ビルド時)
- 以下のパッケージが必要:
  - `mdadm` — RAID 管理
  - `lvm2` — LVM ボリューム管理
  - `gdisk` — GPT パーティション操作 (sgdisk)
  - `e2fsprogs` — ext4 ファイルシステム (mkfs.ext4, resize2fs)
  - `smartmontools` — SMART 監視 (puddle health / monitor 用)
- root 権限が必要

## テスト

```bash
# 単体テスト (root 不要)
cargo test

# lint + format チェック
cargo clippy && cargo fmt --check

# Docker コンテナ内での E2E テスト
./scripts/test-in-docker.sh

# 統合テスト (要 root / privileged container)
cargo test --features integration
```

## puddle なしでのデータ救出

puddle が使えない環境でも、標準 Linux ツールだけでデータにアクセスできる:

```bash
mdadm --assemble --scan
vgscan
vgchange -ay puddle-pool
mount /dev/mapper/puddle--pool-data /mnt/recovery
```

## CLI リファレンス

| コマンド | 説明 |
|---------|------|
| `puddle init <device>` | 新しいストレージプールを作成 |
| `puddle add <device>` | プールにディスクを追加 |
| `puddle status` | プールの状態を表示 |
| `puddle health` | SMART + RAID sync 状態を表示 |
| `puddle replace <old> <new>` | 障害ディスクを交換 |
| `puddle upgrade <old> <new>` | 大容量ディスクに入替え |
| `puddle remove <device>` | ディスクを安全に除去 |
| `puddle destroy` | プールを破棄 |
| `puddle monitor` | SMART + RAID 継続監視 |
| `puddle notify --webhook <url>` | webhook 通知を設定 |
| `puddle generate-systemd` | systemd ユニットファイルを出力 |

主要オプション:
- `--yes` — 確認プロンプトをスキップ
- `--mkfs ext4` — init 時にファイルシステムを作成
- `--redundancy dual` — init 時にデュアル冗長 (RAID6) を指定
- `--once` — monitor を1回だけ実行
- `--interval N` — monitor のポーリング間隔 (秒)

## 開発状況

| Phase | 内容 | 状態 |
|-------|------|------|
| Phase 1 | MVP: init, add, status + ゾーン分割 | 完了 |
| Phase 1.5 | Docker E2E 検証 + バグ修正 | 完了 |
| Phase 2 | replace, upgrade, SMART 監視, ロールバック | 完了 |
| Phase 2.5 | 安全性強化 (排他ロック, デバイス検証, エラー処理) | 完了 |
| Phase 2.7 | 信頼性検証 (プロパティベース, 境界値, 故障注入テスト) | 完了 |
| Phase 3 | RAID6, デーモン, webhook, ディスク除去 | 完了 |
| Phase 4 | リッチ CLI (TUI) | 未着手 |

詳細は [docs/SPEC.md](docs/SPEC.md) (設計書) と [plan.md](plan.md) (実装計画) を参照。

## ライセンス

TBD
