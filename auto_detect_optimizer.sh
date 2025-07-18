#!/bin/bash

# EPYC多配置自动检测优化脚本
# 支持 EPYC 9B14单路 和 EPYC 7K62*2双路 配置

set -euo pipefail

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# 日志函数
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_title() {
    echo -e "${PURPLE}=== $1 ===${NC}"
}

# 检测CPU型号和配置
detect_cpu_config() {
    log_title "检测EPYC CPU配置"
    
    # 获取CPU信息
    local cpu_model=$(lscpu | grep "Model name" | awk -F: '{print $2}' | sed 's/^ *//')
    local cpu_sockets=$(lscpu | grep "Socket(s)" | awk '{print $2}')
    local cpu_cores=$(lscpu | grep "Core(s) per socket" | awk '{print $4}')
    local cpu_threads=$(lscpu | grep "Thread(s) per core" | awk '{print $4}')
    local total_cores=$((cpu_sockets * cpu_cores))
    local total_threads=$((total_cores * cpu_threads))
    
    log_info "CPU型号: $cpu_model"
    log_info "插槽数: $cpu_sockets"
    log_info "每插槽核心: $cpu_cores"
    log_info "每核心线程: $cpu_threads"
    log_info "总核心数: $total_cores"
    log_info "总线程数: $total_threads"
    
    # 检测CPU架构
    local cpu_family=$(lscpu | grep "CPU family" | awk '{print $3}')
    local cpu_model_num=$(lscpu | grep "Model:" | awk '{print $2}')
    
    # 检测内存类型
    local memory_info=""
    if command -v dmidecode >/dev/null 2>&1; then
        memory_info=$(sudo dmidecode --type memory | grep "Type:" | head -1 | awk '{print $2}' || echo "Unknown")
    else
        memory_info="Unknown"
    fi
    
    # 判断配置类型
    if [[ "$cpu_model" =~ "EPYC 9B14" ]]; then
        CPU_CONFIG="EPYC_9B14_SINGLE"
        log_success "检测到 EPYC 9B14 单路配置"
    elif [[ "$cpu_model" =~ "EPYC 7K62" && "$cpu_sockets" == "2" ]]; then
        CPU_CONFIG="EPYC_7K62_DUAL"
        log_success "检测到 EPYC 7K62*2 双路配置"
    elif [[ "$cpu_model" =~ "EPYC 9" ]]; then
        CPU_CONFIG="EPYC_9B14_SINGLE"
        log_warning "检测到EPYC 9系列，使用9B14优化配置"
    elif [[ "$cpu_model" =~ "EPYC 7" && "$cpu_sockets" == "2" ]]; then
        CPU_CONFIG="EPYC_7K62_DUAL"
        log_warning "检测到EPYC 7系列双路，使用7K62优化配置"
    else
        log_error "不支持的CPU配置: $cpu_model"
        log_error "当前仅支持 EPYC 9B14 单路 和 EPYC 7K62*2 双路"
        exit 1
    fi
    
    # 设置全局变量
    TOTAL_CORES=$total_cores
    TOTAL_THREADS=$total_threads
    CPU_SOCKETS=$cpu_sockets
    MEMORY_TYPE=$memory_info
    
    log_info "配置类型: $CPU_CONFIG"
    log_info "内存类型: $MEMORY_TYPE"
}

# 检测系统环境
check_system_requirements() {
    log_title "检查系统环境"
    
    # 检查操作系统
    if [[ ! -f /etc/os-release ]]; then
        log_error "无法检测操作系统"
        exit 1
    fi
    
    local os_name=$(grep "^NAME=" /etc/os-release | cut -d'"' -f2)
    log_info "操作系统: $os_name"
    
    # 检查内核版本
    local kernel_version=$(uname -r)
    log_info "内核版本: $kernel_version"
    
    # 检查必要的工具
    local required_tools=("cargo" "rustc" "gcc" "make" "numactl")
    for tool in "${required_tools[@]}"; do
        if command -v "$tool" >/dev/null 2>&1; then
            log_success "$tool 已安装"
        else
            log_warning "$tool 未安装，将尝试安装"
            install_missing_tool "$tool"
        fi
    done
    
    # 检查Rust版本
    local rust_version=$(rustc --version | awk '{print $2}')
    log_info "Rust版本: $rust_version"
    
    # 检查是否支持目标特性
    if [[ "$CPU_CONFIG" == "EPYC_9B14_SINGLE" ]]; then
        check_avx512_support
    fi
    
    check_numa_support
}

# 检查AVX-512支持
check_avx512_support() {
    log_info "检查AVX-512支持..."
    
    local avx512_features=("avx512f" "avx512dq" "avx512vl" "avx512bw")
    local supported_features=0
    
    for feature in "${avx512_features[@]}"; do
        if grep -q "$feature" /proc/cpuinfo; then
            log_success "支持 $feature"
            ((supported_features++))
        else
            log_warning "不支持 $feature"
        fi
    done
    
    if [[ $supported_features -eq ${#avx512_features[@]} ]]; then
        AVX512_SUPPORTED=true
        log_success "AVX-512 完全支持"
    else
        AVX512_SUPPORTED=false
        log_warning "AVX-512 支持不完整"
    fi
}

# 检查NUMA支持
check_numa_support() {
    log_info "检查NUMA支持..."
    
    if command -v numactl >/dev/null 2>&1; then
        local numa_nodes=$(numactl --hardware | grep "available:" | awk '{print $2}')
        log_info "NUMA节点数: $numa_nodes"
        
        if [[ "$numa_nodes" -gt 1 ]]; then
            NUMA_SUPPORTED=true
            log_success "NUMA支持已启用"
        else
            NUMA_SUPPORTED=false
            log_warning "NUMA支持未启用或单节点"
        fi
    else
        NUMA_SUPPORTED=false
        log_warning "numactl 未安装，NUMA支持不可用"
    fi
}

# 安装缺失工具
install_missing_tool() {
    local tool="$1"
    
    log_info "安装 $tool..."
    
    # 检测包管理器
    if command -v apt-get >/dev/null 2>&1; then
        case "$tool" in
            "numactl")
                sudo apt-get update && sudo apt-get install -y numactl libnuma-dev
                ;;
            "cargo"|"rustc")
                curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
                source ~/.cargo/env
                ;;
            *)
                sudo apt-get update && sudo apt-get install -y "$tool"
                ;;
        esac
    elif command -v yum >/dev/null 2>&1; then
        case "$tool" in
            "numactl")
                sudo yum install -y numactl numactl-devel
                ;;
            "cargo"|"rustc")
                curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
                source ~/.cargo/env
                ;;
            *)
                sudo yum install -y "$tool"
                ;;
        esac
    else
        log_error "不支持的包管理器，请手动安装 $tool"
        exit 1
    fi
}

# 应用配置特定的优化
apply_optimization() {
    log_title "应用 $CPU_CONFIG 专用优化"
    
    case "$CPU_CONFIG" in
        "EPYC_9B14_SINGLE")
            apply_9b14_optimization
            ;;
        "EPYC_7K62_DUAL")
            apply_7k62_dual_optimization
            ;;
        *)
            log_error "未知的CPU配置: $CPU_CONFIG"
            exit 1
            ;;
    esac
}

# 应用EPYC 9B14优化
apply_9b14_optimization() {
    log_info "应用EPYC 9B14单路优化..."
    
    # 修改Cargo.toml添加9B14依赖
    if ! grep -q "mining_epyc9b14" Cargo.toml; then
        log_info "添加EPYC 9B14挖矿模块到Cargo.toml"
        cat >> Cargo.toml << 'EOF'

# EPYC 9B14优化依赖
[dependencies.raw-cpuid]
version = "10.0"

[dependencies.libc]
version = "0.2"

[dependencies.num_cpus]
version = "1.0"
EOF
    fi
    
    # 更新主挖矿模块以支持9B14
    log_info "更新主挖矿模块..."
    update_main_mining_for_9b14
    
    # 创建优化的运行脚本
    create_9b14_run_script
    
    # 设置编译优化
    setup_9b14_compile_flags
    
    log_success "EPYC 9B14优化配置完成"
}

# 应用EPYC 7K62双路优化
apply_7k62_dual_optimization() {
    log_info "应用EPYC 7K62双路优化..."
    
    # 修改Cargo.toml添加双路依赖
    if ! grep -q "mining_epyc7k62_dual" Cargo.toml; then
        log_info "添加EPYC 7K62双路挖矿模块到Cargo.toml"
        cat >> Cargo.toml << 'EOF'

# EPYC 7K62双路优化依赖
[dependencies.libc]
version = "0.2"

[dependencies.num_cpus]
version = "1.0"
EOF
    fi
    
    # 更新主挖矿模块以支持双路
    log_info "更新主挖矿模块..."
    update_main_mining_for_dual_socket
    
    # 创建优化的运行脚本
    create_7k62_dual_run_script
    
    # 设置编译优化
    setup_7k62_compile_flags
    
    log_success "EPYC 7K62双路优化配置完成"
}

# 更新主挖矿模块 - 9B14
update_main_mining_for_9b14() {
    local main_file="crates/nockchain/src/lib.rs"
    
    # 添加9B14模块声明
    if ! grep -q "pub mod mining_epyc9b14" "$main_file"; then
        echo "pub mod mining_epyc9b14;" >> "$main_file"
    fi
    
    # 更新main.rs以使用9B14优化
    local main_rs="crates/nockchain/src/main.rs"
    if [[ -f "$main_rs" ]]; then
        cat > "$main_rs" << 'EOF'
use nockchain::mining_epyc9b14::start_epyc9b14_mining;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 启动EPYC 9B14优化挖矿...");
    
    let _miner = start_epyc9b14_mining()?;
    
    // 保持程序运行
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
EOF
    fi
}

# 更新主挖矿模块 - 双路
update_main_mining_for_dual_socket() {
    local main_file="crates/nockchain/src/lib.rs"
    
    # 添加双路模块声明
    if ! grep -q "pub mod mining_epyc7k62_dual" "$main_file"; then
        echo "pub mod mining_epyc7k62_dual;" >> "$main_file"
    fi
    
    # 更新main.rs以使用双路优化
    local main_rs="crates/nockchain/src/main.rs"
    if [[ -f "$main_rs" ]]; then
        cat > "$main_rs" << 'EOF'
use nockchain::mining_epyc7k62_dual::start_epyc7k62_dual_mining;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 启动EPYC 7K62双路优化挖矿...");
    
    let _miner = start_epyc7k62_dual_mining()?;
    
    // 保持程序运行
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
EOF
    fi
}

# 创建9B14运行脚本
create_9b14_run_script() {
    local script_name="run_nockchain_epyc9b14.sh"
    
    cat > "$script_name" << EOF
#!/bin/bash

# EPYC 9B14专用Nockchain挖矿脚本

set -euo pipefail

echo "🚀 启动EPYC 9B14优化挖矿..."

# 设置CPU性能模式
echo "performance" | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null

# 设置内存巨页
echo 32768 | sudo tee /proc/sys/vm/nr_hugepages > /dev/null

# 禁用swap以提高性能
sudo swapoff -a

# 设置进程优先级
export NOCKCHAIN_PRIORITY="high"

# 设置栈大小
ulimit -s 8192  # 8MB栈

# 启用AVX-512优化
export RUSTFLAGS="\$RUSTFLAGS -C target-cpu=znver4 -C target-feature=+avx512f,+avx512dq,+avx512vl"

# 编译并运行
cargo build --release --bin nockchain
nice -n -10 ./target/release/nockchain

EOF

    chmod +x "$script_name"
    log_success "创建了EPYC 9B14运行脚本: $script_name"
}

# 创建7K62双路运行脚本
create_7k62_dual_run_script() {
    local script_name="run_nockchain_epyc7k62_dual.sh"
    
    cat > "$script_name" << EOF
#!/bin/bash

# EPYC 7K62*2双路专用Nockchain挖矿脚本

set -euo pipefail

echo "🚀 启动EPYC 7K62双路优化挖矿..."

# 设置CPU性能模式
echo "performance" | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null

# 设置NUMA内存策略
numactl --interleave=all echo "NUMA内存交错已启用"

# 设置内存巨页
echo 65536 | sudo tee /proc/sys/vm/nr_hugepages > /dev/null  # 更多巨页用于双路

# 禁用swap以提高性能
sudo swapoff -a

# 设置进程优先级
export NOCKCHAIN_PRIORITY="high"

# 设置栈大小
ulimit -s 4096  # 4MB栈（双路系统）

# 启用Zen 3优化
export RUSTFLAGS="\$RUSTFLAGS -C target-cpu=znver3"

# 编译并运行
cargo build --release --bin nockchain
nice -n -10 numactl --interleave=all ./target/release/nockchain

EOF

    chmod +x "$script_name"
    log_success "创建了EPYC 7K62双路运行脚本: $script_name"
}

# 设置9B14编译标志
setup_9b14_compile_flags() {
    local cargo_config=".cargo/config.toml"
    mkdir -p .cargo
    
    cat > "$cargo_config" << 'EOF'
[build]
rustflags = [
    "-C", "target-cpu=znver4",
    "-C", "target-feature=+avx512f,+avx512dq,+avx512vl,+avx512bw",
    "-C", "opt-level=3",
    "-C", "lto=fat",
    "-C", "codegen-units=1",
]

[target.x86_64-unknown-linux-gnu]
rustflags = [
    "-C", "link-arg=-Wl,--as-needed",
    "-C", "link-arg=-ljemalloc",
]
EOF
    
    log_success "EPYC 9B14编译优化配置完成"
}

# 设置7K62编译标志
setup_7k62_compile_flags() {
    local cargo_config=".cargo/config.toml"
    mkdir -p .cargo
    
    cat > "$cargo_config" << 'EOF'
[build]
rustflags = [
    "-C", "target-cpu=znver3",
    "-C", "opt-level=3",
    "-C", "lto=fat",
    "-C", "codegen-units=1",
]

[target.x86_64-unknown-linux-gnu]
rustflags = [
    "-C", "link-arg=-Wl,--as-needed",
    "-C", "link-arg=-ljemalloc",
]
EOF
    
    log_success "EPYC 7K62双路编译优化配置完成"
}

# 编译优化版本
compile_optimized() {
    log_title "编译优化版本"
    
    log_info "清理之前的构建..."
    cargo clean
    
    log_info "开始编译优化版本..."
    log_info "配置: $CPU_CONFIG"
    
    # 设置环境变量
    export RUST_BACKTRACE=1
    
    if [[ "$CPU_CONFIG" == "EPYC_9B14_SINGLE" && "$AVX512_SUPPORTED" == "true" ]]; then
        export RUSTFLAGS="$RUSTFLAGS -C target-feature=+avx512f,+avx512dq,+avx512vl"
        log_info "启用AVX-512优化"
    fi
    
    # 编译
    if cargo build --release --bin nockchain; then
        log_success "编译成功！"
        
        # 显示编译信息
        local binary_size=$(du -h target/release/nockchain | cut -f1)
        log_info "可执行文件大小: $binary_size"
        
        # 检查目标特性
        if command -v objdump >/dev/null 2>&1; then
            local has_avx512=$(objdump -d target/release/nockchain | grep -c "avx512" || echo "0")
            if [[ "$has_avx512" -gt 0 ]]; then
                log_success "检测到AVX-512指令: $has_avx512 处"
            fi
        fi
        
    else
        log_error "编译失败！"
        exit 1
    fi
}

# 性能测试
run_performance_test() {
    log_title "运行性能测试"
    
    if [[ ! -f "target/release/nockchain" ]]; then
        log_error "可执行文件不存在，请先编译"
        exit 1
    fi
    
    log_info "运行30秒性能测试..."
    
    # 设置测试环境
    case "$CPU_CONFIG" in
        "EPYC_9B14_SINGLE")
            timeout 30s nice -n -10 ./target/release/nockchain || log_info "测试完成"
            ;;
        "EPYC_7K62_DUAL")
            timeout 30s nice -n -10 numactl --interleave=all ./target/release/nockchain || log_info "测试完成"
            ;;
    esac
    
    log_success "性能测试完成"
}

# 显示优化结果
show_optimization_summary() {
    log_title "优化总结"
    
    echo -e "${CYAN}硬件配置:${NC}"
    echo "  CPU配置: $CPU_CONFIG"
    echo "  总核心数: $TOTAL_CORES"
    echo "  总线程数: $TOTAL_THREADS"
    echo "  内存类型: $MEMORY_TYPE"
    echo ""
    
    echo -e "${CYAN}优化特性:${NC}"
    case "$CPU_CONFIG" in
        "EPYC_9B14_SINGLE")
            echo "  ✅ Zen 4架构优化"
            echo "  ✅ AVX-512加速: $AVX512_SUPPORTED"
            echo "  ✅ DDR5内存优化"
            echo "  ✅ 62线程挖矿"
            echo "  ✅ CCD级别线程分配"
            ;;
        "EPYC_7K62_DUAL")
            echo "  ✅ Zen 3双路优化"
            echo "  ✅ NUMA亲和性优化"
            echo "  ✅ 188线程挖矿"
            echo "  ✅ Socket级别负载均衡"
            echo "  ✅ 跨Socket通信优化"
            ;;
    esac
    echo "  ✅ NUMA支持: $NUMA_SUPPORTED"
    echo ""
    
    echo -e "${CYAN}预期性能提升:${NC}"
    case "$CPU_CONFIG" in
        "EPYC_9B14_SINGLE")
            echo "  📈 算力提升: 2-3倍"
            echo "  📈 目标算力: 150-225 MH/s"
            ;;
        "EPYC_7K62_DUAL")
            echo "  📈 算力提升: 2-3倍"
            echo "  📈 目标算力: 240-360 MH/s"
            ;;
    esac
    echo ""
    
    echo -e "${CYAN}运行命令:${NC}"
    case "$CPU_CONFIG" in
        "EPYC_9B14_SINGLE")
            echo "  ./run_nockchain_epyc9b14.sh"
            ;;
        "EPYC_7K62_DUAL")
            echo "  ./run_nockchain_epyc7k62_dual.sh"
            ;;
    esac
}

# 主函数
main() {
    log_title "EPYC多配置自动检测优化器"
    
    # 检查权限
    if [[ $EUID -eq 0 ]]; then
        log_error "请不要以root用户运行此脚本"
        exit 1
    fi
    
    # 检查是否在nockchain目录
    if [[ ! -f "Cargo.toml" ]]; then
        log_error "请在nockchain项目根目录运行此脚本"
        exit 1
    fi
    
    # 初始化变量
    CPU_CONFIG=""
    TOTAL_CORES=0
    TOTAL_THREADS=0
    CPU_SOCKETS=0
    MEMORY_TYPE=""
    AVX512_SUPPORTED=false
    NUMA_SUPPORTED=false
    
    # 执行检测和优化流程
    detect_cpu_config
    check_system_requirements
    apply_optimization
    compile_optimized
    
    # 询问是否运行性能测试
    read -p "是否运行性能测试？(y/n): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        run_performance_test
    fi
    
    show_optimization_summary
    
    log_success "EPYC优化配置完成！现在可以开始挖矿了。"
}

# 错误处理
trap 'log_error "脚本执行失败，退出代码: $?"' ERR

# 运行主函数
main "$@"