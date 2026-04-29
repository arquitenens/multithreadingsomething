use std::any::Any;
use std::fmt::Debug;
use std::intrinsics::transmute;
use std::io::{ErrorKind, Read, Write};
use std::mem::{transmute_copy, MaybeUninit};
use std::net::{TcpListener, TcpStream};
use std::ptr::{addr_of_mut, null_mut, NonNull};
use std::sync::atomic::AtomicU8;
use std::sync::{atomic, mpsc};
use std::time::{Duration, Instant};
use bitvec::order::{Lsb0, Msb0};
use bitvec::view::BitView;
use bytes::{Buf, BytesMut};
use crate::scheduler::{FromBytes, Scheduler};

#[derive(Debug)]
pub(crate) struct WorkerMetaRegion {
    //pub(crate) signals: u8, //bit masked out on scheduler side, highest bit is indicator of multiple signals at the same time
    pub(crate) thread_idx: u8,

    //if teamwork is needed
    //teamwork is a signal

    //will write to the u8 using the masked idx and the main thread only proceeds
    //if the U8 == 255 that means every thread has finished
    //will still write to the done when finished so the scheduler knows not to write to a non finished thread to avoid congestion
    pub(crate) done: *mut AtomicU8,
    pub(crate) finished_data_region: *mut u8,
}

pub struct Worker {
    pub meta: *mut WorkerMetaRegion,
    rx: mpsc::Receiver<(u8, *mut dyn FnMut() -> Option<*mut [u8]>, Box<dyn FromBytes + Send>)>,
}

fn some_expensive_calc() {
    let now = Instant::now();
    while now.elapsed() < Duration::from_millis(1000) {

    }
}

const MULTIPLE_COMMANDS: u8 = 7;
const NOT_READY: u8 = 6;
const TEAM_WORK: u8 = 5;
const EXECUTE: u8 = 4;
const CALLBACK: u8 = 3;
const PASS: u8 =  1;


unsafe impl Send for Worker {}

impl<'a> Worker {
    pub fn new(meta: *mut WorkerMetaRegion, rx: mpsc::Receiver<(u8, *mut dyn FnMut() -> Option<*mut [u8]>, Box<dyn FromBytes + Send>)>) -> Self {
        Worker { meta, rx }
    }
    fn set_free(&mut self) {
        let meta = unsafe { &mut *self.meta };
        let set_free = unsafe {&*meta.done}.load(atomic::Ordering::Acquire) & !(1 << meta.thread_idx);
        unsafe {(*meta.done).store(set_free, atomic::Ordering::Release);}
    }
    fn execute<T: FromBytes + Debug + 'static>(&mut self, src: *mut dyn FnMut() -> Option<*mut [u8]>, conversion: Box<dyn FromBytes>, is_teamwork: bool, is_callback: bool) {
        let mut closure_data = Vec::new();
        unsafe {
            if is_teamwork || is_callback {
                closure_data = Box::from_raw((*src)().unwrap_or_else(||panic!("no valid pointer"))).to_vec();
                let conv = T::from_bytes::<T>(&raw mut *closure_data.as_mut_slice());
                //only gives one thing and doesnt cast the whole array, would need to transmute for that
                println!("conv {:?}", conv.downcast::<T>())
            }else {
                let _ = (*src)();
            }
            println!("Executing {:?}", closure_data);
        }



    }
    fn modify(&mut self, src: Box<dyn FnMut() -> Option<NonNull<[u8]>>>, pred: *mut dyn Fn() -> bool, is_teamwork: bool, is_callback: bool) {
        todo!()
    }

    const fn check_set_bit(input: u8, bit: u8) -> bool {
        input & (1 << bit) != 0
    }

    pub(crate) fn process<T: FromBytes + Debug + 'static>(&mut self) {
        let meta = unsafe { &mut *self.meta };
        while let Ok((packed_command, execute, data_type)) = self.rx.recv(){
            println!("worker: {:?}", meta.thread_idx);
            let mut is_teamwork = false;
            let mut has_callback = false;
            //println!("Processing {:?}", packed_command.view_bits::<Lsb0>());
            if Self::check_set_bit(packed_command, MULTIPLE_COMMANDS) {}

            if Self::check_set_bit(packed_command, NOT_READY) {}

            if Self::check_set_bit(packed_command, TEAM_WORK) {
                is_teamwork = true;
            }
            if Self::check_set_bit(packed_command, CALLBACK) {
                has_callback = true;
            }
            if Self::check_set_bit(packed_command, PASS) {
                //callback is already false so nothing happens
            }
            if Self::check_set_bit(packed_command, EXECUTE) {
            self.execute::<T>(execute, data_type, is_teamwork, has_callback);
            }
            
            self.set_free()
        }
    }

}

impl WorkerMetaRegion {
   pub fn new(done: *mut AtomicU8) -> WorkerMetaRegion {
       WorkerMetaRegion{
           //signals: 0,
           thread_idx: 0,
           done: done,
           finished_data_region: std::ptr::null_mut(),
       }
   } 
}
