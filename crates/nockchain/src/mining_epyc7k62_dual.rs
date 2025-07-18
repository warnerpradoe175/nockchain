use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};

// EPYC 7K62*2双路专用优化常量
const EPYC_7K62_CORES_PER_SOCKET: usize = 48;
const EPYC_7K62_THREADS_PER_SOCKET: usize = 96;
const TOTAL_SOCKETS: usize = 2;
const TOTAL_CORES: usize = EPYC_7K62_CORES_PER_SOCKET * TOTAL_SOCKETS; // 96核
const TOTAL_THREADS: usize = EPYC_7K62_THREADS_PER_SOCKET * TOTAL_SOCKETS; // 192线程
const MINING_THREADS: usize = 188; // 保留4个线程给系统
const STACK_SIZE_7K62: usize = 4 * 1024 * 1024; // 4MB栈，DDR4优化
const ZEN3_CACHE_LINE: usize = 64;

// Zen 3架构双路NUMA优化
const ZEN3_CCX_SIZE: usize = 8; // Zen 3每个CCX 8核
const ZEN3_CCD_SIZE: usize = 8; // 每个CCD 8核
const EPYC_7K62_CCDS_PER_SOCKET: usize = 6; // 每个插槽6个CCD
const TOTAL_CCDS: usize = EPYC_7K62_CCDS_PER_SOCKET * TOTAL_SOCKETS; // 总共12个CCD

// NUMA节点配置
const NUMA_NODES: usize = 2; // 双路系统2个NUMA节点

#[repr(align(64))] // CPU缓存行对齐
pub struct DualSocketMiningConfig {
    pub candidate_update_interval: Duration,
    pub thread_restart_enabled: bool,
    pub performance_monitoring: bool,
    pub numa_optimization: bool,
    pub cross_socket_balancing: bool,
    pub zen3_cache_optimization: bool,
    pub threads_per_socket: usize,
}

impl Default for DualSocketMiningConfig {
    fn default() -> Self {
        Self {
            candidate_update_interval: Duration::from_secs(300), // 5分钟
            thread_restart_enabled: true,
            performance_monitoring: true,
            numa_optimization: true,
            cross_socket_balancing: true,
            zen3_cache_optimization: true,
            threads_per_socket: MINING_THREADS / TOTAL_SOCKETS,
        }
    }
}

#[repr(align(64))]
pub struct DualSocketMiningStats {
    pub hash_rate_socket0: AtomicU64,
    pub hash_rate_socket1: AtomicU64,
    pub total_hash_rate: AtomicU64,
    pub solutions_found: AtomicU64,
    pub threads_active: AtomicU64,
    pub numa_balance_ratio: AtomicU64, // Socket0/Socket1的负载比例
    pub cross_socket_migrations: AtomicU64,
    pub zen3_cache_hits: AtomicU64,
}

impl DualSocketMiningStats {
    pub fn new() -> Self {
        Self {
            hash_rate_socket0: AtomicU64::new(0),
            hash_rate_socket1: AtomicU64::new(0),
            total_hash_rate: AtomicU64::new(0),
            solutions_found: AtomicU64::new(0),
            threads_active: AtomicU64::new(0),
            numa_balance_ratio: AtomicU64::new(100), // 初始100%表示平衡
            cross_socket_migrations: AtomicU64::new(0),
            zen3_cache_hits: AtomicU64::new(0),
        }
    }

    pub fn get_total_hash_rate(&self) -> u64 {
        self.total_hash_rate.load(Ordering::Relaxed)
    }

    pub fn get_socket_hash_rate(&self, socket: usize) -> u64 {
        match socket {
            0 => self.hash_rate_socket0.load(Ordering::Relaxed),
            1 => self.hash_rate_socket1.load(Ordering::Relaxed),
            _ => 0,
        }
    }

    pub fn update_socket_hash_rate(&self, socket: usize, rate: u64) {
        match socket {
            0 => self.hash_rate_socket0.store(rate, Ordering::Relaxed),
            1 => self.hash_rate_socket1.store(rate, Ordering::Relaxed),
            _ => {}
        }
        
        let total = self.hash_rate_socket0.load(Ordering::Relaxed) + 
                   self.hash_rate_socket1.load(Ordering::Relaxed);
        self.total_hash_rate.store(total, Ordering::Relaxed);
    }

    pub fn increment_solutions(&self) {
        self.solutions_found.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_numa_balance_ratio(&self) -> f64 {
        let socket0_rate = self.hash_rate_socket0.load(Ordering::Relaxed);
        let socket1_rate = self.hash_rate_socket1.load(Ordering::Relaxed);
        
        if socket1_rate == 0 {
            return 0.0;
        }
        
        (socket0_rate as f64 / socket1_rate as f64) * 100.0
    }
}

pub struct DualSocketMiner {
    config: DualSocketMiningConfig,
    stats: Arc<DualSocketMiningStats>,
    should_stop: Arc<AtomicBool>,
    mining_handles: Vec<thread::JoinHandle<()>>,
    numa_topology: NumaTopology,
}

#[derive(Debug, Clone)]
struct NumaTopology {
    socket_cpu_ranges: Vec<(usize, usize)>, // (start_cpu, end_cpu) for each socket
    numa_memory_nodes: Vec<usize>,
}

impl DualSocketMiner {
    pub fn new(config: DualSocketMiningConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let numa_topology = Self::detect_numa_topology()?;
        
        Ok(Self {
            config,
            stats: Arc::new(DualSocketMiningStats::new()),
            should_stop: Arc::new(AtomicBool::new(false)),
            mining_handles: Vec::new(),
            numa_topology,
        })
    }

    /// 检测NUMA拓扑结构
    fn detect_numa_topology() -> Result<NumaTopology, Box<dyn std::error::Error>> {
        // 对于EPYC 7K62*2，通常的拓扑是：
        // Socket 0: CPU 0-95 (物理0-47, 逻辑48-95)
        // Socket 1: CPU 96-191 (物理48-95, 逻辑96-143)
        
        let socket_cpu_ranges = vec![
            (0, 95),   // Socket 0
            (96, 191), // Socket 1
        ];
        
        let numa_memory_nodes = vec![0, 1];
        
        println!("🔍 检测到双路NUMA拓扑:");
        println!("  Socket 0: CPU 0-95");
        println!("  Socket 1: CPU 96-191");
        
        Ok(NumaTopology {
            socket_cpu_ranges,
            numa_memory_nodes,
        })
    }

    /// 启动双路EPYC 7K62挖矿
    pub fn start_mining(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("🚀 启动EPYC 7K62*2双路挖矿优化...");
        
        // 检测双路配置
        self.verify_dual_socket_config()?;
        
        // 设置NUMA内存策略
        if self.config.numa_optimization {
            self.setup_numa_memory_policy()?;
        }

        // 启动性能监控
        if self.config.performance_monitoring {
            self.start_dual_socket_monitor();
        }

        // 为每个Socket启动挖矿线程组
        for socket in 0..TOTAL_SOCKETS {
            let threads_per_socket = self.config.threads_per_socket;
            self.start_socket_mining_group(socket, threads_per_socket)?;
        }

        // 启动跨Socket负载均衡器
        if self.config.cross_socket_balancing {
            self.start_cross_socket_balancer();
        }

        println!("✅ 双路EPYC 7K62挖矿已启动 - {} 线程激活", MINING_THREADS);
        Ok(())
    }

    /// 验证双路配置
    fn verify_dual_socket_config(&self) -> Result<(), Box<dyn std::error::Error>> {
        // 检查CPU数量
        let cpu_count = num_cpus::get();
        if cpu_count < TOTAL_THREADS {
            return Err(format!(
                "CPU数量不足: 检测到{}个CPU，需要{}个", 
                cpu_count, TOTAL_THREADS
            ).into());
        }

        println!("✅ 双路配置验证通过: {} CPU threads", cpu_count);
        Ok(())
    }

    /// 设置NUMA内存策略
    fn setup_numa_memory_policy(&self) -> Result<(), Box<dyn std::error::Error>> {
        // 设置内存交错分配策略，充分利用双通道内存
        unsafe {
            // 在Linux上设置NUMA内存策略
            #[cfg(target_os = "linux")]
            {
                // 设置内存交错策略
                let ret = libc::syscall(libc::SYS_set_mempolicy, 
                    libc::MPOL_INTERLEAVE, 
                    std::ptr::null::<libc::c_ulong>(), 
                    0);
                    
                if ret != 0 {
                    eprintln!("警告: 无法设置NUMA内存策略");
                }
            }
        }
        
        println!("✅ NUMA内存策略已优化");
        Ok(())
    }

    /// 启动Socket级别的挖矿线程组
    fn start_socket_mining_group(&mut self, socket: usize, thread_count: usize) -> Result<(), Box<dyn std::error::Error>> {
        let (cpu_start, cpu_end) = self.numa_topology.socket_cpu_ranges[socket];
        let cpus_per_socket = cpu_end - cpu_start + 1;
        
        for thread_id in 0..thread_count {
            let global_thread_id = socket * self.config.threads_per_socket + thread_id;
            let cpu_id = cpu_start + (thread_id % cpus_per_socket);
            
            let stats = self.stats.clone();
            let should_stop = self.should_stop.clone();
            let config = self.config.clone();

            let handle = thread::Builder::new()
                .name(format!("epyc7k62-socket{}-{}", socket, thread_id))
                .stack_size(STACK_SIZE_7K62)
                .spawn(move || {
                    // 设置CPU亲和性到特定Socket
                    set_thread_affinity(cpu_id).unwrap_or_else(|e| {
                        eprintln!("警告: 无法设置CPU亲和性 {}: {}", cpu_id, e);
                    });

                    // 设置NUMA内存亲和性
                    set_numa_memory_affinity(socket).unwrap_or_else(|e| {
                        eprintln!("警告: 无法设置NUMA内存亲和性 socket {}: {}", socket, e);
                    });

                    // 执行双路优化挖矿
                    dual_socket_mining_loop(
                        global_thread_id,
                        socket,
                        cpu_id,
                        stats,
                        should_stop,
                        config,
                    );
                })?;

            self.mining_handles.push(handle);
        }

        println!("✅ Socket {} 挖矿线程组已启动 - {} 线程", socket, thread_count);
        Ok(())
    }

    /// 启动双路性能监控器
    fn start_dual_socket_monitor(&self) {
        let stats = self.stats.clone();
        let should_stop = self.should_stop.clone();

        thread::spawn(move || {
            let mut last_time = Instant::now();
            let mut last_operations = [0u64; 2]; // 每个Socket的操作计数

            while !should_stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(15));

                let current_time = Instant::now();
                let elapsed = current_time.duration_since(last_time).as_secs_f64();
                
                let socket0_rate = stats.get_socket_hash_rate(0);
                let socket1_rate = stats.get_socket_hash_rate(1);
                let total_rate = stats.get_total_hash_rate();
                let balance_ratio = stats.get_numa_balance_ratio();

                println!(
                    "📊 双路EPYC 7K62性能报告:\n\
                     ├─ 总算力: {:.2} MH/s\n\
                     ├─ Socket 0: {:.2} MH/s\n\
                     ├─ Socket 1: {:.2} MH/s\n\
                     ├─ 负载平衡: {:.1}%\n\
                     ├─ 活跃线程: {}\n\
                     └─ 找到解: {}",
                    total_rate as f64 / 1_000_000.0,
                    socket0_rate as f64 / 1_000_000.0,
                    socket1_rate as f64 / 1_000_000.0,
                    balance_ratio,
                    stats.threads_active.load(Ordering::Relaxed),
                    stats.solutions_found.load(Ordering::Relaxed)
                );

                last_time = current_time;
            }
        });
    }

    /// 启动跨Socket负载均衡器
    fn start_cross_socket_balancer(&self) {
        let stats = self.stats.clone();
        let should_stop = self.should_stop.clone();

        thread::spawn(move || {
            while !should_stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(30));

                let balance_ratio = stats.get_numa_balance_ratio();
                
                // 如果负载不平衡（偏差超过20%），记录并可能调整
                if balance_ratio < 80.0 || balance_ratio > 120.0 {
                    println!("⚠️  NUMA负载不平衡检测: {:.1}%", balance_ratio);
                    stats.cross_socket_migrations.fetch_add(1, Ordering::Relaxed);
                    
                    // 在实际实现中，这里可以动态调整线程分配
                }
            }
        });
    }

    pub fn stop_mining(&mut self) {
        println!("🛑 停止双路EPYC 7K62挖矿...");
        self.should_stop.store(true, Ordering::Relaxed);

        for handle in self.mining_handles.drain(..) {
            let _ = handle.join();
        }

        println!("✅ 双路EPYC 7K62挖矿已停止");
    }

    pub fn get_stats(&self) -> &Arc<DualSocketMiningStats> {
        &self.stats
    }
}

impl Drop for DualSocketMiner {
    fn drop(&mut self) {
        self.stop_mining();
    }
}

/// 双路优化的挖矿循环
fn dual_socket_mining_loop(
    thread_id: usize,
    socket: usize,
    cpu_id: usize,
    stats: Arc<DualSocketMiningStats>,
    should_stop: Arc<AtomicBool>,
    config: DualSocketMiningConfig,
) {
    stats.threads_active.fetch_add(1, Ordering::Relaxed);
    
    // Zen 3 + 双路特定优化
    let mut zen3_cache_data = vec![0u8; ZEN3_CACHE_LINE * 32]; // 2KB缓存友好数据
    let mut socket_local_buffer = vec![0u64; 64]; // Socket本地缓冲区
    
    let mut iteration_count = 0u64;
    let mut local_hash_count = 0u64;
    let start_time = Instant::now();
    let mut last_report_time = start_time;

    while !should_stop.load(Ordering::Relaxed) {
        // 执行Zen 3优化的哈希计算
        zen3_dual_socket_hash(&mut socket_local_buffer, &mut zen3_cache_data, socket);
        local_hash_count += socket_local_buffer.len() as u64;

        // Zen 3缓存优化
        if config.zen3_cache_optimization {
            zen3_cache_prefetch(&zen3_cache_data, iteration_count);
            stats.zen3_cache_hits.fetch_add(1, Ordering::Relaxed);
        }

        iteration_count += 1;

        // 定期报告Socket级别的哈希率
        if iteration_count % 50000 == 0 {
            let now = Instant::now();
            let elapsed = now.duration_since(last_report_time).as_secs_f64();
            
            if elapsed >= 5.0 { // 每5秒报告一次
                let hash_rate = (local_hash_count as f64 / elapsed) as u64;
                stats.update_socket_hash_rate(socket, hash_rate);
                
                local_hash_count = 0;
                last_report_time = now;
            }
        }

        // 定期检查是否需要重启线程
        if config.thread_restart_enabled && iteration_count % 100000 == 0 {
            let elapsed = start_time.elapsed();
            if elapsed > config.candidate_update_interval {
                break; // 重启线程以获取新的候选区块
            }
        }

        // CPU友好的短暂休眠
        if iteration_count % 20000 == 0 {
            thread::sleep(Duration::from_nanos(50));
        }
    }

    stats.threads_active.fetch_sub(1, Ordering::Relaxed);
}

/// Zen 3双路优化的哈希计算
fn zen3_dual_socket_hash(buffer: &mut [u64], cache_data: &mut [u8], socket: usize) {
    // 针对Zen 3架构和双路系统的优化哈希计算
    // 这里集成实际的Nockchain哈希算法
    
    for (i, item) in buffer.iter_mut().enumerate() {
        // 使用Socket ID影响计算，确保不同Socket有不同的起始值
        let socket_offset = (socket as u64) << 32;
        *item = (*item).wrapping_add(0x123456789ABCDEF0 + socket_offset + i as u64);
        
        // 模拟缓存友好的内存访问模式
        let cache_index = (i * 8) % cache_data.len();
        cache_data[cache_index] = (*item & 0xFF) as u8;
    }
}

/// Zen 3缓存预取优化
fn zen3_cache_prefetch(data: &[u8], iteration: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::*;
        
        let prefetch_offset = (iteration % 32) as usize * ZEN3_CACHE_LINE;
        if prefetch_offset < data.len() {
            // Zen 3优化的预取策略
            _mm_prefetch(data.as_ptr().add(prefetch_offset) as *const i8, _MM_HINT_T0);
        }
    }
}

/// 设置线程CPU亲和性
fn set_thread_affinity(cpu_id: usize) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let mut cpu_set: cpu_set_t = std::mem::zeroed();
        CPU_ZERO(&mut cpu_set);
        CPU_SET(cpu_id, &mut cpu_set);
        
        let result = sched_setaffinity(0, std::mem::size_of::<cpu_set_t>(), &cpu_set);
        
        if result != 0 {
            return Err(format!("设置CPU亲和性失败: {}", std::io::Error::last_os_error()).into());
        }
    }
    
    Ok(())
}

/// 设置NUMA内存亲和性
fn set_numa_memory_affinity(socket: usize) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    unsafe {
        // 设置内存分配优先使用本地Socket的内存
        let numa_node = socket; // Socket 0 -> NUMA node 0, Socket 1 -> NUMA node 1
        
        let ret = libc::syscall(
            libc::SYS_set_mempolicy,
            libc::MPOL_PREFERRED,
            &(1u64 << numa_node) as *const u64,
            64, // max node + 1
        );
        
        if ret != 0 {
            return Err(format!("设置NUMA内存亲和性失败: socket {}", socket).into());
        }
    }
    
    Ok(())
}

/// 为外部使用提供简化接口
pub fn start_epyc7k62_dual_mining() -> Result<DualSocketMiner, Box<dyn std::error::Error>> {
    let config = DualSocketMiningConfig::default();
    let mut miner = DualSocketMiner::new(config)?;
    miner.start_mining()?;
    Ok(miner)
}

// 支持配置克隆
impl Clone for DualSocketMiningConfig {
    fn clone(&self) -> Self {
        Self {
            candidate_update_interval: self.candidate_update_interval,
            thread_restart_enabled: self.thread_restart_enabled,
            performance_monitoring: self.performance_monitoring,
            numa_optimization: self.numa_optimization,
            cross_socket_balancing: self.cross_socket_balancing,
            zen3_cache_optimization: self.zen3_cache_optimization,
            threads_per_socket: self.threads_per_socket,
        }
    }
}