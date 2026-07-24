use std::os::fd::RawFd;
use std::sync::Arc;

use super::context::Context;

use super::proxy::{BaseProxy, Proxy};

pub trait Dispatcher: Proxy {
    fn dispatch(&self, opcode: u32, fd: Option<RawFd>, data: &[u8]);
}

pub struct DisplayErrorEvent {
    object_id: Arc<dyn Proxy>,
    code: u32,
    message: String,
}
pub type DisplayErrorHandlerFunc = Box<dyn Fn(DisplayErrorEvent) + Send + Sync>;

pub struct DisplayDeleteIdEvent {
    id: u32,
}
pub type DisplayDeleteIdHandlerFunc = Box<dyn Fn(DisplayDeleteIdEvent) + Send + Sync>;

pub struct Display {
    base_proxy: BaseProxy,
    error_handler: Option<DisplayErrorHandlerFunc>,
    delete_id_handler: Option<DisplayDeleteIdHandlerFunc>,
}

impl Display {
    pub fn new(ctx: Arc<Context>) -> Arc<Self> {
        let mut display = Self {
            base_proxy: BaseProxy::new(),
            error_handler: None,
            delete_id_handler: None,
        };
        todo!()
    }
}
