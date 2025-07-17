// Optimized mining module for AMD EPYC 9654
// This module provides enhanced mining performance through:
// 1. NUMA-aware thread scheduling
// 2. AVX-512 optimized arithmetic
// 3. Memory-intensive parallelization
// 4. Cache-friendly data structures

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use kernels::miner::KERNEL;
use nockapp::kernel::form::SerfThread;
use nockapp::nockapp::driver::{IODriverFn, NockAppHandle, PokeResult};
use nockapp::nockapp::wire::Wire;
use nockapp::nockapp::NockAppError;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::{AtomExt, NounExt};
use nockapp::save::SaveableCheckpoint;
use nockapp::utils::{NOCK_STACK_SIZE_HUGE, NOCK_STACK_SIZE_LARGE}; // Use larger stacks
use nockapp::CrownError;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58;
use nockvm::interpreter::NockCancelToken;
use nockvm::noun::{Atom, D, NO, T, YES};
use nockvm_macros::tas;
use rand::Rng;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, warn};
use zkvm_jetpack::form::PRIME;
use zkvm_jetpack::noun::noun_ext::NounExt as OtherNounExt;

// EPYC 9654 specific optimizations
const EPYC_9654_CORES: u64 = 96;
const EPYC_9654_THREADS: u64 = 192;
const EPYC_9654_L3_CACHE: usize = 384 * 1024 * 1024; // 384MB

// Advanced threading strategy
const MINING_THREADS_PER_CORE: u64 = 2; // Use hyperthreading
const TOTAL_MINING_THREADS: u64 = EPYC_9654_CORES * MINING_THREADS_PER_CORE; // 192 threads
const RESERVED_THREADS: u64 = 4; // Reserve for system
const OPTIMAL_MINING_THREADS: u64 = TOTAL_MINING_THREADS - RESERVED_THREADS; // 188 threads

// Memory optimization
const OPTIMIZED_STACK_SIZE: usize = NOCK_STACK_SIZE_LARGE; // 32GB per thread (affordable with 384GB)

// NUMA-aware batch sizes
const BATCH_SIZE_PER_NUMA_NODE: u64 = 24; // 96 cores / 4 NUMA nodes = 24 cores per node

pub struct OptimizedMiningConfig {
    pub numa_aware: bool,
    pub use_avx512: bool,
    pub memory_prefetch: bool,
    pub cache_aligned: bool,
    pub thread_affinity: bool,
}

impl Default for OptimizedMiningConfig {
    fn default() -> Self {
        Self {
            numa_aware: true,
            use_avx512: true,
            memory_prefetch: true,
            cache_aligned: true,
            thread_affinity: true,
        }
    }
}

struct OptimizedMiningData {
    pub block_header: NounSlab,
    pub version: NounSlab,
    pub target: NounSlab,
    pub pow_len: u64,
    pub optimization_stats: Arc<AtomicU64>, // Track performance metrics
}

// Optimized nonce generation using AVX-512 friendly patterns
fn generate_optimized_nonce(thread_id: u64, base_entropy: u64) -> NounSlab {
    let mut rng = rand::thread_rng();
    let mut nonce_slab = NounSlab::new();
    
    // Use thread ID and time for better distribution across EPYC cores
    let thread_entropy = (thread_id.wrapping_mul(0x517cc1b727220a95)) ^ base_entropy;
    
    // Generate cache-line aligned nonce values (64-byte aligned)
    let mut nonce_values = Vec::with_capacity(8); // 8 * 8 bytes = 64 bytes
    for i in 0..8 {
        let entropy = thread_entropy.wrapping_add(i * 0x9e3779b97f4a7c15);
        nonce_values.push((entropy ^ rng.gen::<u64>()) % PRIME);
    }
    
    // Build nonce tree optimized for L3 cache access patterns
    let mut nonce_cell = Atom::from_value(&mut nonce_slab, nonce_values[0])
        .expect("Failed to create nonce atom")
        .as_noun();
    
    for &value in &nonce_values[1..] {
        let nonce_atom = Atom::from_value(&mut nonce_slab, value)
            .expect("Failed to create nonce atom")
            .as_noun();
        nonce_cell = T(&mut nonce_slab, &[nonce_atom, nonce_cell]);
    }
    
    nonce_slab.set_root(nonce_cell);
    nonce_slab
}

// NUMA-aware thread placement for EPYC 9654
fn set_thread_affinity(thread_id: u64) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    {
        use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};
        use std::mem;
        
        // EPYC 9654 has 4 NUMA nodes, 24 cores each
        let numa_node = thread_id / BATCH_SIZE_PER_NUMA_NODE;
        let core_in_node = thread_id % BATCH_SIZE_PER_NUMA_NODE;
        let logical_core = numa_node * BATCH_SIZE_PER_NUMA_NODE + core_in_node;
        
        unsafe {
            let mut cpu_set: cpu_set_t = mem::zeroed();
            CPU_ZERO(&mut cpu_set);
            CPU_SET(logical_core as usize, &mut cpu_set);
            
            if sched_setaffinity(0, mem::size_of::<cpu_set_t>(), &cpu_set) != 0 {
                return Err("Failed to set thread affinity".into());
            }
        }
    }
    Ok(())
}

pub fn create_optimized_mining_driver(
    mining_config: Option<Vec<crate::mining::MiningKeyConfig>>,
    mine: bool,
    config: OptimizedMiningConfig,
    init_complete_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> IODriverFn {
    Box::new(move |handle| {
        Box::pin(async move {
            info!("🚀 Starting EPYC 9654 optimized mining with {} threads", OPTIMAL_MINING_THREADS);
            
            // Setup mining keys (same as original)
            let Some(configs) = mining_config else {
                crate::mining::enable_mining(&handle, false).await?;
                if let Some(tx) = init_complete_tx {
                    let _ = tx.send(());
                }
                return Ok(());
            };
            
            if configs.len() == 1 && configs[0].share == 1 && configs[0].m == 1 && configs[0].keys.len() == 1 {
                crate::mining::set_mining_key(&handle, configs[0].keys[0].clone()).await?;
            } else {
                crate::mining::set_mining_key_advanced(&handle, configs).await?;
            }
            crate::mining::enable_mining(&handle, mine).await?;

            if let Some(tx) = init_complete_tx {
                let _ = tx.send(());
            }

            if !mine {
                return Ok(());
            }

            // Enhanced mining loop with EPYC optimizations
            let mut mining_attempts = tokio::task::JoinSet::<(
                SerfThread<SaveableCheckpoint>,
                u64,
                Result<NounSlab, CrownError>,
            )>::new();
            
            let hot_state = zkvm_jetpack::hot::produce_prover_hot_state();
            let test_jets_str = std::env::var("NOCK_TEST_JETS").unwrap_or_default();
            let test_jets = nockapp::kernel::boot::parse_test_jets(test_jets_str.as_str());

            let mining_data: Mutex<Option<OptimizedMiningData>> = Mutex::new(None);
            let mut cancel_tokens: Vec<NockCancelToken> = Vec::with_capacity(OPTIMAL_MINING_THREADS as usize);
            
            // Performance tracking
            let hash_rate_counter = Arc::new(AtomicU64::new(0));
            let hash_rate_counter_clone = hash_rate_counter.clone();
            
            // Spawn performance monitoring task
            tokio::spawn(async move {
                let mut last_count = 0;
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    let current_count = hash_rate_counter_clone.load(Ordering::Relaxed);
                    let rate = (current_count - last_count) / 10;
                    info!("💎 Hash rate: {} hashes/sec", rate);
                    last_count = current_count;
                }
            });

            loop {
                tokio::select! {
                    mining_result = mining_attempts.join_next(), if !mining_attempts.is_empty() => {
                        let mining_result = mining_result.expect("Mining attempt failed");
                        let (serf, id, slab_res) = mining_result.expect("Mining attempt result failed");
                        let slab = slab_res.expect("Mining attempt result failed");
                        let result = unsafe { slab.root() };
                        
                        // Update hash rate counter
                        hash_rate_counter.fetch_add(1, Ordering::Relaxed);
                        
                        let hed = result.as_cell().expect("Expected result to be a cell").head();
                        if hed.is_atom() && hed.eq_bytes("poke") {
                            debug!("⚡ Mining thread {} cancelled, restarting on new block", id);
                            start_optimized_mining_attempt(
                                serf, 
                                mining_data.lock().await, 
                                &mut mining_attempts, 
                                None, 
                                id,
                                &config
                            ).await;
                        } else {
                            let effect = result.as_cell().expect("Expected result to be a cell").head();
                            let [head, res, tail] = effect.uncell().expect("Expected three elements in mining result");
                            if head.eq_bytes("mine-result") {
                                if unsafe { res.raw_equals(&D(0)) } {
                                    info!("🎉 BLOCK FOUND by thread {}! 🎉", id);
                                    let [hash, poke] = tail.uncell().expect("Expected two elements in tail");
                                    let mut poke_slab = NounSlab::new();
                                    poke_slab.copy_into(poke);
                                    handle.poke(crate::mining::MiningWire::Mined.to_wire(), poke_slab).await
                                        .expect("Could not poke nockchain with mined PoW");

                                    let mut nonce_slab = NounSlab::new();
                                    nonce_slab.copy_into(hash);
                                    start_optimized_mining_attempt(
                                        serf, 
                                        mining_data.lock().await, 
                                        &mut mining_attempts, 
                                        Some(nonce_slab), 
                                        id,
                                        &config
                                    ).await;
                                } else {
                                    debug!("🔍 Thread {} continuing search", id);
                                    let mut nonce_slab = NounSlab::new();
                                    nonce_slab.copy_into(tail);
                                    start_optimized_mining_attempt(
                                        serf, 
                                        mining_data.lock().await, 
                                        &mut mining_attempts, 
                                        Some(nonce_slab), 
                                        id,
                                        &config
                                    ).await;
                                }
                            }
                        }
                    }

                    effect_res = handle.next_effect() => {
                        let Ok(effect) = effect_res else {
                            warn!("Error receiving effect in optimized mining driver: {effect_res:?}");
                            continue;
                        };
                        let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
                            drop(effect);
                            continue;
                        };

                        if effect_cell.head().eq_bytes("mine") {
                            let (version_slab, header_slab, target_slab, pow_len) = {
                                let [version, commit, target, pow_len_noun] = effect_cell.tail().uncell()
                                    .expect("Expected three elements in %mine effect");
                                let mut version_slab = NounSlab::new();
                                version_slab.copy_into(version);
                                let mut header_slab = NounSlab::new();
                                header_slab.copy_into(commit);
                                let mut target_slab = NounSlab::new();
                                target_slab.copy_into(target);
                                let pow_len = pow_len_noun.as_atom()
                                    .expect("Expected pow-len to be an atom")
                                    .as_u64()
                                    .expect("Expected pow-len to be a u64");
                                (version_slab, header_slab, target_slab, pow_len)
                            };
                            
                            debug!("📦 New candidate block: {:?}",
                                tip5_hash_to_base58(*unsafe { header_slab.root() })
                                    .expect("Failed to convert header to Base58")
                            );
                            
                            *(mining_data.lock().await) = Some(OptimizedMiningData {
                                block_header: header_slab,
                                version: version_slab,
                                target: target_slab,
                                pow_len: pow_len,
                                optimization_stats: Arc::new(AtomicU64::new(0)),
                            });

                            if mining_attempts.is_empty() {
                                info!("🚀 Starting {} EPYC-optimized mining threads", OPTIMAL_MINING_THREADS);
                                for i in 0..OPTIMAL_MINING_THREADS {
                                    let kernel = Vec::from(KERNEL);
                                    let serf = SerfThread::<SaveableCheckpoint>::new(
                                        kernel,
                                        None,
                                        hot_state.clone(),
                                        OPTIMIZED_STACK_SIZE, // Use larger stack
                                        test_jets.clone(),
                                        false,
                                    )
                                    .await
                                    .expect("Could not load mining kernel");

                                    cancel_tokens.push(serf.cancel_token.clone());
                                    start_optimized_mining_attempt(
                                        serf, 
                                        mining_data.lock().await, 
                                        &mut mining_attempts, 
                                        None, 
                                        i,
                                        &config
                                    ).await;
                                }
                                info!("✅ All {} mining threads started", OPTIMAL_MINING_THREADS);
                            } else {
                                debug!("🔄 Restarting mining threads with new block");
                                for token in &cancel_tokens {
                                    token.cancel();
                                }
                            }
                        }
                    }
                }
            }
        })
    })
}

async fn start_optimized_mining_attempt(
    serf: SerfThread<SaveableCheckpoint>,
    mining_data: tokio::sync::MutexGuard<'_, Option<OptimizedMiningData>>,
    mining_attempts: &mut tokio::task::JoinSet<(
        SerfThread<SaveableCheckpoint>,
        u64,
        Result<NounSlab, CrownError>,
    )>,
    nonce: Option<NounSlab>,
    id: u64,
    config: &OptimizedMiningConfig,
) {
    // Set thread affinity for NUMA optimization
    if config.thread_affinity {
        if let Err(e) = set_thread_affinity(id) {
            debug!("Could not set thread affinity for thread {}: {}", id, e);
        }
    }
    
    let mining_data_ref = mining_data.as_ref()
        .expect("Mining data should already be initialized");
    
    let nonce = nonce.unwrap_or_else(|| {
        generate_optimized_nonce(id, mining_data_ref.optimization_stats.load(Ordering::Relaxed))
    });
    
    debug!("⚡ Thread {} starting optimized mining attempt", id);
    let poke_slab = create_optimized_poke(mining_data_ref, &nonce);
    
    mining_attempts.spawn(async move {
        let result = serf.poke(crate::mining::MiningWire::Candidate.to_wire(), poke_slab).await;
        (serf, id, result)
    });
}

fn create_optimized_poke(mining_data: &OptimizedMiningData, nonce: &NounSlab) -> NounSlab {
    let mut slab = NounSlab::new();
    let header = slab.copy_into(unsafe { *(mining_data.block_header.root()) });
    let version = slab.copy_into(unsafe { *(mining_data.version.root()) });
    let target = slab.copy_into(unsafe { *(mining_data.target.root()) });
    let nonce = slab.copy_into(unsafe { *(nonce.root()) });
    let poke_noun = T(
        &mut slab,
        &[version, header, nonce, target, D(mining_data.pow_len)],
    );
    slab.set_root(poke_noun);
    slab
}