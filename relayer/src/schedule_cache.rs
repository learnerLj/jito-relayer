use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread,
    thread::{sleep, Builder, JoinHandle},
    time::Duration,
};

use jito_rpc::load_balancer::LoadBalancer;
use log::{debug, error};
use solana_metrics::datapoint_info;
use solana_sdk::{
    clock::{Slot, DEFAULT_SLOTS_PER_EPOCH},
    pubkey::Pubkey,
};

pub struct LeaderScheduleCacheUpdater {
    /// Maps slots to scheduled pubkey
    schedules: Arc<RwLock<HashMap<Slot, Pubkey>>>,

    /// Refreshes leader schedule
    refresh_thread: JoinHandle<()>,
}

#[derive(Clone)]
pub struct LeaderScheduleUpdatingHandle {
    schedule: Arc<RwLock<HashMap<Slot, Pubkey>>>,
}

/// Access handle to a constantly updating leader schedule
impl LeaderScheduleUpdatingHandle {
    pub fn new(schedule: Arc<RwLock<HashMap<Slot, Pubkey>>>) -> LeaderScheduleUpdatingHandle {
        LeaderScheduleUpdatingHandle { schedule }
    }

    pub fn leader_for_slot(&self, slot: &Slot) -> Option<Pubkey> {
        self.schedule.read().unwrap().get(slot).cloned()
    }

    pub fn leaders_for_slots(&self, slots: &[Slot]) -> HashSet<Pubkey> {
        let schedule = self.schedule.read().unwrap();
        slots
            .iter()
            .filter_map(|s| schedule.get(s).cloned())
            .collect()
    }

    pub fn is_scheduled_validator(&self, pubkey: &Pubkey) -> bool {
        self.schedule
            .read()
            .unwrap()
            .iter()
            .any(|(_, scheduled_pubkey)| scheduled_pubkey == pubkey)
    }
}

impl LeaderScheduleCacheUpdater {
    pub fn new(
        load_balancer: &Arc<LoadBalancer>,
        exit: &Arc<AtomicBool>,
    ) -> LeaderScheduleCacheUpdater {
        let schedules = Arc::new(RwLock::new(HashMap::new()));
        let refresh_thread = Self::refresh_thread(schedules.clone(), load_balancer.clone(), exit);
        LeaderScheduleCacheUpdater {
            schedules,
            refresh_thread,
        }
    }

    /// Gets a handle to a constantly updating leader schedule handler
    pub fn handle(&self) -> LeaderScheduleUpdatingHandle {
        LeaderScheduleUpdatingHandle::new(self.schedules.clone())
    }

    pub fn join(self) -> thread::Result<()> {
        self.refresh_thread.join()
    }

    fn refresh_thread(
        schedule: Arc<RwLock<HashMap<Slot, Pubkey>>>,
        load_balancer: Arc<LoadBalancer>,
        exit: &Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let exit = exit.clone();
        Builder::new()
            .name("leader-schedule-refresh".to_string())
            .spawn(move || {
                while !exit.load(Ordering::Relaxed) {
                    let mut update_ok_count = 0;
                    let mut update_fail_count = 0;

                    match Self::update_leader_cache(&load_balancer, &schedule) {
                        true => update_ok_count += 1,
                        false => update_fail_count += 1,
                    }

                    let slots_in_schedule = schedule.read().unwrap().len();

                    datapoint_info!(
                        "schedule-cache-update",
                        ("update_ok_count", update_ok_count, i64),
                        ("update_fail_count", update_fail_count, i64),
                        ("slots_in_schedule", slots_in_schedule, i64),
                    );

                    sleep(Duration::from_secs(10));
                }
            })
            .unwrap()
    }

    /// Fetches the current leader schedule from Solana RPC and updates the cache.
    /// 
    /// This method performs the actual RPC calls to get epoch info and leader schedule,
    /// then converts the relative slot numbers to absolute slot numbers for easier lookup.
    /// 
    /// # Process
    /// 1. Get current epoch info to determine slot offset
    /// 2. Fetch leader schedule for current epoch
    /// 3. Convert relative slots to absolute slots using epoch offset
    /// 4. Update the shared schedule cache atomically
    /// 
    /// # Arguments
    /// * `load_balancer` - RPC client pool for network requests
    /// * `schedule` - Shared schedule cache to update
    /// 
    /// # Returns
    /// `true` if update was successful, `false` if RPC calls failed
    pub fn update_leader_cache(
        load_balancer: &Arc<LoadBalancer>,
        schedule: &Arc<RwLock<HashMap<Slot, Pubkey>>>,
    ) -> bool {
        // Get RPC client from load balancer (selects best available)
        let rpc_client = load_balancer.rpc_client();

        // First, get current epoch information
        if let Ok(epoch_info) = rpc_client.get_epoch_info() {
            // Then, get the leader schedule for current epoch
            if let Ok(Some(leader_schedule)) = rpc_client.get_leader_schedule(None) {
                // Calculate epoch start slot for converting relative to absolute slots
                let epoch_offset = epoch_info.absolute_slot - epoch_info.slot_index;

                debug!("read leader schedule of length: {}", leader_schedule.len());

                // Build new schedule mapping with absolute slot numbers
                let mut new_schedule = HashMap::with_capacity(DEFAULT_SLOTS_PER_EPOCH as usize);
                for (pk_str, slots) in leader_schedule.iter() {
                    // Parse validator pubkey from string
                    if let Ok(pubkey) = Pubkey::from_str(pk_str) {
                        // Convert each relative slot to absolute slot and add to mapping
                        for slot in slots.iter() {
                            new_schedule.insert(*slot as u64 + epoch_offset, pubkey);
                        }
                    }
                }
                
                // Atomically replace the entire schedule cache
                *schedule.write().unwrap() = new_schedule;

                return true; // Successful update
            } else {
                error!("Couldn't Get Leader Schedule Update from RPC!!!")
            };
        } else {
            error!("Couldn't Get Epoch Info from RPC!!!")
        };
        
        false // Failed to update
    }
}
