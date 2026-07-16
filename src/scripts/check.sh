#!/usr/bin/env bash
# 商媒运营助手 Rust 工作区检查脚本（兼容 macOS bash 3.2）。
#
# 注：`cargo check --workspace` / `cargo test -p goose` 会编译 goose 的 dev-dependency
#（goose-mcp / goose-test-support），这两个 crate 源码与依赖版本有漂移（rmcp 1.7 vs 1.8、
# opentelemetry 0.31 vs 0.32，反映 goose-cc 检出处于开发中状态）。嵌入 Goose 只需 lib。
# 因此：check/clippy 用「goose lib + 自研」列表（只编译 lib，不碰 dev-dep）；
# test 只跑自研 crate（其测试只引 goose lib，不引 goose dev-dep）。
set -euo pipefail
cd "$(dirname "$0")/.."

GOOSE_PKGS=(goose goose-providers goose-context-core goose-sdk-types goose-acp-macros)
OUR_PKGS=(mediacrawler socialconnect yunying-ops yunying-config yunying-server)

case "${1:-check}" in
  check)
    ARGS=()
    for p in "${GOOSE_PKGS[@]}" "${OUR_PKGS[@]}"; do ARGS+=("-p" "$p"); done
    cargo check "${ARGS[@]}"
    ;;
  test)
    ARGS=()
    for p in "${OUR_PKGS[@]}"; do ARGS+=("-p" "$p"); done
    cargo test "${ARGS[@]}"
    ;;
  clippy)
    ARGS=()
    for p in "${GOOSE_PKGS[@]}" "${OUR_PKGS[@]}"; do ARGS+=("-p" "$p"); done
    cargo clippy "${ARGS[@]}" --all-targets -- -D warnings
    ;;
  fmt)
    cargo fmt --check
    ;;
  *)
    echo "用法: $0 [check|test|clippy|fmt]" >&2
    exit 1
    ;;
esac
