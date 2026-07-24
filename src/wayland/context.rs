use std::{
    io::IoSliceMut,
    net,
    os::{
        fd::RawFd,
        unix::{io::AsRawFd, net::UnixStream},
    },
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use crate::wayland::{common::Display, event::WaylandMessage, proxy::Proxy};
use dashmap::DashMap;
use nix::sys::socket::{ControlMessageOwned, MsgFlags, recvmsg};

pub struct Context {
    conn: Arc<Mutex<UnixStream>>,
    objects: DashMap<u32, Arc<Mutex<dyn Proxy>>>,
    currrent_id: AtomicU32,
}

impl Context {
    pub fn new(conn: UnixStream) -> Arc<Self> {
        Arc::new(Self {
            conn: Arc::new(Mutex::new(conn)),
            objects: DashMap::new(),
            currrent_id: AtomicU32::new(0),
        })
    }

    pub fn register(self: &Arc<Self>, proxy: Arc<Mutex<dyn Proxy>>) {
        let id = self.currrent_id.fetch_add(1, Ordering::Relaxed);

        {
            let mut p = proxy.lock().unwrap();
            p.set_id(id);
            p.set_context(self.clone());
        }

        self.objects.insert(id, proxy);
    }

    pub fn register_with_id(self: &Arc<Self>, proxy: Arc<Mutex<dyn Proxy>>, id: u32) {
        let mut p = proxy.lock().unwrap();
        p.set_id(id);
        p.set_context(self.clone());
        self.objects.insert(id, proxy.clone());
    }

    pub fn unregister(&self, proxy: &Arc<Mutex<dyn Proxy>>) {
        self.objects.remove(&proxy.lock().unwrap().id());
    }

    pub fn delete_id(&self, id: u32) {
        self.objects.remove(&id);
    }

    pub fn get_proxy(&self, id: u32) -> Option<Arc<Mutex<dyn Proxy>>> {
        self.objects.get(&id).map(|v| v.clone())
    }

    pub fn close(&self) {
        self.conn.lock().unwrap().shutdown(net::Shutdown::Both);
    }

    pub fn fd(&self) -> i32 {
        self.conn.lock().unwrap().as_raw_fd()
    }

    pub fn connect(addr: Option<String>) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        let addr = match addr {
            Some(v) => v,
            None => {
                let runtime_dir = std::env::var("XDG_RUNTIME_DIR")?;
                let display = std::env::var("WAYLAND_DISPLAY").unwrap_or("wayland-0".into());

                format!("{}/{}", runtime_dir, display)
            }
        };

        let conn = UnixStream::connect(addr)?;

        Ok(Self::new(conn))
    }

    pub fn read_msg(&self) -> Result<WaylandMessage, Box<dyn std::error::Error>> {
        let mut conn = self.conn.lock().unwrap();
        let fd = conn.as_raw_fd();
        let mut header = [0u8; 8];
        let mut cmsg_buf = nix::cmsg_space!([RawFd; 1]);
        let mut iov = [IoSliceMut::new(&mut header)];
        let msg = recvmsg::<()>(fd, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())?;

        if msg.bytes != 8 {
            return Err(format!("incorrect header size {}", msg.bytes).into());
        }

        let mut recv_fd = None;

        for cmsg in msg.cmsgs()? {
            if let ControlMessageOwned::ScmRights(fds) = cmsg {
                if let Some(fd) = fds.first() {
                    recv_fd = Some(*fd);
                }
            }
        }

        let sender_id = u32::from_ne_bytes(header[0..4].try_into()?);
        let opcode_and_size = u32::from_ne_bytes(header[4..8].try_into()?);
        let opcode = opcode_and_size & 0xffff;
        let size = opcode_and_size >> 16;

        let msg_size = size as usize - 8;
        if msg_size == 0 {
            return Ok(WaylandMessage {
                sender_id,
                opcode,
                fd: recv_fd,
                msg: Vec::new(),
            });
        }

        let mut msg = vec![0u8; msg_size];

        if recv_fd.is_some() {
            use std::io::Read;
            conn.read_exact(&mut msg)?;
        } else {
            let mut cmsg_buf = nix::cmsg_space!([RawFd; 1]);
            let mut iov = [IoSliceMut::new(&mut msg)];
            let recv_msg = recvmsg::<()>(fd, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())?;

            if recv_msg.bytes != msg_size {
                return Err(format!(
                    "incorrect msg size {} expected {}",
                    recv_msg.bytes, msg_size
                )
                .into());
            }

            for cmsg in recv_msg.cmsgs()? {
                if let ControlMessageOwned::ScmRights(fds) = cmsg {
                    if let Some(fd) = fds.first() {
                        recv_fd = Some(*fd);
                    }
                }
            }
        }

        Ok(WaylandMessage {
            sender_id,
            opcode,
            fd: recv_fd,
            msg,
        })
    }

    pub fn get_dispatch(&self) -> Result<(), Box<dyn std::error::Error>> {
        let wayland_msg = self.read_msg()?;
        let proxy = match self.objects.get(&wayland_msg.sender_id) {
            Some(p) => p.clone(),
            None => Ok(()),
        };

        todo!()
    }

    pub fn dispatch(&self) -> Result<(), DispatchError> {
        todo!()
    }
}

pub fn connect(addr: Option<String>) -> Result<Arc<Display>, Box<dyn std::error::Error>> {
    let ctx = Context::connect(addr)?;
    let display = Display::new(ctx.clone());

    Ok(display)
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("dispatch: unable to read msg: {0}")]
    UnableToReadMsg(#[from] std::io::Error),

    #[error("dispatch: sender does not support Dispatch (senderID={0})")]
    SenderUnsupported(u32),

    #[error("dispatch: panic handling opcode={opcode} senderID={sender_id}: {message}")]
    Panic {
        opcode: u32,
        sender_id: u32,
        message: String,
    },
}
