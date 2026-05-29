use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

#[derive(Clone)]
pub struct PacketQueues {
    pub inbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
    pub outbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl PacketQueues {
    pub fn new() -> Self {
        Self {
            inbound: Arc::new(Mutex::new(VecDeque::new())),
            outbound: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn push_inbound(&self, packet: Vec<u8>) {
        self.inbound
            .lock()
            .expect("vpn inbound queue poisoned")
            .push_back(packet);
    }

    pub fn drain_outbound(&self) -> Vec<Vec<u8>> {
        let mut q = self.outbound.lock().expect("vpn outbound queue poisoned");
        q.drain(..).collect()
    }
}

pub struct VirtualDevice {
    queues: PacketQueues,
    mtu: usize,
}

impl VirtualDevice {
    pub fn new(queues: PacketQueues, mtu: usize) -> Self {
        Self { queues, mtu }
    }
}

impl Device for VirtualDevice {
    type RxToken<'a>
        = VirtualRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = VirtualTxToken
    where
        Self: 'a;

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self
            .queues
            .inbound
            .lock()
            .expect("vpn inbound queue poisoned")
            .pop_front()?;
        Some((
            VirtualRxToken { buffer: packet },
            VirtualTxToken {
                outbound: self.queues.outbound.clone(),
            },
        ))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(VirtualTxToken {
            outbound: self.queues.outbound.clone(),
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut cap = DeviceCapabilities::default();
        cap.medium = Medium::Ip;
        cap.max_transmission_unit = self.mtu;
        cap
    }
}

pub struct VirtualRxToken {
    buffer: Vec<u8>,
}

impl RxToken for VirtualRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buffer)
    }
}

pub struct VirtualTxToken {
    outbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl TxToken for VirtualTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        self.outbound
            .lock()
            .expect("vpn outbound queue poisoned")
            .push_back(buf);
        r
    }
}
