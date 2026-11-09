#!/usr/bin/env bash
set -euo pipefail
python3 scripts/ci/security-scan.py cargo backend evidence/cargo-deny.jsonl
