# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

puddle は Synology SHR / Drobo BeyondRAID のオープン再実装。異種容量ディスクをゾーン分割し、各ゾーンで最適な RAID (mdadm) を構成、LVM で結合する軽量ストレージプール管理ツール。Rust 製。

詳細は `docs/SPEC.md` (設計書) と `plan.md` (実装計画) を参照。

## ビルド・テスト

```bash
cargo build                          # ビルド
cargo test                           # 単体テスト (planner, metadata, モック付き executor)
cargo clippy && cargo fmt --check    # lint + format チェック
cargo test --features integration    # 統合テスト (要 root / privileged container)
```

## 開発ワークフロー

**TDD を守ること。** 新しい機能やモジュールを実装する際は、必ず先にテストを書き、テストが失敗することを確認してから実装コードを書く。Red → Green → Refactor のサイクルを繰り返す。

**ドキュメント同期を守ること。** SPEC.md にないアルゴリズムや設計判断を実装する場合は、その都度 `docs/` 内の該当ファイルに追記する。計画を変更・完了した場合は `plan.md` も更新する。コードとドキュメントを常に一致させる。

**README.md を最新に保つこと。** CLI の使い方（サブコマンド、オプション）に変更があった場合は、必ず `README.md` も更新する。ユーザーが最初に読むドキュメントなので、常に正確な情報を反映させる。

## アーキテクチャ

```
src/
  types.rs          共有型定義 (RaidLevel, DiskInfo, ZoneSpec, PoolConfig 等)
  planner/          ゾーン分割アルゴリズム (純粋ロジック、外部依存なし)
    zone.rs         compute_zones() — 核心アルゴリズム
    diff.rs         リプラン差分計算
    capacity.rs     実効容量計算
  executor/         外部コマンドラッパー (sgdisk, mdadm, LVM, FS)
    → trait で抽象化し、テスト時にモック差し替え可能
  metadata/         TOML メタデータの読み書き・ディスク間同期
  monitor/          SMART / udev / mdstat 監視 (Phase 2以降)
  cli/              clap サブコマンド (init, add, status)
  daemon.rs         puddled デーモン (Phase 3以降)
```

核心は `planner/zone.rs` のゾーン分割アルゴリズム。ディスクを容量順ソート→容量境界でゾーン分割→参加ディスク数に応じて RAID レベル自動選択。

## 関連ドキュメント

- `docs/SPEC.md` — 設計書（アルゴリズム詳細、CLI 設計、状態遷移、テスト戦略等）
- `plan.md` — 実装計画（Phase 1 MVP のステップ分解、開発環境構成、依存関係）
