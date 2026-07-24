use std::os::fd::RawFd;

pub struct WaylandMessage {
    pub sender_id: u32,
    pub opcode: u32,
    pub fd: Option<RawFd>,
    pub msg: Vec<u8>,
}
