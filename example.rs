

mod worker;
mod scheduler;
mod tangled;

use std::io::{ErrorKind, Read, Write};
use crate::worker::Worker;
use crate::scheduler::{ExpectNothing, ExpectResult, ExpectTeamwork, Scheduler};

use std::{net, ptr};
use std::mem::{transmute_copy, ManuallyDrop};
use std::net::{TcpListener, TcpStream};
use std::ptr::{null, null_mut, slice_from_raw_parts_mut, NonNull};
use std::thread;
use httparse::Request;
use std::thread::available_parallelism;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use bitvec::order::Lsb0;
use bitvec::vec::BitVec;
use bitvec::view::BitView;
use crate::tangled::Tangled;

fn cast_data_t<T>(data: &mut [T]) -> &mut [u8] {
    let len = data.len();
    let size_of = size_of::<T>();

    let first_byte = &mut data[0] as *const _ as *mut u8;

    let unaligned_slice = unsafe {slice_from_raw_parts_mut(first_byte, len * size_of).as_mut()}.unwrap();
    //let aligned_slice = unsafe {(&mut *unaligned_slice).align_to_mut::<T>().2};
    return unaligned_slice;
}
const WORKER_PER_SCHEDULER: usize = 8;
const MAX_THREADS: usize = 12;

fn main() {

    let mut schedulers = scheduler::SchedulerConfig::new(12, 8)
                                    .add_scheduler::<i32>()
                                    .add_scheduler::<i32>()
                                    .apply();
    let x = 5;

    let mut heap = vec![6767; 100].into_boxed_slice();

    let func1 = move ||{
        let buffer = ptr::from_mut(&mut *cast_data_t(&mut heap));
        println!("Hello, world! {:?}", x);

        Some(buffer)
    };

    let mut heap = vec![420; 100].into_boxed_slice();

    let func2 = move ||{
        let buffer = ptr::from_mut(&mut *cast_data_t(&mut heap));
        println!("Hello, world! {:?}", x);

        Some(buffer)
    };


    let execute = schedulers.any::<_, ExpectResult, i32>(func1);
    let execute = schedulers.any::<_, ExpectResult, i32>(func2);

    loop {

    }




}
