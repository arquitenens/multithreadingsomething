use std::any::{Any, TypeId};
use std::{f64, mem, ptr};
use std::cell::UnsafeCell;
use std::collections::{HashMap, HashSet};
use crate::worker::{Worker, WorkerMetaRegion};
use bitvec::order::Lsb0;
use bitvec::slice::BitSlice;
use bytes::BytesMut;
use once_cell::sync::Lazy;
use std::fmt::Debug;
use std::io::Write;
use std::mem::MaybeUninit;
use std::ptr::{addr_of_mut, null_mut, slice_from_raw_parts_mut, NonNull};
use std::sync::atomic::Ordering::Acquire;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{available_parallelism, JoinHandle};
use crate::MAX_THREADS;

pub static mut PROCESSING_SIGNAL: Lazy<Vec<AtomicBool>> = Lazy::<Vec<AtomicBool>>::new(|| Vec::<AtomicBool>::new());
pub static mut SCHEDULER_ACCEPTED: AtomicBool = AtomicBool::new(false);
#[allow(static_mut_refs)]
const THREADS_PER_SCHEDULER: usize = 8;
#[derive(Debug)]
pub struct Scheduler{
    pub idx: usize,
    pub completion: *mut AtomicU8,

    //each scheduler adds a new node, to communicate they will simply try to read the
    //next node thus read the scheduler that comes after it


    pub process_test: HashMap<TypeId, AtomicBool>,
    pub process_next: Arc<*mut Vec<AtomicBool>>, //will propagate down the line
    pub team_work_buffer: BytesMut, //each scheduler has their own owned tangled buffer
    //TODO make sure to drop the workers because they dont get dropped even if initialize
    //spread workload between workers
    pub raw_workers: Vec<*mut Worker>,
    pub workers: [MaybeUninit<mpsc::Sender<(u8, *mut dyn FnMut() -> Option<*mut [u8]>, Box<dyn FromBytes + Send>)>>; THREADS_PER_SCHEDULER],
    pub handles: [MaybeUninit<JoinHandle<()>>; THREADS_PER_SCHEDULER],
}

pub struct SchedulerHandle{
    inner: *mut InnerSchedulerHandle
}
impl SchedulerHandle {
    pub fn any<F, CommandType, DataType>(&mut self, exec: F) -> Result<(), String>
    where F: FnMut() -> Option<*mut [u8]> + 'static,
          CommandType: WorkerResults,
          DataType: FromBytes + Send + 'static,
    {

        let new_self = unsafe {&mut *self.inner};

        if !new_self.generic_targets.contains_key(&TypeId::of::<DataType>()) {
            return Err("wrong type".to_string());
        }
        let mut counter = 0;
        let generic_inners = new_self.generic_targets.get_mut(&TypeId::of::<DataType>()).unwrap();
        //println!("generic_inners: {:?}", generic_inners);
        let len = generic_inners.len();
        let mut s_exec = Some(exec);
        while !unsafe {(*addr_of_mut!(SCHEDULER_ACCEPTED)).load(Ordering::Acquire)} {
            unsafe {(*new_self.inners)[generic_inners[counter % len]].process::<F, CommandType, DataType>(&mut s_exec)};
            counter += 1;
        }
        let _ = s_exec.take();
        unsafe {(*addr_of_mut!(SCHEDULER_ACCEPTED)).store(false, Ordering::Release)};
        return Ok(())
    }
}

struct InnerSchedulerHandle{
    //Holder for the Schedulers
    pub inners: Vec<Scheduler>,
    pub scheduler_accepted: *mut AtomicBool,

    //inner vec index, its stored in a vec because there might be
    //multiple schedulers with the same type e.g 6-7 all using i32
    pub generic_targets: HashMap<TypeId, Vec<usize>>
}


pub struct SchedulerConfig{
    max_threads: usize,
    threads_per_scheduler: usize,
    inner: *mut InnerSchedulerHandle
}
impl Default for SchedulerConfig {
    fn default() -> Self {
        Self{
            max_threads: MAX_THREADS,
            threads_per_scheduler: THREADS_PER_SCHEDULER,
            inner: Box::into_raw(Box::new(InnerSchedulerHandle{
                inners: vec![],
                scheduler_accepted: null_mut(),
                generic_targets: HashMap::new()
            }))
        }
    }
}


impl<'a> SchedulerConfig {
    pub fn new(max_threads: usize, threads_per_scheduler: usize) -> Self {
        Self{
            max_threads,
            threads_per_scheduler,
            inner: Box::into_raw(Box::new(InnerSchedulerHandle{
                generic_targets: HashMap::new(),
                scheduler_accepted: null_mut(),
                inners: vec![]
            }))
        }
    }

    ///not guaranteed to be 8 child threads, might also just be the remainder
    pub fn add_scheduler<T: FromBytes + 'static + Debug>(&mut self) -> &mut Self{
        let raw_inner = unsafe {(&mut *self.inner)};
        if available_parallelism().unwrap().get() <= raw_inner.inners.len() * THREADS_PER_SCHEDULER {
            panic!("cannot add scheduler without available parallelism");
        }
        //let mut scheduler_communication = Vec::new();
        let mut scheduler = Scheduler::new_inner();
        //println!("self.inner len: {}", raw_inner.inners.len());
        let len = raw_inner.inners.len();
        raw_inner
            .generic_targets
            .entry(TypeId::of::<T>())
            .or_insert_with(Vec::new)
            .push(len);
        for w in 0..self.threads_per_scheduler {
            let mut child = Box::new(scheduler.init_child(w, scheduler.completion as *mut _));
            scheduler.raw_workers.push(&raw mut *child);
            let ch = std::thread::spawn(move ||{
                child.process::<T>()
            });
            scheduler.handles[w] = MaybeUninit::new(ch);
        }
        //scheduler_communication.push(scheduler.process_next.clone());
        raw_inner.inners.push(scheduler);

        return self;
    }
    pub fn apply<'b>(&mut self) -> SchedulerHandle {
        let available_threads = available_parallelism()
            .unwrap_or_else(|_| panic!("upgrade your system dawg"))
            .get();
        let raw_inner = unsafe {(&mut *self.inner)};
        let diff = self.max_threads.abs_diff(available_threads);
        let actual_threads = available_threads - diff;
        let needed_schedulers_rem = actual_threads % THREADS_PER_SCHEDULER;
        unsafe {raw_inner.inners[0].process_next.as_mut().unwrap()[0].store(true, Ordering::Release)};
        let mask_unavailable = !((1 << needed_schedulers_rem) - 1);

        unsafe {(*raw_inner.inners.last().unwrap().completion).store(mask_unavailable, Ordering::Release)};
        return SchedulerHandle{
            inner: self.inner,
        }
    }
}


unsafe impl<'a> Send for Scheduler {}

pub trait WorkerResults {
    fn flag() -> WorkerCommands;
}

pub trait FromBytes{
    fn from_bytes<T>(bytes: *mut [u8]) -> Box<dyn Any + 'static> where Self: Sized;
}


impl FromBytes for i32 {
    fn from_bytes<T>(bytes: *mut [u8]) -> Box<dyn Any + 'static> {
        let arr: [u8; 4] = unsafe {(&*bytes)[0..4].try_into().unwrap()};
        Box::new(i32::from_ne_bytes(arr))
    }
}

impl FromBytes for f64 {
    fn from_bytes<T>(bytes: *mut [u8]) -> Box<dyn Any + 'static> {
        let arr: [u8; 8] = unsafe {(&*bytes)[0..8].try_into().unwrap()};
        Box::new(f64::from_ne_bytes(arr))
    }
}





pub struct ExpectResult;
impl WorkerResults for ExpectResult {
    fn flag() -> WorkerCommands {
        return WorkerCommands::CallBack
    }
}
pub struct ExpectTeamwork;
impl WorkerResults for ExpectTeamwork {
    fn flag() -> WorkerCommands {
        return WorkerCommands::TeamWork
    }
}
pub struct ExpectNothing;
impl WorkerResults for ExpectNothing {
    fn flag() -> WorkerCommands {
        return WorkerCommands::Pass
    }
}


#[derive(Debug)]
pub enum WorkerCommands{
    NotReady,
    TeamWork,
    Execute,
    CallBack,
    Pass,
}


impl<'a> Scheduler {
    fn new_inner() -> Self {
        unsafe {
            let list=  Lazy::force_mut(&mut *&raw mut PROCESSING_SIGNAL);
            return Scheduler{
                raw_workers: Vec::new(),
                idx: {
                    let l = list.len();
                    list.push(AtomicBool::new(false));
                    l
                },
                completion: Box::into_raw(Box::new(AtomicU8::new(0))),
                process_test: HashMap::new(),
                process_next: Arc::new(list),
                team_work_buffer: BytesMut::new(),
                workers: [const { MaybeUninit::uninit() }; THREADS_PER_SCHEDULER],
                handles: [const { MaybeUninit::uninit() }; THREADS_PER_SCHEDULER],
            };
        }
    }

    fn init_child(&'_ mut self, idx: usize, result_ptr: *mut AtomicU8) -> Worker{
        let mut region = WorkerMetaRegion::new(std::ptr::from_ref(&result_ptr) as *mut _);
        region.done = self.completion;
        region.thread_idx = idx as u8;
        let (tx, rx) = mpsc::channel();
        self.workers[idx] = MaybeUninit::new(tx);
        let region_ptr = Box::new(region);
        let perm = Box::leak(region_ptr); //TODO leaks memory, gotta fiix :3
        Worker::new(perm, rx)
    }
    fn process_commands(&self, commands: &[WorkerCommands]) -> u8 {

        let mut packed_commands = 0;

        if commands.len() > 1{
            packed_commands |= 1 << 7;
        }
        for command in commands{
            match command{
                WorkerCommands::NotReady => {
                    packed_commands |= 1 << 6;
                }
                WorkerCommands::TeamWork => {
                    packed_commands |= 1 << 5;
                }
                WorkerCommands::Execute => {
                    packed_commands |= 1 << 4;
                }
                WorkerCommands::CallBack => {
                    packed_commands |= 1 << 3;
                }
                WorkerCommands::Pass => {
                    packed_commands |= 1 << 2;
                }
            }
        }
        packed_commands

    }


    pub fn process<F, CommandType, DataType>(&mut self, exec: &mut Option<F>) -> ()
    where F: FnMut() -> Option<*mut [u8]> + 'static,
        CommandType: WorkerResults,
        DataType: FromBytes + Send + 'static,
    {
        //quickly exit


        //TODO dont check EVERY scheduler, only the ones with the type of DataType
        let process = unsafe {self.process_next.as_mut().unwrap()};
        let my_turn = process[self.idx].load(Acquire);
        if !my_turn {
            return;
        }

        //mark the scheduler as accepted
        unsafe {(*addr_of_mut!(SCHEDULER_ACCEPTED)).store(true, Ordering::Release)}

        let exec = exec.take().unwrap();

        let fake_data: MaybeUninit<DataType> = MaybeUninit::uninit();

        let command = CommandType::flag();


        let completion_ref = unsafe {&mut *self.completion};

        let next_available_thread: Option<usize> = BitSlice::<AtomicU8, Lsb0>::from_element(completion_ref).first_zero();
        if let Some(idx) = next_available_thread {
            let current = completion_ref.load(Ordering::Acquire);
            completion_ref.store(current | (1 << idx), Ordering::Release);
            unsafe {
                let command = self.process_commands(&[WorkerCommands::Execute, command]); //maybe allow multiple commands?
                let _ = self.handles[idx].assume_init_mut().thread().unpark();
                let res = self.workers[idx].assume_init_mut().send(
                    (command, Box::into_raw(Box::new(exec)), Box::new(fake_data.assume_init()))
                );

            }
        }else {
            let wrapping_index = (self.idx + 1) % process.len();
            if let Some(_) = unsafe {process.get(wrapping_index)} {
                process[self.idx].store(false, Ordering::Release);
                process[wrapping_index].store(true, Ordering::Release);

            }
        }

    }
}
