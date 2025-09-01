# holomotion-installer
build
```bash
#!/bin/bash

# === 使用cross工具 ===

echo "1. 安装cross工具"
cargo install cross --git https://github.com/cross-rs/cross

echo "2. 直接编译ARM64版本（自动处理依赖）"
cross build --release --target aarch64-unknown-linux-gnu

echo "3. 编译结果"
ls -la target/aarch64-unknown-linux-gnu/release/

echo "4. 其他ARM架构选项："
echo "  - aarch64-unknown-linux-gnu    (ARM64 Linux)"
echo "  - aarch64-unknown-linux-musl   (ARM64 Linux静态链接)"
echo "  - aarch64-apple-darwin          (ARM64 macOS)"
echo "  - armv7-unknown-linux-gnueabihf (ARM32 Linux)"

echo "5. 编译多个目标示例："
echo "cross build --release --target aarch64-unknown-linux-gnu"
echo "cross build --release --target aarch64-unknown-linux-musl"
```
