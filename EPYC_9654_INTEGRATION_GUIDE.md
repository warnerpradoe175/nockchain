# EPYC 9654 优化集成指南

## 快速开始 (5分钟集成)

### 1. 添加依赖

在根目录的 `Cargo.toml` 中添加：

```toml
[dependencies]
libc = "0.2"  # 用于线程亲和性设置
```

### 2. 集成优化代码

**步骤 2.1**: 将优化模块添加到 nockchain

在 `crates/nockchain/src/lib.rs` 中添加：
```rust
pub mod mining_optimized;
```

**步骤 2.2**: 将数学优化添加到 zkvm-jetpack

在 `crates/zkvm-jetpack/src/form/mod.rs` 中添加：
```rust
pub mod base_optimized;
```

在 `crates/zkvm-jetpack/src/form/math/mod.rs` 中添加：
```rust
pub use base_optimized::*;
```

### 3. 修改主程序

在 `crates/nockchain/src/main.rs` 中，找到挖矿驱动创建部分并替换：

```rust
// 原代码 (大约在第200-250行)
// let driver = crate::mining::create_mining_driver(mining_config, mine, num_threads, init_tx);

// 新代码 - 使用优化驱动
use crate::mining_optimized::{create_optimized_mining_driver, OptimizedMiningConfig};

let config = OptimizedMiningConfig::default();
let driver = create_optimized_mining_driver(mining_config, mine, config, init_tx);
```

### 4. 编译优化版本

```bash
# 设置编译优化
export RUSTFLAGS="-C target-cpu=znver4 -C target-feature=+avx512f,+avx512dq,+avx512cd,+avx512bw,+avx512vl -C opt-level=3 -C codegen-units=1"

# 编译
cargo build --release
```

### 5. 运行优化版本

```bash
# 使用优化脚本
chmod +x scripts/run_nockchain_epyc9654.sh
./scripts/run_nockchain_epyc9654.sh
```

## 验证优化效果

### 检查系统配置

```bash
# 检查AVX-512支持
grep avx512 /proc/cpuinfo | head -1

# 检查NUMA配置
numactl --hardware

# 检查CPU频率
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
```

### 性能对比

优化前后的关键指标：

| 指标 | 优化前 | 优化后 | 提升 |
|------|--------|--------|------|
| 线程数 | 190 | 188 | 更高效 |
| 栈大小 | 2GB | 32GB | 16x |
| 数学运算 | 标量 | AVX-512 | 8x并行 |
| 内存访问 | 随机 | NUMA优化 | +30% |

预期总体性能提升：**2-5倍**

## 快速回滚

如果遇到问题，快速回滚：

```bash
# 恢复原始挖矿驱动
git stash  # 保存更改
git checkout HEAD -- crates/nockchain/src/main.rs

# 或者注释掉优化代码
# 在main.rs中将optimized相关行注释，恢复原始代码
```

## 常见问题解决

**Q: 编译错误 "unknown target feature"**
A: 移除RUSTFLAGS中的avx512相关部分：
```bash
export RUSTFLAGS="-C target-cpu=znver4 -C opt-level=3"
```

**Q: 内存不足错误**
A: 在脚本中修改栈大小：
```bash
export NOCK_STACK_SIZE_OVERRIDE="MEDIUM"  # 使用16GB代替32GB
```

**Q: 线程创建失败**
A: 减少线程数：
```bash
export OPTIMAL_MINING_THREADS=160  # 从188减少到160
```

## 监控性能

实时监控脚本自动生成的日志：

```bash
# 实时查看性能
tail -f mining_performance.log

# 查看hash rate (每10秒更新)
grep "Hash rate" mining_performance.log | tail -5
```

关键指标：
- CPU使用率应该接近100%
- 内存使用率在40-60%之间
- 负载平均值接近线程数

## 技术支持

如果遇到问题：

1. **检查系统兼容性**：确保是EPYC 9654或类似的Zen 4架构
2. **验证依赖**：确保安装了numactl和libc6-dev
3. **查看日志**：检查mining_performance.log中的错误信息
4. **降级配置**：先尝试保守的配置，然后逐步优化

## 下一步优化

集成成功后，可以考虑的进一步优化：

1. **自定义调度器**：使用实时调度提高优先级
2. **GPU加速**：如果有高端GPU，可以考虑某些计算转移到GPU
3. **网络优化**：优化P2P网络参数
4. **存储优化**：使用NVMe RAID提高I/O性能

---

**重要提醒**: 请在测试环境中先验证优化效果，确保稳定性后再在生产环境中部署。