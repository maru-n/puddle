# puddle v2 — 実装計画 (Phase 1 MVP)

## 目標

ループバックデバイス3台でプール作成→ディスク追加→障害後もデータ読み出し可能、
かつ puddle なしでも mdadm + LVM でデータ救出可能な状態まで。

---

## ステップ一覧

### Step 0: プロジェクト初期化 ✅

- `cargo init` + Cargo.toml に依存クレート追加
  - clap (derive), serde, toml, uuid, anyhow/thiserror
  - 注: Rust 1.72 環境のため uuid=1.7.0, getrandom=0.2.15, clap=4.4.18 にピン留め
- モジュールスケルトン作成（空の mod.rs だけ配置）
- `cargo build` が通ることを確認

### Step 1: 型定義 (`src/types.rs`) ✅

プロジェクト全体で共有する基本型を先に定義する。
他の全モジュールがこれに依存するため最初に固める。

```
RaidLevel        = Single | Raid1 | Raid5 | Raid6
Redundancy       = Single | Dual
DiskStatus       = Active | Failed | Removing
PoolStatus       = Healthy | Degraded | Critical
Warning          = NoRedundancy | PartialRedundancy | ...

DiskInfo { uuid, device_id, capacity_bytes, seq, status }
ZoneSpec { index, start_bytes, size_bytes, raid_level, participating_disk_uuids }
PoolConfig { pool.uuid/name/created_at/redundancy, disks[], zones[], lvm, state }
```

判断:
- PoolConfig はそのままメタデータ TOML のスキーマでもある
- serde(Serialize, Deserialize) を derive しておく
- capacity_bytes は u64 で統一（表示時のみ human-readable 変換）

### Step 2: Planner — ゾーン分割アルゴリズム (`src/planner/`) ✅

puddle の核心。純粋な計算ロジックなので外部依存なし、単体テスト駆動で開発。

**2a: `zone.rs` — compute_zones()**
- 入力: ソート済みディスク容量リスト + Redundancy
- 出力: Vec<ZoneSpec>
- SPEC §3.2 のアルゴリズムをそのまま実装
- select_raid_level() も同ファイル内に

**2b: `capacity.rs` — 実効容量計算**
- RAID レベルごとの実効容量 = zone_size × (n - parity_count)
- 合計実効容量の算出

**2c: `diff.rs` — リプラン差分計算**
- compute_replan(before_disks, after_disks, redundancy) → ReplanDiff
- ゾーン追加・削除・RAID レベル変更を検出
- Phase 1 では「拡張のみ」をサポート（縮小は Phase 2）

**テスト (tests/planner_test.rs)**:
- 均一3台 (4T×3) → Zone 1個, RAID5, 実効 8T
- 混成 (2T, 4T, 4T) → Zone 2個, RAID5+RAID1, 実効 6T
- 1台 → SINGLE + NoRedundancy 警告
- 段階的追加 (1台→2台→3台) の replan diff
- 同一容量ディスク連続時の skip 処理
- エッジケース: 0台（エラー）

### Step 3: Metadata (`src/metadata/`) ✅

**3a: `pool_config.rs` — TOML シリアライズ/デシリアライズ**
- PoolConfig ↔ TOML 文字列の変換
- SPEC §4.2 のフォーマットに準拠
- バリデーション（必須フィールドチェック等）

**3b: `sync.rs` — メタデータの読み書き**
- write_metadata(pool_config, disk_paths) → 全ディスクのメタデータパーティションに書き込み
- read_metadata(disk_path) → PoolConfig を読み出し
- Phase 1 では metadata パーティション上に ext4 をマウントしてファイル I/O
  - マウントポイントは一時ディレクトリ

### Step 4: Executor (`src/executor/`) ✅

外部コマンドのラッパー群。各モジュールは対応するコマンドの薄いラッパー。

共通設計:
- すべてのコマンド実行は `Command::new()` 経由
- stdout/stderr をキャプチャしてログ出力
- 失敗時は anyhow::Error で伝搬
- dry-run モード対応（コマンドを表示するだけで実行しない）

**4a: `partition.rs` — sgdisk ラッパー**
- create_metadata_partition(device) → 16MB パーティション作成
- create_zone_partitions(device, zones) → ゾーン用パーティション作成
- wipe_partition_table(device) → GPT 初期化
- partprobe / `blockdev --rereadpt` で kernel に通知

**4b: `mdadm.rs` — mdadm ラッパー**
- create_array(name, level, devices) → mdadm --create
- add_device(array, device) → mdadm --add (--grow が必要な場合あり)
- array_status(name) → mdadm --detail パース
- assemble / stop

**4c: `lvm.rs` — LVM ラッパー**
- pvcreate(device)
- vgcreate(vg_name, pvs) / vgextend(vg_name, pv)
- lvcreate(vg_name, lv_name, size) / lvextend
- vg_info / lv_info

**4d: `filesystem.rs` — FS ラッパー**
- mkfs(device, fs_type)
- resize(device, fs_type) — ext4: resize2fs, XFS: xfs_growfs
- mount / umount (一時マウント用)

**4e: `rollback.rs` — 操作ログ**
- OperationLog: 各ステップとそのロールバックコマンドを記録
- Phase 1 ではログ記録のみ（自動ロールバック実行は Phase 2）

### Step 5: CLI (`src/cli/`) ✅

clap derive で実装。各サブコマンドが planner + executor + metadata を組み合わせる。

**5a: `init.rs` — puddle init <device>`**
1. デバイスの存在・パーティション有無チェック
2. GPT 初期化 + metadata パーティション + zone パーティション作成
3. 1台構成の zones 計算 (SINGLE)
4. mdadm array 作成 (SINGLE なので --force)
5. LVM: pvcreate → vgcreate → lvcreate (100%FREE)
6. --mkfs 指定時: ファイルシステム作成
7. --mount 指定時: マウント
8. メタデータ書き込み
9. ⚠ 冗長性なし警告表示

**5b: `add.rs` — puddle add <device>`**
1. 既存メタデータ読み込み（いずれかのディスクから）
2. 新ディスク情報取得
3. planner で新ゾーン構成を計算
4. diff 表示 + 確認プロンプト (--yes でスキップ)
5. executor で実行:
   - 新ディスクにパーティション作成
   - 既存 mdadm アレイにデバイス追加 or 新アレイ作成
   - LVM 拡張
   - FS リサイズ
6. メタデータ更新（全ディスク）

**5c: `status.rs` — puddle status`**
1. メタデータ読み込み
2. 各 mdadm アレイの状態取得
3. LVM 情報取得
4. FS 使用量取得
5. SPEC §5.1 のフォーマットで表示

### Step 6: 統合テスト ✅

**ループバックデバイスを使った E2E テスト** (要 root 権限):
- テスト用に 256MB × 3 のループバックデバイス作成
- `puddle init` → `puddle add` × 2 → `puddle status`
- データ書き込み → mdadm --fail → データ読み出し → ハッシュ一致確認
- クリーンアップ (Drop trait で自動)
- 非 root 環境ではスキップ (テスト自体は pass 扱い)

実装済みテスト:
- `test_init_single_disk` — 1台で init
- `test_full_lifecycle` — init → add × 2 → status
- `test_data_survives_disk_failure` — 障害後のデータ整合性確認

---

## 実装順序の依存関係

```
Step 0 (scaffolding)
  └→ Step 1 (types)
       ├→ Step 2 (planner) ← 独立してテスト可能、最優先
       ├→ Step 3 (metadata)
       └→ Step 4 (executor)
            └→ Step 5 (CLI) ← planner + metadata + executor を結合
                 └→ Step 6 (integration test)
```

Step 2 (planner) は外部依存ゼロなので、Step 3/4 と並行開発可能。
最もリスクが高い（ロジックの正しさが全体の基盤）ため、先にテストを充実させる。

---

## 開発環境

### 2層構成

planner / metadata などの純粋ロジックと、executor / 統合テストなどの root 必須操作を分離する。

```
普段の開発 (ホスト / 通常ユーザー):
  cargo build                     ← 全体のビルド確認
  cargo test                      ← planner, metadata, モック付き executor テスト
  cargo clippy && cargo fmt       ← lint + format

統合テスト (VM or privileged container):
  cargo test --features integration  ← loopback + mdadm + LVM の実テスト
```

### executor の trait 抽象化

executor の各モジュールを trait で抽象化し、テスト時にモック実装を差し込めるようにする。

```rust
// 本番: 実際に sgdisk / mdadm / pvcreate 等を実行
// テスト: コマンド呼び出しを記録するだけのモック
trait PartitionManager { ... }
trait RaidManager { ... }
trait VolumeManager { ... }
trait FilesystemManager { ... }
```

CLI のハンドラはこれらの trait を受け取る。
単体テストではモックを渡し、「正しい順序で正しいコマンドが呼ばれるか」を検証する。

### feature flag による分離

```toml
# Cargo.toml
[features]
default = []
integration = []  # 統合テスト有効化
```

統合テストは `#[cfg(feature = "integration")]` で囲み、通常の `cargo test` では実行されない。

### CI 構成

- **通常テスト**: GitHub Actions 標準ランナーで `cargo test` + `cargo clippy`
- **統合テスト**: `--privileged` Docker コンテナ内で `cargo test --features integration`
  - コンテナイメージに mdadm, lvm2, e2fsprogs, gdisk をプリインストール
  - ループバックデバイスを使うため `/dev/loop*` へのアクセスが必要

---

## Phase 1.5: 実環境検証と品質向上

Phase 1 のコードは全テスト Green だが、実際のループバックデバイスで動かしていない。
ここで実環境検証を行い、発見したバグを修正する。

### Step 7: Docker コンテナでの E2E 検証 ✅

`scripts/test-in-docker.sh` で privileged Docker コンテナ内での完全 E2E 検証を実施。

確認項目:
- [x] sgdisk の引数が正しいか（パーティション作成が成功するか）
- [x] mdadm --create が正しく動くか（SINGLE の --force 含む）
- [x] LVM (pvcreate → vgcreate → lvcreate) が正しく動くか
- [x] mkfs.ext4 が成功するか
- [x] puddle add で既存アレイへのデバイス追加が動くか
- [x] puddle status で正しい情報が表示されるか
- [x] /var/lib/puddle/pool.toml が正しく生成されるか
- [x] mount → write → read が成功するか
- [x] puddle なしでの mdadm + LVM データ救出が可能か

### Step 8: 検証で発見したバグの修正 ✅

Docker 検証中に発見・修正した問題:

- [x] `reload_table()` が BLKRRPART エラーで失敗 → partprobe → partx → blockdev のフォールバックチェーンに変更、全失敗時は警告のみ
- [x] mdadm アレイ名が前回テスト残骸と衝突 → テストスクリプトに pre-cleanup 追加
- [x] `lvcreate` が Docker 内で `device not cleared` エラー → `-Wn -Zn` フラグで wipe/zero を無効化 + lvm.conf で udev_sync=0, udev_rules=0
- [x] `lvextend` が RAID1 ミラー追加時に "matches existing size" エラー → 容量変化なしの場合はスキップ
- [x] `pvcreate` に `-f` フラグ追加（既存シグネチャ対応）

Phase 1.5 追加修正 (完了):
- [x] `chrono_now()` がハードコード → `date -u` コマンドで実際のタイムスタンプを取得
- [x] `puddle add` の確認プロンプト → プレビュー表示 + `[Y/n]` 確認 (`--yes` でスキップ)
- [x] `puddle destroy` コマンド → LVM/mdadm/パーティションの順に削除、確認プロンプト付き

---

---

## Phase 2: 実用化

### ゴール

実ディスクでディスク交換ができる。SMART 異常を検知して表示できる。
操作失敗時のロールバックが自動実行される。

### Step 9: `puddle replace` — 同容量ディスク交換 ✅

障害ディスクを同容量以上のディスクに交換する。

1. 旧ディスクを全 mdadm アレイで `--fail` + `--remove`
2. 新ディスクにパーティション作成 (旧ディスクと同じゾーン構成)
3. 各 mdadm アレイに `--add` でリビルド開始
4. メタデータ更新 (旧ディスクを removed、新ディスクを active に)

テスト:
- モックで正しいコマンド順序を検証
- 旧ディスクが存在しない場合のエラーハンドリング

### Step 10: `puddle upgrade` — 容量アップグレード交換 ✅

旧ディスクを大容量ディスクに交換し、ゾーンを再構成する。

1. replace と同様にリビルド実行
2. リビルド完了後、新容量でゾーン再計算
3. 新ゾーン用のパーティション追加・mdadm アレイ作成・LVM 拡張

テスト:
- 2TB → 8TB へのアップグレード時のゾーン再計算
- リビルド中は再構成をブロック

### Step 11: `puddle health` — SMART 監視 ✅

smartctl を呼び出してディスク健全性を表示する。

- `smartctl -j` の JSON 出力をパース
- 温度、Reallocated Sector Count、Written bytes を表示
- `/proc/mdstat` をパースして RAID sync 状態を表示

テスト:
- smartctl JSON 出力のモックパーステスト
- /proc/mdstat のパーステスト

### Step 12: 操作ログのロールバック自動実行 ✅

Phase 1 で記録のみだった OperationLog を実際に実行する。

- `init` / `add` / `replace` の各操作に OperationLog を組み込む
- 途中ステップ失敗時、記録済みロールバックコマンドを逆順実行
- ロールバック自体の失敗はログ出力して続行

テスト:
- モックで途中失敗 → ロールバックコマンドが逆順で呼ばれることを検証

### Step 13: Docker E2E 検証 (Phase 2) ✅

Phase 2 の全機能を Docker privileged コンテナで E2E 検証。

- replace: ディスク交換後にデータが読めること
- destroy: プール削除後に mdadm/LVM が残っていないこと

---

## Phase 2.5: 安全性強化

### ゴール

ストレージ操作ツールとしての安全性を確保する。
操作失敗時の自動ロールバック、排他制御、入力検証、エラーハンドリングを実装。

### Step 14: ロールバックの各コマンド組み込み ✅

Phase 2 で実装した `execute_rollback()` を init/add/replace/upgrade の各コマンドに組み込む。
操作途中で失敗した場合、それまでに実行した手順を自動で巻き戻す。

- 各コマンド関数内で `OperationLog` を作成し、各ステップを `log_step()` で記録
- ステップ失敗時に `execute_rollback()` を呼んでから error を返す
- 成功時は `commit()` + `save_to_file()`

テスト:
- モックで途中失敗をシミュレート → ロールバックコマンドが逆順実行されることを検証
- 成功時はロールバック実行されないことを検証

### Step 15: flock による排他ロック ✅

2つの puddle コマンドが同時実行されることを防止する。

- `/var/lib/puddle/puddle.lock` に対する排他ロック (flock)
- ロック取得に失敗した場合は「別の puddle プロセスが実行中」エラー
- main.rs のコマンドディスパッチ前にロック取得、終了時に自動解放

テスト:
- ロック取得・解放の基本テスト
- ロック競合時のエラーメッセージ検証

### Step 16: unwrap() 除去とエラーハンドリング改善 ✅

main.rs の `.unwrap()` 6箇所を適切なエラーハンドリングに置換。

- `PoolConfig::from_toml().unwrap()` → `.context("...")?` or eprintln + exit
- metadata/sync.rs の `.unwrap()` 2箇所も修正
- pool.toml が破損している場合の graceful error 表示

テスト:
- 不正な TOML での graceful エラー表示テスト

### Step 17: デバイス検証強化 ✅

操作対象デバイスの安全性チェックを追加する。

- **マウントチェック**: 操作対象デバイスがマウント中でないことを確認
- **重複チェック**: `puddle add` で既にプールに含まれるデバイスの追加を防止
- **init 確認プロンプト**: `puddle init` にも確認プロンプト追加 (`--yes` でスキップ)
- **destroy の umount 強化**: `.ok()` 黙殺ではなく、失敗時にエラー報告

テスト:
- マウント中デバイスへの操作拒否テスト
- 重複デバイス追加拒否テスト
- init 確認プロンプトの動作テスト

### Step 18: Docker E2E 検証 (Phase 2.5) ✅

安全性修正の全機能を Docker privileged コンテナで E2E 検証。

- 排他ロック: 並行実行の衝突テスト
- ロールバック: 実デバイスでの途中失敗→ロールバック検証

---

## Phase 3 以降のスコープ外（意図的に後回し）

- puddled デーモン → Phase 3
- webhook / email 通知 → Phase 3
- RAID6 / デュアル冗長 → Phase 3（planner の型だけ Phase 1 で用意）
- リッチ CLI (TUI) → Phase 4
- 縮小リプラン (pvmove) → Phase 3

## 注意事項

- executor の全操作は root 権限が必要。テスト環境では sudo or コンテナ内で実行
- mdadm --create で SINGLE (1台) は `--force` が必要
- RAID1→RAID5 昇格は `mdadm --grow --level=raid5 --raid-devices=3` になる。これはデータ再配置を伴う重い操作であり、失敗時のリカバリを慎重に設計する必要がある
- sgdisk はパーティション番号を 1 始まりで管理。metadata は常に partition 1
- LVM の VG 名にハイフンが含まれると device-mapper 名でエスケープされる (`puddle-pool` → `puddle--pool`)。名前規則に注意
