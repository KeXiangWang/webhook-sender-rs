#!/bin/bash
set -ex

cargo build --release

./target/release/webhook-sender-rs \
    --secret TEST_WEBHOOK \
    --unlimited \
    --concurrency 30 \
    --duration 180