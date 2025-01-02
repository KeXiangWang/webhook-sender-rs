# webhook-sender-rs

### Build
```bash
cargo build --release
```

### Run
```bash
./target/release/webhook-sender-rs \
    --secret TEST_WEBHOOK \
    --total-requests 100000 \
    --concurrency 30 \
    --duration 180
```

```bash
./target/release/webhook-sender-rs \
    --secret TEST_WEBHOOK \
    --unlimited \
    --concurrency 30 \
    --duration 180
```