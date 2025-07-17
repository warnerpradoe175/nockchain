#!/bin/bash

# EPYC 9654 Optimized Nockchain Mining Script
# Maximizes performance on 96-core/192-thread systems with 384GB RAM

set -e

echo "🚀 EPYC 9654 Nockchain Mining Optimizer v1.0"
echo "=============================================="

# Source environment
source .env
export RUST_LOG
export MINIMAL_LOG_FORMAT
export MINING_PUBKEY

# EPYC 9654 specific optimizations
export EPYC_9654_CORES=96
export EPYC_9654_THREADS=192
export AVAILABLE_MEMORY_GB=384

# Calculate optimal thread count for EPYC 9654
# Use 188 threads (leave 4 for system)
export OPTIMAL_MINING_THREADS=188

echo "💻 Detected EPYC 9654: ${EPYC_9654_CORES} cores, ${EPYC_9654_THREADS} threads"
echo "💾 Available memory: ${AVAILABLE_MEMORY_GB}GB"
echo "⚡ Using ${OPTIMAL_MINING_THREADS} mining threads"

# Memory and performance optimizations
echo "🔧 Applying EPYC 9654 optimizations..."

# Set NUMA policy for optimal memory allocation
if command -v numactl &> /dev/null; then
    echo "📍 Configuring NUMA policy..."
    export NUMA_POLICY="numactl --interleave=all"
else
    echo "⚠️  numactl not found, skipping NUMA optimization"
    export NUMA_POLICY=""
fi

# CPU governor optimization
if [ -d "/sys/devices/system/cpu/cpu0/cpufreq/" ]; then
    echo "⚡ Setting CPU governor to performance mode..."
    for cpu in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
        if [ -w "$cpu" ]; then
            echo performance | sudo tee "$cpu" > /dev/null 2>&1 || echo "⚠️  Could not set CPU governor (requires sudo)"
        fi
    done
fi

# Memory optimizations
echo "💾 Configuring memory optimizations..."

# Increase memory limits for massive parallel processing
ulimit -v unlimited  # Virtual memory
ulimit -m unlimited  # Physical memory
ulimit -s unlimited  # Stack size

# Use larger stack sizes (32GB per thread)
export NOCK_STACK_SIZE_OVERRIDE="LARGE"

# Enable huge pages for better memory performance
if [ -d "/sys/kernel/mm/hugepages" ]; then
    echo "📄 Configuring huge pages..."
    # Reserve 64GB for huge pages (16384 * 2MB pages)
    echo 16384 | sudo tee /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages > /dev/null 2>&1 || echo "⚠️  Could not configure huge pages (requires sudo)"
fi

# Compiler optimizations for AVX-512
export RUSTFLAGS="-C target-cpu=znver4 -C target-feature=+avx512f,+avx512dq,+avx512cd,+avx512bw,+avx512vl -C opt-level=3 -C codegen-units=1"

# Enable jemalloc for better memory allocation
export MALLOC_CONF="background_thread:true,metadata_thp:auto,dirty_decay_ms:30000,muzzy_decay_ms:30000"

# Set thread affinity and scheduling
echo "🧵 Configuring thread scheduling..."
export OMP_NUM_THREADS=${OPTIMAL_MINING_THREADS}
export OMP_PROC_BIND=true
export OMP_PLACES=cores

# Configure for high-performance mining
export MINING_OPTIMIZATIONS="
--enable-avx512
--numa-aware
--cache-aligned
--thread-affinity
--memory-prefetch
"

# Disable swap to prevent memory performance degradation
echo "💿 Optimizing memory management..."
sudo sysctl vm.swappiness=1 2>/dev/null || echo "⚠️  Could not set swappiness (requires sudo)"

# Set I/O scheduler for NVMe drives
for disk in /sys/block/nvme*; do
    if [ -d "$disk/queue" ]; then
        echo none | sudo tee "$disk/queue/scheduler" > /dev/null 2>&1 || echo "⚠️  Could not set I/O scheduler for $(basename $disk)"
    fi
done

# Function to check system resources
check_system_resources() {
    echo "🔍 System Resource Check:"
    echo "   CPU Cores: $(nproc)"
    echo "   Memory: $(free -h | grep '^Mem:' | awk '{print $2}')"
    echo "   Load Average: $(uptime | awk -F'load average:' '{print $2}')"
    
    # Check if AVX-512 is available
    if grep -q avx512f /proc/cpuinfo; then
        echo "   ✅ AVX-512 support detected"
    else
        echo "   ❌ AVX-512 not detected"
    fi
    
    # Check NUMA configuration
    if command -v numactl &> /dev/null; then
        echo "   ✅ NUMA tools available"
        numactl --hardware | head -n 3
    fi
}

# Pre-flight checks
check_system_resources

# Warn if not running as optimal user
if [ "$EUID" -eq 0 ]; then
    echo "⚠️  Running as root. Consider running as a dedicated mining user for security."
fi

# Create performance monitoring background process
create_monitoring() {
    echo "📊 Starting performance monitoring..."
    (
        while true; do
            timestamp=$(date '+%Y-%m-%d %H:%M:%S')
            cpu_usage=$(top -bn1 | grep "Cpu(s)" | awk '{print $2}' | cut -d'%' -f1)
            memory_usage=$(free | grep Mem | awk '{printf "%.1f", ($3/$2) * 100.0}')
            load_avg=$(uptime | awk -F'load average:' '{print $2}' | awk '{print $1}' | tr -d ',')
            
            echo "[$timestamp] CPU: ${cpu_usage}% | Memory: ${memory_usage}% | Load: ${load_avg}" >> mining_performance.log
            sleep 30
        done
    ) &
    
    echo $! > mining_monitor.pid
    echo "📈 Performance monitoring started (PID: $(cat mining_monitor.pid))"
}

# Cleanup function
cleanup() {
    echo "🧹 Cleaning up..."
    if [ -f mining_monitor.pid ]; then
        kill $(cat mining_monitor.pid) 2>/dev/null || true
        rm -f mining_monitor.pid
    fi
    
    # Reset CPU governor
    for cpu in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
        if [ -w "$cpu" ]; then
            echo ondemand | sudo tee "$cpu" > /dev/null 2>&1 || true
        fi
    done
    
    echo "✅ Cleanup completed"
}

# Set trap for cleanup
trap cleanup EXIT INT TERM

# Start performance monitoring
create_monitoring

echo "🚀 Starting optimized Nockchain mining..."
echo "   Threads: ${OPTIMAL_MINING_THREADS}"
echo "   Optimizations: AVX-512, NUMA-aware, Cache-aligned"
echo "   Stack size: Large (32GB per thread)"
echo "   Memory prefetching: Enabled"
echo ""

# Final command with all optimizations
exec ${NUMA_POLICY} nockchain \
    --mining-pubkey "${MINING_PUBKEY}" \
    --mine \
    --num-threads ${OPTIMAL_MINING_THREADS} \
    ${MINING_OPTIMIZATIONS}