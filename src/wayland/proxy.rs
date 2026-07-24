use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::wayland::context::Context;

pub trait Proxy: Send + Sync {
    fn id(&self) -> u32;
    fn set_id(&mut self, id: u32);
    fn context(&self) -> Option<Arc<Context>>;
    fn set_context(&mut self, ctx: Arc<Context>);
    fn is_zombie(&self) -> bool;
    fn mark_zombie(&self);
}

pub struct BaseProxy {
    ctx: Option<Arc<Context>>,
    id: u32,
    zombie: AtomicBool,
}

impl BaseProxy {
    pub fn new() -> Self {
        Self {
            ctx: None,
            id: 0,
            zombie: AtomicBool::new(false),
        }
    }
}

impl Proxy for BaseProxy {
    fn id(&self) -> u32 {
        self.id
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn context(&self) -> Option<Arc<Context>> {
        self.ctx.clone()
    }

    fn set_context(&mut self, ctx: Arc<Context>) {
        self.ctx = Some(ctx);
    }

    fn is_zombie(&self) -> bool {
        self.zombie.load(Ordering::Acquire)
    }

    fn mark_zombie(&self) {
        self.zombie.store(true, Ordering::Release);
    }
}
